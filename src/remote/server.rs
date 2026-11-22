//! Multi-threaded zero-crate TCP/HTTP/SSE server bridged safely to the GTK main loop.

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::remote::auth::{local_hostname, PairedDevice, RemoteAuthStore, PAIRING_TTL_SECS};
use crate::remote::http::{
    read_http_request, route_request, HttpResponse, RemoteBridgeRequest, RemoteBridgeResponse,
    RouteOutcome,
};
use crate::remote::protocol::{build_qr_pair_url, encode_status_json};
use crate::remote::sse::SseHub;

const MAX_HTTP_WORKERS: usize = 24;
const HTTP_IO_TIMEOUT: Duration = Duration::from_secs(5);
const BRIDGE_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

type ReplySlot = Arc<(Mutex<Option<RemoteBridgeResponse>>, Condvar)>;
pub type BridgeQueue = Arc<Mutex<VecDeque<PendingRemoteRequest>>>;
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

pub struct PendingRemoteRequest {
    pub request: RemoteBridgeRequest,
    reply: ReplySlot,
}

impl PendingRemoteRequest {
    pub fn respond(self, response: RemoteBridgeResponse) {
        let (lock, cvar) = &*self.reply;
        if let Ok(mut slot) = lock.lock() {
            *slot = Some(response);
            cvar.notify_one();
        }
    }
}

pub struct RemoteServer {
    bind_ip: String,
    port: u16,
    host_name: String,
    auth: Arc<Mutex<RemoteAuthStore>>,
    sse: Arc<SseHub>,
    queue: BridgeQueue,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

fn parse_bind_addr(bind_ip: &str, port: u16) -> io::Result<SocketAddr> {
    format!("{bind_ip}:{port}")
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid bind address: {e}")))
}

impl RemoteServer {
    pub fn start(bind_ip: String, port: u16, devices_path: PathBuf, on_wake: Option<WakeFn>) -> io::Result<Self> {
        let listener = TcpListener::bind(parse_bind_addr(&bind_ip, port)?)?;
        let actual_port = listener.local_addr()?.port();
        let host_name = local_hostname();
        let auth = Arc::new(Mutex::new(RemoteAuthStore::load(devices_path)));
        let sse = Arc::new(SseHub::new());
        let queue: BridgeQueue = Arc::new(Mutex::new(VecDeque::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let ctx = Arc::new(WorkerCtx {
            host_name: host_name.clone(), auth: Arc::clone(&auth), sse: Arc::clone(&sse),
            queue: Arc::clone(&queue), on_wake,
        });
        let handle = spawn_accept_loop(listener, ctx, Arc::clone(&shutdown))?;
        Ok(Self { bind_ip, port: actual_port, host_name, auth, sse, queue, shutdown, accept_thread: Some(handle) })
    }

    pub fn bind_ip(&self) -> &str {
        &self.bind_ip
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn host_name(&self) -> &str {
        &self.host_name
    }

    pub fn sse(&self) -> &Arc<SseHub> {
        &self.sse
    }

    pub fn drain_pending(&self) -> Vec<PendingRemoteRequest> {
        self.queue.lock().map_or_else(|_| Vec::new(), |mut q| q.drain(..).collect())
    }

    pub fn ensure_pairing_url(&self) -> io::Result<(String, u64)> {
        let now = Instant::now();
        let mut guard = self.auth.lock().unwrap_or_else(|e| e.into_inner());
        let (code, ttl) = match guard.active_pairing_code(now) {
            Some((existing, rem)) => (existing.to_owned(), rem),
            None => (guard.start_pairing(now)?, PAIRING_TTL_SECS),
        };
        Ok((build_qr_pair_url(&self.bind_ip, self.port, &code, &self.host_name), ttl))
    }

    pub fn rotate_pairing_url(&self) -> io::Result<(String, u64)> {
        let mut guard = self.auth.lock().unwrap_or_else(|e| e.into_inner());
        let code = guard.start_pairing(Instant::now())?;
        Ok((build_qr_pair_url(&self.bind_ip, self.port, &code, &self.host_name), PAIRING_TTL_SECS))
    }

    pub fn list_devices(&self) -> Vec<PairedDevice> {
        self.auth.lock().map_or_else(|_| Vec::new(), |g| g.devices().to_vec())
    }

    pub fn revoke_device(&self, device_id: &str) -> bool {
        let removed = self.auth.lock().map_or(false, |mut g| g.revoke_device(device_id));
        if removed {
            self.sse.disconnect_device(device_id);
        }
        removed
    }

    pub fn connected_clients(&self) -> usize {
        self.sse.client_count()
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.sse.close_all();
        let _ = TcpStream::connect((self.bind_ip.as_str(), self.port));
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
    }
}

struct WorkerCtx {
    host_name: String,
    auth: Arc<Mutex<RemoteAuthStore>>,
    sse: Arc<SseHub>,
    queue: BridgeQueue,
    on_wake: Option<WakeFn>,
}

fn spawn_accept_loop(
    listener: TcpListener,
    ctx: Arc<WorkerCtx>,
    shutdown: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    let active = Arc::new(AtomicUsize::new(0));
    thread::Builder::new()
        .name("akhterm-remote-http".into())
        .spawn(move || accept_loop(listener, ctx, shutdown, active))
}

fn accept_loop(listener: TcpListener, ctx: Arc<WorkerCtx>, shutdown: Arc<AtomicBool>, active: Arc<AtomicUsize>) {
    for incoming in listener.incoming() {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        if let Ok(stream) = incoming {
            spawn_http_worker(stream, &ctx, &active);
        }
    }
}

fn reject_busy(mut stream: TcpStream) {
    let _ = HttpResponse::error(503, "Service Unavailable", "server busy").write_to(&mut stream);
}

fn spawn_http_worker(stream: TcpStream, ctx: &Arc<WorkerCtx>, active: &Arc<AtomicUsize>) {
    if active.fetch_add(1, Ordering::AcqRel) >= MAX_HTTP_WORKERS {
        active.fetch_sub(1, Ordering::AcqRel);
        reject_busy(stream);
        return;
    }
    let ctx = Arc::clone(ctx);
    let active = Arc::clone(active);
    let _ = thread::Builder::new().name("akhterm-remote-req".into()).spawn(move || {
        handle_tcp_client(stream, &ctx);
        active.fetch_sub(1, Ordering::AcqRel);
    });
}

fn dispatch_client_http(req: &crate::remote::http::HttpRequest, ctx: &WorkerCtx) -> RouteOutcome {
    let mut auth = ctx.auth.lock().unwrap_or_else(|e| e.into_inner());
    route_request(
        req,
        &mut auth,
        &ctx.host_name,
        env!("CARGO_PKG_VERSION"),
        |dev_id| ctx.sse.disconnect_device(dev_id),
        |breq| enqueue_and_wait(breq, &ctx.queue, ctx.on_wake.as_ref()),
    )
}

fn attach_sse_client(stream: TcpStream, device_id: String, pane_id: Option<String>, ctx: &WorkerCtx) {
    let snap = match enqueue_and_wait(RemoteBridgeRequest::Status, &ctx.queue, ctx.on_wake.as_ref()) {
        RemoteBridgeResponse::Status(s) => Some(encode_status_json(&s)),
        _ => None,
    };
    let _ = ctx.sse.add_client(stream, device_id, pane_id, snap.as_deref());
}

fn handle_tcp_client(mut stream: TcpStream, ctx: &WorkerCtx) {
    let _ = stream.set_read_timeout(Some(HTTP_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HTTP_IO_TIMEOUT));
    let req = match read_http_request(&stream) {
        Ok(r) => r,
        Err(resp) => { let _ = resp.write_to(&mut stream); return; }
    };
    match dispatch_client_http(&req, ctx) {
        RouteOutcome::Response(resp) => { let _ = resp.write_to(&mut stream); }
        RouteOutcome::SseSubscribe { device_id, pane_id } => attach_sse_client(stream, device_id, pane_id, ctx),
    }
}

fn push_bridge_request(request: RemoteBridgeRequest, queue: &BridgeQueue, on_wake: Option<&WakeFn>) -> Option<ReplySlot> {
    let reply: ReplySlot = Arc::new((Mutex::new(None), Condvar::new()));
    queue.lock().ok()?.push_back(PendingRemoteRequest { request, reply: Arc::clone(&reply) });
    if let Some(wake) = on_wake {
        wake();
    }
    Some(reply)
}

fn wait_for_reply(reply: &ReplySlot) -> RemoteBridgeResponse {
    let (lock, cvar) = &**reply;
    let Ok(guard) = lock.lock() else { return RemoteBridgeResponse::Error("reply lock poisoned".into()) };
    match cvar.wait_timeout_while(guard, BRIDGE_WAIT_TIMEOUT, |slot| slot.is_none()) {
        Ok((mut slot, _)) => slot.take().unwrap_or_else(|| RemoteBridgeResponse::Error("timed out".into())),
        Err(_) => RemoteBridgeResponse::Error("wait failed".into()),
    }
}

fn enqueue_and_wait(request: RemoteBridgeRequest, queue: &BridgeQueue, on_wake: Option<&WakeFn>) -> RemoteBridgeResponse {
    match push_bridge_request(request, queue, on_wake) {
        Some(reply) => wait_for_reply(&reply),
        None => RemoteBridgeResponse::Error("bridge lock poisoned".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn http_get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn remote_server_binds_loopback_and_serves_health_and_pwa() {
        let store_path = std::env::temp_dir().join(format!("akhterm-srv-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&store_path);
        let srv = RemoteServer::start("127.0.0.1".into(), 0, store_path.clone(), None).unwrap();
        let health = http_get(srv.port(), "/api/v1/health");
        assert!(health.contains("200 OK") && health.contains("\"api_version\":1"));
        assert!(http_get(srv.port(), "/").contains("AkhTerm Remote"));
        drop(srv);
        let _ = std::fs::remove_file(&store_path);
    }
}
