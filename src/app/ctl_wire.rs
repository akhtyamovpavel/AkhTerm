//! Control-plane GTK main-loop bridge for Unix socket requests (`status`, `focus`, `notify`, `send`, `open-doc`).

use std::ffi::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::app_state::{self, AppState};
use crate::ctl_protocol::{parse_pane_id_num, CtlRequest, CtlResponse, HookState, PaneStatusItem};
use crate::ctl_socket::{self, CtlServer};
use crate::ffi::*;
use crate::pane_registry::{PaneId, PaneRecord};
use crate::session::{TermSession, WorkState};
use crate::{pane_radar, platform, session_ops, topbar, ui_text};

static CTL_SERVER: OnceLock<Mutex<Option<CtlServer>>> = OnceLock::new();

fn server_slot() -> &'static Mutex<Option<CtlServer>> {
    CTL_SERVER.get_or_init(|| Mutex::new(None))
}

fn install_server_handle(server: CtlServer) {
    ctl_socket::set_active_socket_path(Some(server.path().to_path_buf()));
    if let Ok(mut guard) = server_slot().lock() {
        *guard = Some(server);
    }
    unsafe { g_timeout_add(250, on_ctl_drain_tick, ptr::null_mut()) };
}

pub(crate) fn start_server() {
    let wake = Arc::new(|| unsafe {
        g_idle_add(on_ctl_drain_idle, ptr::null_mut());
    });
    match CtlServer::bind(Some(wake)) {
        Ok(server) => install_server_handle(server),
        Err(err) => eprintln!("AkhTerm: control socket unavailable: {err}"),
    }
}

pub(crate) fn shutdown_server() {
    if let Ok(mut guard) = server_slot().lock() {
        let _ = guard.take();
    }
}

extern "C" fn on_ctl_drain_idle(_data: *mut c_void) -> c_int {
    drain_and_dispatch();
    0
}

extern "C" fn on_ctl_drain_tick(_data: *mut c_void) -> c_int {
    drain_and_dispatch();
    1
}

pub(crate) fn drain_and_dispatch() -> usize {
    let pending = match server_slot().lock() {
        Ok(guard) => guard.as_ref().map(CtlServer::drain_pending).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let count = pending.len();
    for item in pending {
        let resp = dispatch_request(item.request.clone());
        item.respond(resp);
    }
    count
}

pub(crate) fn dispatch_request(req: CtlRequest) -> CtlResponse {
    match req {
        CtlRequest::Status => handle_status(),
        CtlRequest::Focus { pane_id } => handle_focus(&pane_id),
        CtlRequest::Notify { pane_id, state, title, message, desktop } => {
            handle_notify(&pane_id, state, title, message, desktop)
        }
        CtlRequest::Send { pane_id, text, enter } => handle_send(&pane_id, &text, enter),
        CtlRequest::OpenDoc { pane_id, path } => handle_open_doc(pane_id.as_deref(), &path),
    }
}

fn work_state_label(state: WorkState) -> &'static str {
    match state {
        WorkState::Idle => "idle",
        WorkState::Working => "working",
        WorkState::Ready => "done",
        WorkState::NeedsApproval => "waiting",
    }
}

fn build_pane_status(s: &TermSession, p: &PaneRecord, active_tab: bool, now: Instant) -> PaneStatusItem {
    let hook = p.active_hook_state(now);
    let state = hook.map_or_else(|| work_state_label(p.state), |h| h.state.as_str()).to_owned();
    let cwd = if p.cwd.is_empty() { s.cwd.clone() } else { p.cwd.clone() };
    let ssh_host = (!p.proc_info.ssh_host.is_empty()).then(|| p.proc_info.ssh_host.clone());
    PaneStatusItem {
        pane_id: p.id.raw().to_string(), tab_id: s.id, tab_title: s.title.clone(), active_tab,
        focused_pane: active_tab && s.active_vte() == p.vte, agent: p.effective_agent(), state,
        hook_state: hook.map(|h| h.state.as_str().to_owned()), hook_title: hook.and_then(|h| h.title.clone()),
        hook_message: hook.and_then(|h| h.message.clone()), cwd, pid: p.child_pid, ssh_host, unread: p.unread,
    }
}

fn collect_session_panes(app: &AppState, now: Instant) -> Vec<PaneStatusItem> {
    let mut panes = Vec::new();
    for s in &app.sessions {
        let active_tab = s.id == app.active_id;
        panes.extend(s.pane_registry.panes().iter().map(|p| build_pane_status(s, p, active_tab, now)));
    }
    panes
}

fn collect_status(app: &AppState) -> CtlResponse {
    let panes = collect_session_panes(app, Instant::now());
    let socket = ctl_socket::current_socket_path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    CtlResponse::Status {
        version: env!("CARGO_PKG_VERSION").to_owned(), pid: std::process::id(),
        socket, active_tab: app.active_id, panes,
    }
}

fn handle_status() -> CtlResponse {
    app_state::with_app(collect_status).unwrap_or_else(|| CtlResponse::Error {
        message: "AkhTerm UI is not initialized".to_owned(),
    })
}

fn handle_focus(pane_id: &str) -> CtlResponse {
    match session_ops::focus_pane_by_id(pane_id) {
        Ok(()) => CtlResponse::Ok { message: Some(format!("focused pane {pane_id}")) },
        Err(message) => CtlResponse::Error { message },
    }
}

fn default_notify_summary(state: HookState) -> String {
    match state {
        HookState::Waiting => ui_text::NOTIFY_APPROVAL_TITLE.to_string_lossy().into_owned(),
        HookState::Error => ui_text::NOTIFY_ERROR_TITLE.as_str().to_owned(),
        HookState::Done | HookState::Working => ui_text::NOTIFY_READY_TITLE.to_string_lossy().into_owned(),
    }
}

fn maybe_capture_hook_checkpoint(s: &TermSession, pid: PaneId, state: HookState, message: Option<&str>) {
    let Some(pane) = s.pane_registry.get(pid) else { return };
    let is_local = s.remote().is_none() && pane.proc_info.ssh_host.is_empty() && !s.is_dynamic_ssh();
    let cwd = if pane.cwd.is_empty() { s.cwd.as_str() } else { pane.cwd.as_str() };
    if crate::git_checkpoints::should_trigger_hook_checkpoint(state, message, is_local, cwd) {
        let label = message.filter(|m| !m.trim().is_empty()).unwrap_or("pre-tool").to_owned();
        crate::git_checkpoints::schedule_hook_checkpoint(pid.raw(), cwd.to_owned(), label);
    }
}

fn apply_notify_to_app(
    app: &mut AppState,
    raw_id: u64,
    state: HookState,
    title: &Option<String>,
    message: &Option<String>,
    now: Instant,
) -> Option<(String, *mut GtkWidget, bool)> {
    let win_active = app.win.is_null() || unsafe { gtk_window_is_active(app.win) } != 0;
    let (active_id, win, pid) = (app.active_id, app.win, PaneId::new(raw_id));
    for s in &mut app.sessions {
        let Some(vte) = s.pane_registry.get(pid).map(|p| p.vte) else { continue };
        let is_focused = win_active && s.id == active_id && s.active_vte() == vte;
        if pane_radar::apply_pane_hook_notification(s, pid, state, title.clone(), message.clone(), is_focused, now) {
            maybe_capture_hook_checkpoint(s, pid, state, message.as_deref());
            return Some((s.title.clone(), win, win_active));
        }
    }
    None
}

fn handle_notify(
    pane_id: &str,
    state: HookState,
    title: Option<String>,
    message: Option<String>,
    desktop: bool,
) -> CtlResponse {
    let Some(raw_id) = parse_pane_id_num(pane_id) else {
        return CtlResponse::Error { message: format!("invalid pane_id '{pane_id}'") };
    };
    let applied = app_state::with_app_mut(|app| {
        apply_notify_to_app(app, raw_id, state, &title, &message, Instant::now())
    })
    .flatten();
    let Some((tab_title, win, win_active)) = applied else {
        return CtlResponse::Error { message: format!("pane not found: {pane_id}") };
    };
    topbar::refresh_ui();
    if desktop {
        emit_hook_desktop_notify(state, title, message, &tab_title, raw_id, win, win_active);
    }
    CtlResponse::Ok { message: Some(format!("pane {raw_id} state {}", state.as_str())) }
}

fn emit_hook_desktop_notify(
    state: HookState,
    title: Option<String>,
    message: Option<String>,
    tab_title: &str,
    raw_id: u64,
    win: *mut GtkWidget,
    win_active: bool,
) {
    if !win.is_null() && !win_active {
        unsafe { gtk_window_set_urgency_hint(win, 1) };
    }
    let summary = title.filter(|s| !s.is_empty()).unwrap_or_else(|| default_notify_summary(state));
    let body = message.filter(|s| !s.is_empty()).unwrap_or_else(|| tab_title.to_owned());
    platform::spawn_clickable_notification(summary, body, Some(raw_id.to_string()));
}

fn find_pane_vte(pane_id: &str) -> Result<*mut GtkWidget, String> {
    let raw_id = parse_pane_id_num(pane_id)
        .ok_or_else(|| format!("invalid pane_id '{pane_id}'"))?;
    let target = PaneId::new(raw_id);
    app_state::with_app(|app| {
        app.sessions
            .iter()
            .find_map(|s| s.pane_registry.get(target).map(|p| p.vte))
    })
    .flatten()
    .ok_or_else(|| format!("pane not found: {pane_id}"))
}

fn handle_send(pane_id: &str, text: &str, enter: bool) -> CtlResponse {
    let vte = match find_pane_vte(pane_id) {
        Ok(v) if !v.is_null() => v,
        Ok(_) => return CtlResponse::Error { message: format!("pane {pane_id} has no terminal") },
        Err(message) => return CtlResponse::Error { message },
    };
    let payload = if enter { format!("{text}\r") } else { text.to_owned() };
    let bytes = payload.as_bytes();
    unsafe {
        vte_terminal_feed_child(vte, bytes.as_ptr().cast::<c_char>(), bytes.len() as isize);
    }
    CtlResponse::Ok { message: Some(format!("sent {} bytes to pane {pane_id}", bytes.len())) }
}

fn open_doc_for_pane(pid: &str, path: &Path) -> Result<(), String> {
    let vte = find_pane_vte(pid)?;
    let _ = session_ops::focus_pane_by_id(pid);
    crate::markdown::open_local(path, crate::markdown::for_vte(vte));
    Ok(())
}

fn handle_open_doc(pane_id: Option<&str>, raw_path: &str) -> CtlResponse {
    let path = Path::new(raw_path);
    if !path.is_file() {
        return CtlResponse::Error { message: format!("file not found: {raw_path}") };
    }
    if let Some(pid) = pane_id {
        if let Err(message) = open_doc_for_pane(pid, path) {
            return CtlResponse::Error { message };
        }
    } else {
        crate::markdown::open(path);
    }
    CtlResponse::Ok { message: Some(format!("opened {raw_path}")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::app_tests::{cleanup_test_window, install_test_window};
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::tabs::Tab;
    use std::time::Duration;

    fn read_proc_env_var(pid: i32, key: &str) -> Option<String> {
        let bytes = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        let prefix = format!("{key}=");
        for item in bytes.split(|&b| b == 0) {
            if let Ok(s) = std::str::from_utf8(item) {
                if let Some(val) = s.strip_prefix(&prefix) {
                    return Some(val.to_owned());
                }
            }
        }
        None
    }

    fn spawn_three_test_panes() {
        let _s1 = session_ops::add_tab("Tab1", "", "/tmp", crate::session::SessionCategory::Project, &["sh"], Tab::local("shell"));
        let s2 = session_ops::add_tab("Tab2", "", "/tmp", crate::session::SessionCategory::Project, &["sh"], Tab::local("shell"));
        session_ops::select_session(s2);
        session_ops::split_pane_in_session(Some(s2), ptr::null_mut(), GTK_ORIENTATION_HORIZONTAL);
        pump_gtk_events();
    }

    fn verify_pty_env_and_status(sock_path: &Path) -> (u64, u64, u64) {
        spawn_three_test_panes();
        let CtlResponse::Status { panes, .. } = dispatch_request(CtlRequest::Status) else {
            panic!("expected Status response");
        };
        assert_eq!(panes.len(), 3);
        let ids: Vec<u64> = panes.iter().map(|p| p.pane_id.parse::<u64>().unwrap()).collect();
        assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2]);
        assert_eq!(read_proc_env_var(panes[0].pid, "AKHTERM_PANE_ID").as_deref(), Some(panes[0].pane_id.as_str()));
        assert_eq!(read_proc_env_var(panes[0].pid, "AKHTERM_SOCKET").as_deref(), Some(sock_path.to_string_lossy().as_ref()));
        (ids[0], ids[1], ids[2])
    }

    fn verify_notify_updates_ui(p3: u64) {
        let req = CtlRequest::Notify {
            pane_id: p3.to_string(), state: HookState::Waiting,
            title: Some("Claude Code".into()), message: Some("Allow cargo test?".into()), desktop: false,
        };
        assert!(matches!(dispatch_request(req), CtlResponse::Ok { .. }));
        app_state::with_app(|app| {
            let cards = crate::swarm_view::collect_swarm_cards_from_sessions(&app.sessions, Instant::now());
            assert!(cards.iter().any(|c| c.approval_line.as_deref() == Some("Allow cargo test?")));
            let headers = pane_radar::collect_pane_header_updates(&app.sessions, app.active_id);
            assert!(headers.iter().any(|(_, st, _)| st.status_dot == Some("pane-dot-waiting")));
        });
    }

    fn verify_focus_and_send(p1: u64, p3: u64) {
        assert!(matches!(dispatch_request(CtlRequest::Focus { pane_id: p1.to_string() }), CtlResponse::Ok { .. }));
        assert_eq!(app_state::active_session_id(), 1);
        assert!(matches!(dispatch_request(CtlRequest::Focus { pane_id: p3.to_string() }), CtlResponse::Ok { .. }));
        assert_eq!(app_state::active_session_id(), 2);
        app_state::with_app(|app| {
            let s2 = &app.sessions[1];
            assert_eq!(s2.active_vte(), s2.pane_registry.get(PaneId::new(p3)).unwrap().vte);
        });
        let sent = dispatch_request(CtlRequest::Send { pane_id: p3.to_string(), text: "echo hi".into(), enter: true });
        assert!(matches!(sent, CtlResponse::Ok { .. }));
    }

    fn verify_open_doc_dispatch(p3: u64) {
        let doc_path = std::env::temp_dir().join(format!("akhterm-ctl-doc-{}.md", std::process::id()));
        std::fs::write(&doc_path, "# Demo\n").unwrap();
        let req = CtlRequest::OpenDoc { pane_id: Some(p3.to_string()), path: doc_path.to_string_lossy().into_owned() };
        assert!(matches!(dispatch_request(req), CtlResponse::Ok { .. }));
        let missing = dispatch_request(CtlRequest::OpenDoc { pane_id: None, path: "/nonexistent/akhterm.md".into() });
        assert!(matches!(missing, CtlResponse::Error { .. }));
        let _ = std::fs::remove_file(&doc_path);
    }

    fn run_broadway_ctl_checks() {
        with_headless_gtk(|| {
            let sock_path = std::env::temp_dir().join(format!("akhterm-bw-ctl-{}.sock", std::process::id()));
            ctl_socket::set_active_socket_path(Some(sock_path.clone()));
            let (win, _) = install_test_window();
            unsafe { gtk_widget_show_all(win) };
            let (p1, _p2, p3) = verify_pty_env_and_status(&sock_path);
            verify_notify_updates_ui(p3);
            verify_focus_and_send(p1, p3);
            verify_open_doc_dispatch(p3);
            let sids: Vec<usize> = app_state::with_app(|a| a.sessions.iter().map(|s| s.id).collect()).unwrap_or_default();
            cleanup_test_window(win, &sids);
            ctl_socket::set_active_socket_path(None);
        });
    }

    #[test]
    fn headless_broadway_ctl_focus_notify_send_status_and_open_doc() {
        if std::env::var_os("AKHTERM_CTL_BWAY_CHILD").is_some() {
            run_broadway_ctl_checks();
            return;
        }
        let exe = std::env::current_exe().expect("test binary path");
        let test_name = "app::ctl_wire::tests::headless_broadway_ctl_focus_notify_send_status_and_open_doc";
        let status = std::process::Command::new(exe)
            .args(["--exact", test_name])
            .env("AKHTERM_CTL_BWAY_CHILD", "1")
            .status()
            .expect("spawn broadway ctl test");
        assert!(status.success());
    }

    fn sample_agent_pane() -> PaneRecord {
        let mut pane = PaneRecord::new(PaneId::new(9), ptr::null_mut(), ptr::null_mut(), 42, "/repo".into());
        pane.proc_info.foreground_shell = false;
        pane.proc_info.agent = "Claude".into();
        pane
    }

    #[test]
    fn hook_state_output_grace_and_error_flag() {
        let now = Instant::now();
        let mut pane = sample_agent_pane();
        pane.apply_hook_state(HookState::Working, Some("Claude".into()), None, false, now);
        assert!(pane_radar::settle_pane_with_text(&mut pane, "done\n", now + Duration::from_secs(10), false, false).is_none());
        pane.apply_hook_state(HookState::Waiting, Some("Claude".into()), Some("Approve?".into()), false, now);
        pane.mark_output(now + Duration::from_secs(1), false);
        assert_eq!(pane.state, WorkState::NeedsApproval);
        pane.apply_hook_state(HookState::Error, Some("Claude".into()), Some("Build failed".into()), false, now);
        assert!(pane.has_hook_error() && pane.last_exit_code == Some(1));
    }

    #[test]
    fn hook_state_ttl_expiry_and_agent_exit_cleanup() {
        let now = Instant::now();
        let mut pane = sample_agent_pane();
        pane.apply_hook_state(HookState::Error, Some("Claude".into()), Some("Fail".into()), false, now);
        let mut shell_proc = crate::proc_radar::ProcInfo::default();
        shell_proc.foreground_shell = true;
        pane.sync_agent(&mut shell_proc);
        assert!(pane.hook_state.is_none() && pane.state == WorkState::Idle);
        pane.apply_hook_state(HookState::Working, None, None, false, now);
        assert!(pane.expire_hook_state(now + Duration::from_secs(1000)));
        assert_eq!(pane.state, WorkState::Idle);
    }

    #[test]
    fn ctl_modules_meet_maintainability_index_gates() {
        let files = [
            ("sys/json_lite.rs", include_str!("../sys/json_lite.rs")),
            ("sys/ctl_protocol.rs", include_str!("../sys/ctl_protocol.rs")),
            ("sys/ctl_socket.rs", include_str!("../sys/ctl_socket.rs")),
            ("sys/agent_hooks.rs", include_str!("../sys/agent_hooks.rs")),
            ("sys/ctl_cli.rs", include_str!("../sys/ctl_cli.rs")),
            ("app/ctl_wire.rs", include_str!("ctl_wire.rs")),
        ];
        for (name, src) in files {
            let (_, _, score) = crate::mi::compute_file_mi(src);
            assert!(score >= 60.0, "{name} MI {score:.2} < 60.0");
        }
    }
}
