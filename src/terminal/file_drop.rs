//! GTK3 `text/uri-list` file drag-and-drop for local VTE panes and async SSH `scp` uploads.

use std::cell::Cell;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ffi::*;
use crate::tabs::is_ssh_host;
use crate::ui_text as text;

static DROP_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropTarget {
    Local,
    Remote { sid: usize, host: String },
    Ignore,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadCompletion {
    pub sid: usize,
    pub vte_addr: usize,
    pub injected_text: Option<String>,
    pub footer_msg: String,
}

#[derive(Clone, Copy)]
pub struct FileDropHooks {
    pub resolve_target: fn(*mut GtkWidget) -> DropTarget,
    pub set_status: fn(&str),
    pub complete_upload: fn(UploadCompletion),
}

thread_local! {
    static FILE_DROP_HOOKS: Cell<Option<FileDropHooks>> = const { Cell::new(None) };
}

pub fn register_file_drop_hooks(hooks: FileDropHooks) {
    FILE_DROP_HOOKS.with(|slot| slot.set(Some(hooks)));
}

fn current_hooks() -> Option<FileDropHooks> {
    FILE_DROP_HOOKS.with(Cell::get)
}

fn notify_drop_status(message: &str) {
    if let Some(hooks) = current_hooks() {
        (hooks.set_status)(message);
    }
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_percent_step(bytes: &[u8], idx: usize) -> (u8, usize) {
    if bytes[idx] == b'%' && idx + 2 < bytes.len() {
        if let (Some(hi), Some(lo)) = (hex_val(bytes[idx + 1]), hex_val(bytes[idx + 2])) {
            return ((hi << 4) | lo, 3);
        }
    }
    (bytes[idx], 1)
}

pub fn percent_decode_utf8(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        let (byte, step) = decode_percent_step(bytes, idx);
        if byte == 0 {
            return None;
        }
        out.push(byte);
        idx += step;
    }
    String::from_utf8(out).ok()
}

fn strip_file_scheme(uri: &str) -> Option<&str> {
    let trimmed = uri.trim_matches(['\r', '\n', ' ', '\t']);
    let rest = trimmed.strip_prefix("file://")?;
    if rest.starts_with('/') {
        return Some(rest);
    }
    let (host, _) = rest.split_once('/')?;
    host.eq_ignore_ascii_case("localhost").then_some(&rest[host.len()..])
}

pub fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    let raw_path = strip_file_scheme(uri)?;
    let decoded = percent_decode_utf8(raw_path)?;
    (!decoded.is_empty()).then(|| PathBuf::from(decoded))
}

pub fn decode_uri_list(uris: &[String]) -> Vec<PathBuf> {
    uris.iter()
        .map(String::as_str)
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(decode_file_uri)
        .collect()
}

fn is_safe_unquoted_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'-')
}

pub fn posix_shell_escape(raw: &str) -> String {
    if !raw.is_empty() && raw.bytes().all(is_safe_unquoted_byte) {
        return raw.to_owned();
    }
    format!("'{}'", raw.replace('\'', "'\\''"))
}

pub fn format_shell_paths<S: AsRef<str>>(paths: &[S]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let escaped: Vec<String> = paths.iter().map(|p| posix_shell_escape(p.as_ref())).collect();
    format!("{} ", escaped.join(" "))
}

pub fn format_local_drop_paths(paths: &[PathBuf]) -> String {
    let items: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .collect();
    format_shell_paths(&items)
}

fn push_sanitized_char(out: &mut String, ch: char) {
    let mapped = if ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_') { ch } else { '_' };
    if mapped == '.' && out.ends_with('_') {
        out.pop();
    }
    if matches!((out.chars().last(), mapped), (Some('_'), '_') | (Some('.'), '.')) {
        return;
    }
    out.push(mapped);
}

fn collapse_separators(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        push_sanitized_char(&mut out, ch);
    }
    out.trim_start_matches(['.', '-', '_']).trim_end_matches(['.', '_']).to_owned()
}

fn truncate_basename(clean: &str, max_chars: usize) -> String {
    if clean.chars().count() <= max_chars {
        return clean.to_owned();
    }
    if let Some((stem, ext)) = clean.rsplit_once('.') {
        let ext_len = ext.chars().count();
        if ext_len > 0 && ext_len + 2 <= max_chars {
            let head: String = stem.chars().take(max_chars - ext_len - 1).collect();
            return format!("{head}.{ext}");
        }
    }
    clean.chars().take(max_chars).collect()
}

pub fn sanitize_remote_basename(raw_name: &str) -> String {
    let leaf = raw_name.rsplit(['/', '\\']).next().unwrap_or(raw_name);
    let clean = truncate_basename(&collapse_separators(leaf), 64);
    if clean.is_empty() { "file".to_owned() } else { clean }
}

pub fn remote_drop_path(local_path: &Path, stamp: u64, seq: usize) -> String {
    let raw = local_path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let safe = sanitize_remote_basename(raw);
    format!("/tmp/akhterm_{stamp}_{seq}_{safe}")
}

fn next_drop_stamp() -> u64 {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let seq = DROP_SEQ.fetch_add(1, Ordering::Relaxed) & 0xffff;
    (secs << 16) | seq
}

pub fn is_valid_scp_host(host: &str) -> bool {
    is_ssh_host(host) && !host.contains('/')
}

fn bracket_ipv6_host(host_part: &str) -> String {
    if host_part.contains(':') && !host_part.starts_with('[') {
        format!("[{host_part}]")
    } else {
        host_part.to_owned()
    }
}

pub fn format_scp_destination(ssh_host: &str, remote_path: &str) -> String {
    if let Some((user, host_part)) = ssh_host.rsplit_once('@') {
        return format!("{user}@{}:{remote_path}", bracket_ipv6_host(host_part));
    }
    format!("{}:{remote_path}", bracket_ipv6_host(ssh_host))
}

fn safe_local_scp_arg(local_path: &Path) -> String {
    let raw = local_path.to_string_lossy();
    if raw.starts_with('/') || raw.starts_with("./") {
        raw.into_owned()
    } else {
        format!("./{raw}")
    }
}

pub fn scp_upload_args(local_path: &Path, ssh_host: &str, remote_path: &str) -> Vec<String> {
    let fixed = [
        "-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=6", "-o", "ClearAllForwardings=yes", "--",
    ];
    let mut args: Vec<String> = fixed.into_iter().map(str::to_owned).collect();
    args.push(safe_local_scp_arg(local_path));
    args.push(format_scp_destination(ssh_host, remote_path));
    args
}

fn run_single_scp(program: &str, local: &Path, host: &str, remote: &str) -> Result<(), String> {
    if !local.is_file() {
        return Err(text::cstr_text(text::DROP_SCP_NO_FILE).to_owned());
    }
    let ok = Command::new(program)
        .args(scp_upload_args(local, host, remote))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|st| st.success());
    ok.then_some(()).ok_or_else(|| format!("{} {host}:{remote}", text::cstr_text(text::DROP_SCP_ERROR)))
}

fn format_upload_done_msg(ssh_host: &str, remote_paths: &[String]) -> String {
    let prefix = text::cstr_text(text::DROP_SCP_DONE);
    if remote_paths.len() == 1 {
        format!("{prefix} {ssh_host}:{}", remote_paths[0])
    } else {
        format!("{prefix} {ssh_host}: {} ф.", remote_paths.len())
    }
}

fn upload_err(sid: usize, vte_addr: usize, footer_msg: String) -> UploadCompletion {
    UploadCompletion { sid, vte_addr, injected_text: None, footer_msg }
}

pub fn upload_files_with_program(
    program: &str,
    sid: usize,
    vte_addr: usize,
    ssh_host: &str,
    paths: &[PathBuf],
) -> UploadCompletion {
    if !is_valid_scp_host(ssh_host) {
        return upload_err(sid, vte_addr, format!("{} {ssh_host}", text::cstr_text(text::DROP_SCP_ERROR)));
    }
    let stamp = next_drop_stamp();
    let mut remote_paths = Vec::with_capacity(paths.len());
    for (idx, path) in paths.iter().enumerate() {
        let remote = remote_drop_path(path, stamp, idx);
        if let Err(msg) = run_single_scp(program, path, ssh_host, &remote) {
            return upload_err(sid, vte_addr, msg);
        }
        remote_paths.push(remote);
    }
    let injected = format_shell_paths(&remote_paths);
    let footer_msg = format_upload_done_msg(ssh_host, &remote_paths);
    UploadCompletion { sid, vte_addr, injected_text: (!injected.is_empty()).then_some(injected), footer_msg }
}

pub fn feed_vte_text(vte: *mut GtkWidget, payload: &str, grab_focus: bool) {
    if vte.is_null() || payload.is_empty() {
        return;
    }
    if let Ok(c_str) = CString::new(payload) {
        unsafe { vte_terminal_feed_child(vte, c_str.as_ptr(), payload.len() as isize) };
    }
    if grab_focus {
        unsafe { gtk_widget_grab_focus(vte) };
    }
}

fn collect_glib_strv(raw: *mut *mut c_char) -> Vec<String> {
    let mut out = Vec::new();
    if raw.is_null() {
        return out;
    }
    let mut idx = 0usize;
    unsafe {
        while !(*raw.add(idx)).is_null() {
            let item = CStr::from_ptr(*raw.add(idx)).to_string_lossy().into_owned();
            if !item.trim().is_empty() {
                out.push(item);
            }
            idx += 1;
        }
    }
    out
}

fn extract_selection_uris(selection_data: *const GtkSelectionData) -> Vec<String> {
    if selection_data.is_null() {
        return Vec::new();
    }
    let raw = unsafe { gtk_selection_data_get_uris(selection_data) };
    if raw.is_null() {
        return Vec::new();
    }
    let uris = collect_glib_strv(raw);
    unsafe { g_strfreev(raw) };
    uris
}

extern "C" fn on_scp_upload_idle(data: *mut c_void) -> c_int {
    if !data.is_null() {
        let done = unsafe { *Box::from_raw(data.cast::<UploadCompletion>()) };
        if let Some(hooks) = current_hooks() {
            (hooks.complete_upload)(done);
        }
    }
    0
}

pub fn schedule_upload_completion(done: UploadCompletion) {
    let boxed = Box::into_raw(Box::new(done));
    unsafe { g_idle_add(on_scp_upload_idle, boxed.cast()) };
}

fn spawn_remote_scp_upload(sid: usize, vte_addr: usize, host: String, paths: Vec<PathBuf>) {
    thread::spawn(move || {
        let done = upload_files_with_program("scp", sid, vte_addr, &host, &paths);
        schedule_upload_completion(done);
    });
}

fn start_remote_drop(vte: *mut GtkWidget, sid: usize, host: String, paths: Vec<PathBuf>) -> bool {
    if !is_valid_scp_host(&host) {
        notify_drop_status(&format!("{} {host}", text::cstr_text(text::DROP_SCP_ERROR)));
        return false;
    }
    let msg = format!("{} {host} ({} ф.)...", text::cstr_text(text::DROP_SCP_START), paths.len());
    notify_drop_status(&msg);
    unsafe { gtk_widget_grab_focus(vte) };
    spawn_remote_scp_upload(sid, vte as usize, host, paths);
    true
}

pub fn dispatch_dropped_paths(vte: *mut GtkWidget, paths: Vec<PathBuf>) -> bool {
    if vte.is_null() || paths.is_empty() {
        return false;
    }
    match current_hooks().map_or(DropTarget::Local, |h| (h.resolve_target)(vte)) {
        DropTarget::Ignore => false,
        DropTarget::Local => {
            let payload = format_local_drop_paths(&paths);
            feed_vte_text(vte, &payload, true);
            !payload.is_empty()
        }
        DropTarget::Remote { sid, host } => start_remote_drop(vte, sid, host, paths),
    }
}

extern "C" fn on_vte_drag_data_received(
    vte: *mut GtkWidget,
    context: *mut GdkDragContext,
    _x: c_int,
    _y: c_int,
    selection_data: *const GtkSelectionData,
    _info: c_uint,
    time: c_uint,
    _user_data: *mut c_void,
) {
    let uris = extract_selection_uris(selection_data);
    let paths = decode_uri_list(&uris);
    let ok = dispatch_dropped_paths(vte, paths);
    finish_drag(context, ok, time);
}

pub fn setup_vte_file_drop(vte: *mut GtkWidget) {
    if vte.is_null() {
        return;
    }
    enable_uri_drag_dest(vte);
    unsafe {
        g_signal_connect_data(
            vte,
            c"drag-data-received".as_ptr(),
            on_vte_drag_data_received as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::session::create_vte_widget;
    use crate::split_pane::{stop_pane_proc, wrap_vte_scroll, TabPanes};
    use std::fs;
    use std::time::{Duration, Instant};

    #[test]
    fn decodes_file_uris_with_cyrillic_spaces_and_quotes() {
        let cyr = "file:///home/user/%D0%A3%D1%87%D1%91%D0%B1%D0%B0/%D0%BE%D1%82%D1%87%D1%91%D1%82%201.pdf\r\n";
        assert_eq!(decode_file_uri(cyr), Some(PathBuf::from("/home/user/Учёба/отчёт 1.pdf")));
        let quoted = "file://localhost/tmp/log%20%27alpha%27%20%232.txt";
        assert_eq!(decode_file_uri(quoted), Some(PathBuf::from("/tmp/log 'alpha' #2.txt")));
        assert_eq!(decode_file_uri("https://example.com/a.png"), None);
        assert_eq!(decode_file_uri("file://remotehost/share/a.png"), None);
        assert_eq!(decode_file_uri("file:///tmp/bad%00null.txt"), None);
        let batch = vec!["# comment".to_owned(), cyr.to_owned(), quoted.to_owned()];
        assert_eq!(decode_uri_list(&batch).len(), 2);
    }

    #[test]
    fn escapes_posix_shell_paths_without_injection() {
        assert_eq!(posix_shell_escape("/tmp/build_2026-11/out.log"), "/tmp/build_2026-11/out.log");
        assert_eq!(posix_shell_escape("/tmp/мой файл.txt"), "'/tmp/мой файл.txt'");
        assert_eq!(posix_shell_escape("path/with 'quote'"), "'path/with '\\''quote'\\'''");
        assert_eq!(posix_shell_escape("/tmp/$(rm -rf /);`id`"), "'/tmp/$(rm -rf /);`id`'");
        assert_eq!(posix_shell_escape(""), "''");
        let paths = [PathBuf::from("/tmp/a.log"), PathBuf::from("/tmp/мой файл.txt")];
        assert_eq!(format_local_drop_paths(&paths), "/tmp/a.log '/tmp/мой файл.txt' ");
    }

    #[test]
    fn sanitizes_remote_tmp_filenames_and_guarantees_uniqueness() {
        assert_eq!(sanitize_remote_basename("Снимок экрана (1).png"), "Снимок_экрана_1.png");
        assert_eq!(sanitize_remote_basename("../../-rf; 'pwn'.tar.gz"), "rf_pwn.tar.gz");
        assert_eq!(sanitize_remote_basename("..."), "file");
        assert_eq!(sanitize_remote_basename(""), "file");
        let p1 = remote_drop_path(Path::new("/dir1/config.json"), 99, 0);
        let p2 = remote_drop_path(Path::new("/dir2/config.json"), 99, 1);
        assert_ne!(p1, p2);
        assert!(p1.starts_with("/tmp/akhterm_99_0_config.json"));
    }

    #[test]
    fn builds_scp_upload_args_for_hosts_ipv6_and_option_safety() {
        let args = scp_upload_args(Path::new("/tmp/a.png"), "devbox", "/tmp/akhterm_1_0_a.png");
        let expected = [
            "-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=6", "-o", "ClearAllForwardings=yes",
            "--", "/tmp/a.png", "devbox:/tmp/akhterm_1_0_a.png",
        ];
        assert_eq!(args, expected);
        assert_eq!(format_scp_destination("fe80::1", "/tmp/x"), "[fe80::1]:/tmp/x");
        assert_eq!(format_scp_destination("[2001:db8::1]", "/tmp/x"), "[2001:db8::1]:/tmp/x");
        assert_eq!(format_scp_destination("ubuntu@2001:db8::2", "/tmp/x"), "ubuntu@[2001:db8::2]:/tmp/x");
        let opt_inj = scp_upload_args(Path::new("-oProxyCommand=evil"), "devbox", "/tmp/x");
        assert_eq!((opt_inj[7].as_str(), opt_inj[8].as_str()), ("--", "./-oProxyCommand=evil"));
        assert!(!is_valid_scp_host("-oProxyCommand=evil") && !is_valid_scp_host("bad host"));
    }

    #[test]
    fn runs_mock_scp_upload_success_and_failure_branches() {
        let tmp = std::env::temp_dir().join(format!("akhterm_drop_u_{}.txt", std::process::id()));
        fs::write(&tmp, b"hello").unwrap();
        let ok = upload_files_with_program("true", 7, 1234, "gpu-box", std::slice::from_ref(&tmp));
        assert!(ok.injected_text.as_deref().unwrap_or("").starts_with("/tmp/akhterm_"));
        assert!(ok.footer_msg.contains("gpu-box"));
        let fail = upload_files_with_program("false", 7, 1234, "gpu-box", std::slice::from_ref(&tmp));
        assert!(fail.injected_text.is_none());
        assert!(fail.footer_msg.contains("Ошибка SCP"));
        let _ = fs::remove_file(&tmp);
    }

    fn wait_non_empty_file(path: &Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            pump_gtk_events();
            if let Ok(txt) = fs::read_to_string(path) {
                if !txt.is_empty() {
                    return txt;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        String::new()
    }

    fn verify_split_vte_drag_targets_and_pty_feed() {
        let out_file = std::env::temp_dir().join(format!("akhterm_pty_drop_{}", std::process::id()));
        let _ = fs::remove_file(&out_file);
        let cmd = format!("cat > '{}'", out_file.display());
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let (vte1, pid1) = create_vte_widget("/tmp", &["sh", "-c", &cmd]);
        let scroll1 = wrap_vte_scroll(vte1);
        let mut panes = TabPanes::new(scroll1, vte1);
        let (vte2, pid2) = create_vte_widget("/tmp", &["sh"]);
        panes.split(scroll1, vte1, vte2, pid2, "/tmp".to_owned(), GTK_ORIENTATION_HORIZONTAL);
        unsafe {
            gtk_container_add(win, panes.root);
            gtk_widget_show_all(win);
        }
        pump_gtk_events();
        assert!(widget_accepts_uri_drop(vte1) && widget_accepts_uri_drop(vte2));
        let dropped = vec![PathBuf::from("/tmp/файл 1.txt"), PathBuf::from("/tmp/it's.log")];
        assert!(dispatch_dropped_paths(vte1, dropped));
        feed_vte_text(vte1, "\x04", false);
        assert_eq!(wait_non_empty_file(&out_file), "'/tmp/файл 1.txt' '/tmp/it'\\''s.log' ");
        let _ = fs::remove_file(&out_file);
        panes.stop_all_extra();
        stop_pane_proc(pid1);
        unsafe { gtk_widget_destroy(win) };
        pump_gtk_events();
    }

    fn dummy_resolve(_vte: *mut GtkWidget) -> DropTarget {
        DropTarget::Local
    }

    fn dummy_status(_msg: &str) {}

    fn verify_closed_tab_idle_callback_safety() {
        static RECORDED: AtomicU64 = AtomicU64::new(0);
        fn record_complete(done: UploadCompletion) {
            let live = crate::find_live_drop_vte(&[], 0, done.sid, done.vte_addr as *mut GtkWidget);
            if live.is_none() {
                RECORDED.store(42, Ordering::SeqCst);
            }
        }
        register_file_drop_hooks(FileDropHooks {
            resolve_target: dummy_resolve,
            set_status: dummy_status,
            complete_upload: record_complete,
        });
        let destroyed_vte = unsafe { vte_terminal_new() };
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        unsafe {
            gtk_container_add(win, destroyed_vte);
            gtk_widget_destroy(win);
        }
        schedule_upload_completion(UploadCompletion {
            sid: 99,
            vte_addr: destroyed_vte as usize,
            injected_text: Some("/tmp/akhterm_99_0_a.log ".to_owned()),
            footer_msg: "done".to_owned(),
        });
        pump_gtk_events();
        assert_eq!(RECORDED.load(Ordering::SeqCst), 42);
        crate::install_file_drop_hooks();
    }

    #[test]
    fn headless_broadway_configures_drag_dest_and_guards_closed_vte() {
        with_headless_gtk(|| {
            verify_split_vte_drag_targets_and_pty_feed();
            verify_closed_tab_idle_callback_safety();
        });
    }

    #[test]
    fn maintainability_index_meets_file_drop_and_main_targets() {
        let (_, _, drop_mi) = crate::mi::compute_file_mi(include_str!("file_drop.rs"));
        let (_, _, main_mi) = crate::mi::compute_file_mi(include_str!("../main.rs"));
        assert!(drop_mi >= 58.0, "file_drop.rs MI too low: {drop_mi:.2}");
        assert!(main_mi >= 54.87, "main.rs MI regressed: {main_mi:.2}");
    }
}
