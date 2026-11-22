//! Unix domain socket control server, stale cleanup, discovery, and client transport.

use std::collections::VecDeque;
use std::fs::{self, Permissions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use crate::sys::ctl_protocol::{
    decode_request, decode_response, encode_request, encode_response, CtlRequest, CtlResponse,
};

const MAX_LINE_BYTES: u64 = 256 * 1024;
const MAX_CONCURRENT_CLIENTS: usize = 16;
const CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(3);
const REPLY_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

extern "C" {
    fn getuid() -> u32;
}

static ACTIVE_SOCKET: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

pub fn current_uid() -> u32 {
    unsafe { getuid() }
}

pub fn set_active_socket_path(path: Option<PathBuf>) {
    let lock = ACTIVE_SOCKET.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = lock.lock() {
        *guard = path;
    }
}

pub fn current_socket_path() -> Option<PathBuf> {
    ACTIVE_SOCKET.get()?.lock().ok()?.clone()
}

pub fn resolve_socket_path(runtime_dir: Option<&Path>, uid: u32, pid: u32) -> PathBuf {
    if let Some(dir) = runtime_dir.filter(|p| p.is_absolute() && p.is_dir()) {
        return dir.join(format!("akhterm-{pid}.sock"));
    }
    PathBuf::from(format!("/tmp/akhterm-{uid}-{pid}.sock"))
}

pub fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    resolve_socket_path(runtime.as_deref(), current_uid(), std::process::id())
}

pub fn is_akhterm_socket_name(name: &str) -> bool {
    name.starts_with("akhterm-") && name.ends_with(".sock")
}

pub fn parse_socket_pid(name: &str) -> Option<u32> {
    let body = name.strip_prefix("akhterm-")?.strip_suffix(".sock")?;
    body.rsplit('-').next()?.parse::<u32>().ok()
}

fn is_socket_stale(path: &Path, name: &str) -> bool {
    if let Some(pid) = parse_socket_pid(name) {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            return true;
        }
    }
    match UnixStream::connect(path) {
        Ok(_) => false,
        Err(err) => matches!(
            err.kind(),
            io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
        ),
    }
}

fn maybe_remove_stale_entry(entry: &fs::DirEntry) -> bool {
    let name = entry.file_name();
    let name_str = name.to_string_lossy();
    if !is_akhterm_socket_name(&name_str) {
        return false;
    }
    let path = entry.path();
    let owned_sock = entry
        .metadata()
        .is_ok_and(|m| m.file_type().is_socket() && m.uid() == current_uid());
    owned_sock && is_socket_stale(&path, &name_str) && fs::remove_file(&path).is_ok()
}

pub fn cleanup_stale_sockets_in(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries.flatten().filter(|e| maybe_remove_stale_entry(e)).count()
}

type ReplyPair = Arc<(Mutex<Option<CtlResponse>>, Condvar)>;
pub type CtlQueue = Arc<Mutex<VecDeque<PendingCtlRequest>>>;
pub type WakeCallback = Arc<dyn Fn() + Send + Sync>;

pub struct PendingCtlRequest {
    pub request: CtlRequest,
    reply: ReplyPair,
}

impl PendingCtlRequest {
    pub fn respond(self, resp: CtlResponse) {
        let (lock, cvar) = &*self.reply;
        if let Ok(mut slot) = lock.lock() {
            *slot = Some(resp);
            cvar.notify_one();
        }
    }
}

pub struct CtlServer {
    path: PathBuf,
    queue: CtlQueue,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

impl CtlServer {
    pub fn bind(on_wake: Option<WakeCallback>) -> io::Result<Self> {
        let path = default_socket_path();
        if let Some(parent) = path.parent() {
            let _ = cleanup_stale_sockets_in(parent);
        }
        Self::bind_at(path, on_wake)
    }

    pub fn bind_at(path: PathBuf, on_wake: Option<WakeCallback>) -> io::Result<Self> {
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, Permissions::from_mode(0o600))?;
        let queue: CtlQueue = Arc::new(Mutex::new(VecDeque::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_accept_loop(
            listener,
            Arc::clone(&queue),
            Arc::clone(&shutdown),
            on_wake,
        )?;
        Ok(Self { path, queue, shutdown, accept_thread: Some(handle) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn drain_pending(&self) -> Vec<PendingCtlRequest> {
        drain_queue(&self.queue)
    }

    #[allow(dead_code)]
    pub fn queue(&self) -> CtlQueue {
        Arc::clone(&self.queue)
    }
}

impl Drop for CtlServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
        let _ = fs::remove_file(&self.path);
        if current_socket_path().as_deref() == Some(self.path.as_path()) {
            set_active_socket_path(None);
        }
    }
}

pub fn drain_queue(queue: &CtlQueue) -> Vec<PendingCtlRequest> {
    match queue.lock() {
        Ok(mut guard) => guard.drain(..).collect(),
        Err(_) => Vec::new(),
    }
}

fn spawn_accept_loop(
    listener: UnixListener,
    queue: CtlQueue,
    shutdown: Arc<AtomicBool>,
    on_wake: Option<WakeCallback>,
) -> io::Result<JoinHandle<()>> {
    let active_clients = Arc::new(AtomicUsize::new(0));
    thread::Builder::new()
        .name("akhterm-ctl-sock".to_owned())
        .spawn(move || accept_loop(listener, queue, shutdown, on_wake, active_clients))
}

fn accept_loop(
    listener: UnixListener,
    queue: CtlQueue,
    shutdown: Arc<AtomicBool>,
    on_wake: Option<WakeCallback>,
    active_clients: Arc<AtomicUsize>,
) {
    for incoming in listener.incoming() {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let Ok(stream) = incoming else { continue };
        spawn_client_worker(stream, &queue, &on_wake, &active_clients);
    }
}

fn spawn_client_worker(
    stream: UnixStream,
    queue: &CtlQueue,
    on_wake: &Option<WakeCallback>,
    active_clients: &Arc<AtomicUsize>,
) {
    if active_clients.fetch_add(1, Ordering::AcqRel) >= MAX_CONCURRENT_CLIENTS {
        active_clients.fetch_sub(1, Ordering::AcqRel);
        let _ = write_response_line(&stream, &CtlResponse::Error {
            message: "server busy".to_owned(),
        });
        return;
    }
    let q = Arc::clone(queue);
    let wake = on_wake.clone();
    let count = Arc::clone(active_clients);
    let _ = thread::Builder::new()
        .name("akhterm-ctl-client".to_owned())
        .spawn(move || {
            handle_client_stream(stream, q, wake);
            count.fetch_sub(1, Ordering::AcqRel);
        });
}

fn handle_client_stream(stream: UnixStream, queue: CtlQueue, on_wake: Option<WakeCallback>) {
    let _ = stream.set_read_timeout(Some(CLIENT_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CLIENT_IO_TIMEOUT));
    let Some(line) = read_bounded_line(&stream) else {
        return;
    };
    if line.trim().is_empty() {
        return;
    }
    let resp = match decode_request(&line) {
        Ok(request) => enqueue_and_wait(request, &queue, on_wake.as_ref()),
        Err(message) => CtlResponse::Error { message },
    };
    let _ = write_response_line(&stream, &resp);
}

fn enqueue_and_wait(
    request: CtlRequest,
    queue: &CtlQueue,
    on_wake: Option<&WakeCallback>,
) -> CtlResponse {
    let reply: ReplyPair = Arc::new((Mutex::new(None), Condvar::new()));
    let pending = PendingCtlRequest { request, reply: Arc::clone(&reply) };
    if let Ok(mut guard) = queue.lock() {
        guard.push_back(pending);
    } else {
        return CtlResponse::Error { message: "control queue poisoned".to_owned() };
    }
    if let Some(wake) = on_wake {
        wake();
    }
    wait_for_reply(&reply)
}

fn wait_for_reply(reply: &ReplyPair) -> CtlResponse {
    let (lock, cvar) = &**reply;
    let Ok(guard) = lock.lock() else {
        return CtlResponse::Error { message: "reply lock poisoned".to_owned() };
    };
    match cvar.wait_timeout_while(guard, REPLY_WAIT_TIMEOUT, |slot| slot.is_none()) {
        Ok((mut g, _)) => g.take().unwrap_or_else(|| CtlResponse::Error {
            message: "AkhTerm UI did not respond in time".to_owned(),
        }),
        Err(_) => CtlResponse::Error { message: "reply wait failed".to_owned() },
    }
}

fn read_bounded_line(stream: &UnixStream) -> Option<String> {
    let mut line = String::new();
    let mut reader = BufReader::new(stream.take(MAX_LINE_BYTES));
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

fn write_response_line(mut stream: &UnixStream, resp: &CtlResponse) -> io::Result<()> {
    let payload = format!("{}\n", encode_response(resp));
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

pub fn discover_socket() -> Result<PathBuf, String> {
    let env_sock = std::env::var_os("AKHTERM_SOCKET")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let mut dirs = Vec::new();
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(rt));
    }
    dirs.push(PathBuf::from("/tmp"));
    discover_socket_in(env_sock.as_deref(), &dirs)
}

pub fn discover_socket_in(
    env_socket: Option<&Path>,
    search_dirs: &[PathBuf],
) -> Result<PathBuf, String> {
    if let Some(path) = env_socket {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        return Err(format!("AKHTERM_SOCKET does not exist: {}", path.display()));
    }
    let uid = current_uid();
    let mut candidates = collect_candidate_sockets(search_dirs, uid);
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    for (path, _) in candidates {
        if UnixStream::connect(&path).is_ok() {
            return Ok(path);
        }
    }
    Err("no live AkhTerm control socket found (is AkhTerm running?)".to_owned())
}

fn socket_candidate_entry(entry: &fs::DirEntry, uid: u32) -> Option<(PathBuf, SystemTime)> {
    let name = entry.file_name();
    if !is_akhterm_socket_name(&name.to_string_lossy()) {
        return None;
    }
    let meta = entry.metadata().ok()?;
    (meta.file_type().is_socket() && meta.uid() == uid)
        .then(|| (entry.path(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
}

fn collect_candidate_sockets(dirs: &[PathBuf], uid: u32) -> Vec<(PathBuf, SystemTime)> {
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        found.extend(entries.flatten().filter_map(|e| socket_candidate_entry(&e, uid)));
    }
    found
}

pub fn send_request(socket_path: &Path, req: &CtlRequest) -> Result<CtlResponse, String> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|e| format!("cannot connect to {}: {e}", socket_path.display()))?;
    let _ = stream.set_read_timeout(Some(CLIENT_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CLIENT_IO_TIMEOUT));
    let payload = format!("{}\n", encode_request(req));
    stream.write_all(payload.as_bytes()).map_err(|e| format!("write failed: {e}"))?;
    stream.flush().map_err(|e| format!("flush failed: {e}"))?;
    let line = read_bounded_line(&stream)
        .ok_or_else(|| "empty response from AkhTerm control socket".to_owned())?;
    decode_response(&line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::ctl_protocol::{HookState, PaneStatusItem};

    fn test_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("akhterm-ctl-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test temp dir");
        dir
    }

    fn mock_dispatch(req: CtlRequest) -> CtlResponse {
        match req {
            CtlRequest::Status => CtlResponse::Status {
                version: "1.4.0".into(), pid: 100, socket: "test.sock".into(), active_tab: 1,
                panes: vec![PaneStatusItem {
                    pane_id: "1".into(), tab_id: 1, tab_title: "Work".into(), active_tab: true,
                    focused_pane: true, agent: "Claude".into(), state: "working".into(),
                    hook_state: Some("working".into()), hook_title: None, hook_message: None,
                    cwd: "/tmp".into(), pid: 42, ssh_host: None, unread: false,
                }],
            },
            CtlRequest::Focus { pane_id } => CtlResponse::Ok { message: Some(format!("focused {pane_id}")) },
            CtlRequest::Notify { pane_id, state, .. } => {
                CtlResponse::Ok { message: Some(format!("notified {pane_id} {}", state.as_str())) }
            }
            CtlRequest::Send { pane_id, text, enter } => {
                CtlResponse::Ok { message: Some(format!("sent {pane_id} {} {enter}", text.len())) }
            }
            CtlRequest::OpenDoc { path, .. } => CtlResponse::Ok { message: Some(format!("opened {path}")) },
            CtlRequest::Capture { pane_id, .. } => CtlResponse::Capture { pane_id, lines: vec!["ok".into()] },
            CtlRequest::SendKey { pane_id, key } => CtlResponse::Ok { message: Some(format!("key {key} -> {pane_id}")) },
        }
    }

    fn spawn_mock_pump(queue: CtlQueue) -> (Arc<AtomicBool>, JoinHandle<()>) {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let pump = thread::spawn(move || {
            while !stop_bg.load(Ordering::Acquire) {
                for item in drain_queue(&queue) {
                    let resp = mock_dispatch(item.request.clone());
                    item.respond(resp);
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        (stop, pump)
    }

    fn five_sample_requests() -> [CtlRequest; 5] {
        [
            CtlRequest::Status,
            CtlRequest::Focus { pane_id: "1".into() },
            CtlRequest::Notify {
                pane_id: "1".into(), state: HookState::Done,
                title: Some("Claude".into()), message: Some("Готово".into()), desktop: true,
            },
            CtlRequest::Send { pane_id: "1".into(), text: "ls -la".into(), enter: true },
            CtlRequest::OpenDoc { pane_id: Some("1".into()), path: "/tmp/doc.md".into() },
        ]
    }

    #[test]
    fn unix_socket_round_trips_all_five_commands_sets_0600_and_unlinks_on_drop() {
        let dir = test_temp_dir("roundtrip");
        let sock_path = dir.join("akhterm-999999.sock");
        let server = CtlServer::bind_at(sock_path.clone(), None).expect("bind server");
        let mode = fs::metadata(&sock_path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let (stop, pump) = spawn_mock_pump(server.queue());
        for req in five_sample_requests() {
            let resp = send_request(&sock_path, &req).expect("round-trip response");
            assert!(!matches!(resp, CtlResponse::Error { .. }));
        }
        stop.store(true, Ordering::Release);
        let _ = pump.join();
        drop(server);
        assert!(!sock_path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleans_stale_sockets_and_discovers_newest_live_socket() {
        let dir = test_temp_dir("discover");
        let stale = dir.join("akhterm-999998.sock");
        {
            let _dead = UnixListener::bind(&stale).expect("bind stale");
        }
        let live_path = dir.join(format!("akhterm-{}.sock", std::process::id()));
        let server = CtlServer::bind_at(live_path.clone(), None).expect("bind live");

        let found = discover_socket_in(None, std::slice::from_ref(&dir)).expect("discover live");
        assert_eq!(found, live_path);
        let override_found = discover_socket_in(Some(&live_path), &[]).expect("env override");
        assert_eq!(override_found, live_path);

        assert_eq!(cleanup_stale_sockets_in(&dir), 1);
        assert!(!stale.exists());
        assert!(live_path.exists());
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_runtime_dir_and_tmp_fallback_socket_paths() {
        let dir = test_temp_dir("paths");
        assert_eq!(resolve_socket_path(Some(&dir), 1000, 77), dir.join("akhterm-77.sock"));
        assert_eq!(resolve_socket_path(None, 1000, 77), PathBuf::from("/tmp/akhterm-1000-77.sock"));
        let _ = fs::remove_dir_all(&dir);
    }
}
