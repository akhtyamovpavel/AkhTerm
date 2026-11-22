//! GTK main-loop bridge for AkhTerm Remote HTTP/SSE server, tail capture, and state alerts.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::app_state::{self, AppState};
use crate::approval::{extract_approval_snippet, read_vte_visible_text};
use crate::ctl_protocol::parse_pane_id_num;
use crate::ffi::*;
use crate::git_wt::detect_git_repo;
use crate::pane_registry::{PaneId, PaneRecord};
use crate::remote::auth::{default_devices_path, PairedDevice};
use crate::remote::http::{RemoteBridgeRequest, RemoteBridgeResponse};
use crate::remote::protocol::{
    encode_alert_json, encode_status_json, encode_tail_json, AlertEvent, ApprovalKind,
    ApproveAction, PaneTailResponse, RemoteKey, RemotePaneSummary, RemoteStatusResponse,
    MAX_TAIL_LINES,
};
use crate::remote::server::RemoteServer;
use crate::remote::settings_io::RemoteSettings;
use crate::remote::sse::SseHub;
use crate::session::{TermSession, WorkState};
use crate::swarm_view::extract_preview_lines;
use crate::{session_ops, topbar};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRunningInfo {
    pub bind_ip: String,
    pub port: u16,
    pub host_name: String,
    pub connected_clients: usize,
    pub paired_count: usize,
}

struct WatcherState {
    last_snapshot_json: String,
    last_pane_states: HashMap<String, String>,
    last_tail_hashes: HashMap<String, String>,
    tail_seqs: HashMap<String, u64>,
    next_event_seq: u64,
    last_heartbeat: Instant,
    mute_when_focused: bool,
    timer_installed: bool,
}

impl Default for WatcherState {
    fn default() -> Self {
        Self {
            last_snapshot_json: String::new(),
            last_pane_states: HashMap::new(),
            last_tail_hashes: HashMap::new(),
            tail_seqs: HashMap::new(),
            next_event_seq: 1,
            last_heartbeat: Instant::now(),
            mute_when_focused: true,
            timer_installed: false,
        }
    }
}

static REMOTE_SERVER: OnceLock<Mutex<Option<RemoteServer>>> = OnceLock::new();
static WATCHER: OnceLock<Mutex<WatcherState>> = OnceLock::new();

fn server_slot() -> &'static Mutex<Option<RemoteServer>> {
    REMOTE_SERVER.get_or_init(|| Mutex::new(None))
}

fn watcher_slot() -> &'static Mutex<WatcherState> {
    WATCHER.get_or_init(|| Mutex::new(WatcherState::default()))
}

fn matches_running_binding(settings: &RemoteSettings) -> bool {
    server_slot().lock().ok().and_then(|g| {
        g.as_ref().map(|s| s.bind_ip() == settings.bind_ip && s.port() == settings.port)
    }) == Some(true)
}

pub fn apply_settings(settings: &RemoteSettings) -> Result<(), String> {
    let norm = settings.normalized();
    if let Ok(mut w) = watcher_slot().lock() {
        w.mute_when_focused = norm.mute_when_focused;
    }
    if !norm.enabled {
        stop_server();
        return Ok(());
    }
    if matches_running_binding(&norm) {
        sync_topbar_widget();
        return Ok(());
    }
    stop_server();
    start_server(&norm)
}

fn reset_watcher_for_start() -> bool {
    watcher_slot().lock().map_or(false, |mut w| {
        let need = !w.timer_installed;
        w.timer_installed = true;
        w.last_snapshot_json.clear();
        w.last_pane_states.clear();
        w.last_tail_hashes.clear();
        w.last_heartbeat = Instant::now();
        need
    })
}

fn start_server(settings: &RemoteSettings) -> Result<(), String> {
    let wake = Arc::new(|| unsafe { g_idle_add(on_remote_drain_idle, ptr::null_mut()); });
    let server = RemoteServer::start(
        settings.bind_ip.clone(), settings.port, default_devices_path(), Some(wake),
    ).map_err(|e| e.to_string())?;
    if let Ok(mut slot) = server_slot().lock() {
        *slot = Some(server);
    }
    if reset_watcher_for_start() {
        unsafe { g_timeout_add(250, on_remote_drain_tick, ptr::null_mut()) };
    }
    sync_topbar_widget();
    Ok(())
}

pub fn stop_server() {
    if let Ok(mut slot) = server_slot().lock() {
        let _ = slot.take();
    }
    if let Ok(mut w) = watcher_slot().lock() {
        w.last_snapshot_json.clear();
        w.last_pane_states.clear();
        w.last_tail_hashes.clear();
    }
    sync_topbar_widget();
}

pub fn is_running() -> bool {
    server_slot().lock().ok().is_some_and(|g| g.is_some())
}

pub fn running_info() -> Option<RemoteRunningInfo> {
    let guard = server_slot().lock().ok()?;
    let srv = guard.as_ref()?;
    Some(RemoteRunningInfo {
        bind_ip: srv.bind_ip().to_owned(), port: srv.port(), host_name: srv.host_name().to_owned(),
        connected_clients: srv.connected_clients(), paired_count: srv.list_devices().len(),
    })
}

pub fn ensure_pairing_url() -> Result<(String, u64), String> {
    let guard = server_slot().lock().map_err(|_| "lock poisoned".to_owned())?;
    let srv = guard.as_ref().ok_or_else(|| "remote server is stopped".to_owned())?;
    srv.ensure_pairing_url().map_err(|e| e.to_string())
}

pub fn rotate_pairing_url() -> Result<(String, u64), String> {
    let guard = server_slot().lock().map_err(|_| "lock poisoned".to_owned())?;
    let srv = guard.as_ref().ok_or_else(|| "remote server is stopped".to_owned())?;
    srv.rotate_pairing_url().map_err(|e| e.to_string())
}

pub fn list_devices() -> Vec<PairedDevice> {
    if let Some(devs) = server_slot().lock().ok().and_then(|g| g.as_ref().map(RemoteServer::list_devices)) {
        return devs;
    }
    crate::remote::auth::RemoteAuthStore::load(default_devices_path()).devices().to_vec()
}

pub fn revoke_device(device_id: &str) -> bool {
    let removed = match server_slot().lock() {
        Ok(guard) => match guard.as_ref() {
            Some(srv) => srv.revoke_device(device_id),
            None => crate::remote::auth::RemoteAuthStore::load(default_devices_path()).revoke_device(device_id),
        },
        Err(_) => false,
    };
    sync_topbar_widget();
    removed
}

fn sync_topbar_widget() {
    if let Some(m) = app_state::with_app(|app| app.remote_menu.clone()) {
        m.sync_state();
    }
}

extern "C" fn on_remote_drain_idle(_data: *mut c_void) -> c_int {
    drain_requests();
    0
}

extern "C" fn on_remote_drain_tick(_data: *mut c_void) -> c_int {
    drain_requests();
    1
}

pub fn drain_requests() -> usize {
    let pending = server_slot().lock().ok().and_then(|g| g.as_ref().map(RemoteServer::drain_pending)).unwrap_or_default();
    let count = pending.len();
    for item in pending {
        let resp = dispatch_bridge(item.request.clone());
        item.respond(resp);
    }
    if count > 0 {
        sync_topbar_widget();
    }
    count
}

fn active_sse_hub() -> Option<(Arc<SseHub>, Vec<String>)> {
    let guard = server_slot().lock().ok()?;
    let srv = guard.as_ref()?;
    Some((Arc::clone(srv.sse()), srv.sse().watched_pane_ids()))
}

pub fn tick_broadcast() {
    if !is_running() {
        return;
    }
    drain_requests();
    let Some(status) = app_state::with_app(collect_remote_status) else { return };
    let Some((sse, watched)) = active_sse_hub() else { return };
    broadcast_status_and_alerts(&sse, &status);
    broadcast_watched_tails(&sse, &watched);
    sync_topbar_widget();
}

fn broadcast_status_and_alerts(sse: &Arc<SseHub>, status: &RemoteStatusResponse) {
    let snap_json = encode_status_json(status);
    let alerts = {
        let Ok(mut w) = watcher_slot().lock() else { return };
        let alerts = collect_transition_alerts(&mut w, status);
        if w.last_snapshot_json != snap_json {
            w.last_snapshot_json = snap_json.clone();
            sse.broadcast_all("snapshot", &snap_json);
        }
        if w.last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            w.last_heartbeat = Instant::now();
            sse.send_heartbeat();
        }
        alerts
    };
    for alert in alerts {
        sse.broadcast_all("alert", &encode_alert_json(&alert));
    }
}

fn tail_changed(pid: &str, tail_json: &str) -> bool {
    watcher_slot().lock().map_or(false, |mut w| {
        if w.last_tail_hashes.get(pid).map(String::as_str) == Some(tail_json) {
            return false;
        }
        w.last_tail_hashes.insert(pid.to_owned(), tail_json.to_owned());
        true
    })
}

fn broadcast_watched_tails(sse: &Arc<SseHub>, watched: &[String]) {
    for pid in watched {
        if let Ok(tail) = capture_pane_tail(pid, 120) {
            let tail_json = encode_tail_json(&tail);
            if tail_changed(pid, &tail_json) {
                sse.broadcast_pane_tail(pid, &tail_json);
            }
        }
    }
}

fn collect_transition_alerts(w: &mut WatcherState, status: &RemoteStatusResponse) -> Vec<AlertEvent> {
    let mut alerts = Vec::new();
    let mut next_map = HashMap::with_capacity(status.panes.len());
    for pane in &status.panes {
        let prev = w.last_pane_states.get(&pane.pane_id).map(String::as_str);
        let entered = prev.is_some() && prev != Some(pane.state.as_str());
        if entered && matches!(pane.state.as_str(), "waiting" | "done" | "error") {
            let seq = w.next_event_seq;
            w.next_event_seq += 1;
            alerts.push(build_alert_event(seq, pane, status.desktop_focused, status.mute_when_focused));
        }
        next_map.insert(pane.pane_id.clone(), pane.state.clone());
    }
    w.last_pane_states = next_map;
    alerts
}

fn build_alert_event(seq: u64, pane: &RemotePaneSummary, desktop_focused: bool, mute: bool) -> AlertEvent {
    let word = match pane.state.as_str() { "waiting" => "Needs Approval", "error" => "Error", _ => "Done" };
    let body = pane.approval_snippet.clone().or_else(|| pane.preview_lines.last().cloned())
        .unwrap_or_else(|| pane.tab_title.clone());
    AlertEvent {
        event_id: format!("evt-{seq}"), pane_id: pane.pane_id.clone(), tab_title: pane.tab_title.clone(),
        agent: pane.agent.clone(), repo_name: pane.repo_name.clone(), state: pane.state.clone(),
        title: format!("{} · {word}", pane.agent), body, approval_kind: pane.approval_kind,
        desktop_focused, should_notify: !(mute && desktop_focused),
    }
}

fn dispatch_bridge(req: RemoteBridgeRequest) -> RemoteBridgeResponse {
    match req {
        RemoteBridgeRequest::Status => handle_status_req(),
        RemoteBridgeRequest::Tail { pane_id, lines } => handle_tail_req(&pane_id, lines),
        RemoteBridgeRequest::Approve { pane_id, action } => handle_approve(&pane_id, action),
        RemoteBridgeRequest::Send { pane_id, text, enter } => handle_send(&pane_id, &text, enter),
        RemoteBridgeRequest::Key { pane_id, key } => handle_key(&pane_id, key),
        RemoteBridgeRequest::Focus { pane_id } => handle_focus_req(&pane_id),
    }
}

fn handle_status_req() -> RemoteBridgeResponse {
    app_state::with_app(collect_remote_status)
        .map(RemoteBridgeResponse::Status)
        .unwrap_or_else(|| RemoteBridgeResponse::Error("AkhTerm UI is not initialized".into()))
}

fn handle_tail_req(pane_id: &str, lines: usize) -> RemoteBridgeResponse {
    match capture_pane_tail(pane_id, lines) {
        Ok(tail) => RemoteBridgeResponse::Tail(tail),
        Err(m) => RemoteBridgeResponse::NotFound(m),
    }
}

fn handle_focus_req(pane_id: &str) -> RemoteBridgeResponse {
    match session_ops::focus_pane_by_id(pane_id) {
        Ok(()) => RemoteBridgeResponse::OkJson(format!("{{\"ok\":true,\"pane_id\":\"{pane_id}\"}}")),
        Err(m) => RemoteBridgeResponse::NotFound(m),
    }
}

fn state_label(s: &TermSession, p: &PaneRecord, now: Instant) -> String {
    if let Some(hook) = p.active_hook_state(now) {
        return hook.state.as_str().to_owned();
    }
    match p.state {
        WorkState::NeedsApproval => "waiting".into(),
        WorkState::Working => "working".into(),
        WorkState::Ready => "done".into(),
        WorkState::Idle => if s.state == WorkState::NeedsApproval { "waiting" } else { "idle" }.into(),
    }
}

fn resolve_repo(cwd: &str, fallback: &str) -> (String, String, bool) {
    let path = Path::new(cwd);
    if let Some((root, info)) = detect_git_repo(path) {
        let name = root.file_name().and_then(|n| n.to_str()).unwrap_or(fallback);
        return (name.to_owned(), info.badge(), info.is_linked_worktree);
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(fallback);
    (name.to_owned(), String::new(), false)
}

fn resolve_pane_approval(p: &PaneRecord, state: &str, visible: &str) -> (Option<String>, Option<ApprovalKind>) {
    if state != "waiting" {
        return (None, None);
    }
    let snippet = p.approval_snippet.clone().or_else(|| extract_approval_snippet(visible));
    let kind = Some(ApprovalKind::detect(snippet.as_deref()));
    (snippet, kind)
}

fn effective_pane_agent(p: &PaneRecord) -> String {
    let a = p.effective_agent();
    if a.is_empty() { "shell".to_owned() } else { a }
}

fn build_pane_summary(s: &TermSession, p: &PaneRecord, active_tab: bool, now: Instant) -> RemotePaneSummary {
    let cwd = if p.cwd.is_empty() { s.cwd.as_str() } else { p.cwd.as_str() };
    let (repo_name, branch_badge, is_worktree) = resolve_repo(cwd, &s.title);
    let visible = read_vte_visible_text(p.vte);
    let state = state_label(s, p, now);
    let (approval_snippet, approval_kind) = resolve_pane_approval(p, &state, &visible);
    RemotePaneSummary {
        pane_id: p.id.raw().to_string(), tab_id: s.id, tab_title: s.title.clone(), active_tab,
        focused_pane: active_tab && s.active_vte() == p.vte, agent: effective_pane_agent(p),
        repo_name, branch_badge, is_worktree,
        ssh_host: (!p.proc_info.ssh_host.is_empty()).then(|| p.proc_info.ssh_host.clone()),
        state, elapsed_secs: p.live_turn_secs(now).or(p.last_turn_secs),
        context_badge: p.telemetry.snapshot().compact_context_badge(), unread: p.unread,
        preview_lines: extract_preview_lines(&visible, 3), approval_snippet, approval_kind,
    }
}

fn current_host_name() -> String {
    server_slot().lock().ok().and_then(|g| g.as_ref().map(|s| s.host_name().to_owned()))
        .unwrap_or_else(crate::remote::auth::local_hostname)
}

pub fn collect_remote_status(app: &AppState) -> RemoteStatusResponse {
    let now = Instant::now();
    let desktop_focused = !app.win.is_null() && unsafe { gtk_window_is_active(app.win) } != 0;
    let mute_when_focused = watcher_slot().lock().map_or(true, |w| w.mute_when_focused);
    let mut panes = Vec::new();
    for s in &app.sessions {
        let active = s.id == app.active_id;
        panes.extend(s.pane_registry.panes().iter().map(|p| build_pane_summary(s, p, active, now)));
    }
    RemoteStatusResponse {
        version: env!("CARGO_PKG_VERSION").to_owned(), host_name: current_host_name(),
        desktop_focused, mute_when_focused, active_tab_id: app.active_id, panes,
    }
}

pub fn tail_lines_from_text(raw: &str, max_lines: usize) -> Vec<String> {
    let cap = max_lines.clamp(1, MAX_TAIL_LINES);
    let mut lines: Vec<String> = raw.lines().map(|l| l.trim_end().to_owned()).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.len() > cap {
        lines.drain(0..(lines.len() - cap));
    }
    lines
}

fn next_tail_seq(pane_id: &str) -> u64 {
    watcher_slot().lock().map_or(1, |mut w| {
        let entry = w.tail_seqs.entry(pane_id.to_owned()).or_insert(0);
        *entry += 1;
        *entry
    })
}

fn lookup_pane_tail(target: PaneId, max_lines: usize) -> Option<(String, Option<String>, Option<ApprovalKind>, Vec<String>)> {
    app_state::with_app(|app| {
        let now = Instant::now();
        app.sessions.iter().find_map(|s| {
            let p = s.pane_registry.get(target)?;
            let visible = read_vte_visible_text(p.vte);
            let state = state_label(s, p, now);
            let (snippet, kind) = resolve_pane_approval(p, &state, &visible);
            Some((state, snippet, kind, tail_lines_from_text(&visible, max_lines)))
        })
    })
    .flatten()
}

pub fn capture_pane_tail(pane_id: &str, max_lines: usize) -> Result<PaneTailResponse, String> {
    let raw_id = parse_pane_id_num(pane_id).ok_or_else(|| format!("invalid pane_id '{pane_id}'"))?;
    let Some((state, approval_snippet, approval_kind, lines)) = lookup_pane_tail(PaneId::new(raw_id), max_lines) else {
        return Err(format!("pane not found: {pane_id}"));
    };
    Ok(PaneTailResponse { pane_id: raw_id.to_string(), seq: next_tail_seq(pane_id), state, approval_snippet, approval_kind, lines })
}

fn find_pane_vte_and_kind(pane_id: &str) -> Result<(*mut GtkWidget, Option<ApprovalKind>), String> {
    let raw_id = parse_pane_id_num(pane_id).ok_or_else(|| format!("invalid pane_id '{pane_id}'"))?;
    let target = PaneId::new(raw_id);
    app_state::with_app(|app| {
        app.sessions.iter().find_map(|s| {
            let p = s.pane_registry.get(target)?;
            let visible = read_vte_visible_text(p.vte);
            let snippet = p.approval_snippet.clone().or_else(|| extract_approval_snippet(&visible));
            Some((p.vte, Some(ApprovalKind::detect(snippet.as_deref()))))
        })
    })
    .flatten()
    .ok_or_else(|| format!("pane not found: {pane_id}"))
}

fn feed_bytes(vte: *mut GtkWidget, bytes: &[u8]) {
    if !vte.is_null() && !bytes.is_empty() {
        unsafe { vte_terminal_feed_child(vte, bytes.as_ptr().cast::<c_char>(), bytes.len() as isize) };
    }
}

fn handle_approve(pane_id: &str, action: ApproveAction) -> RemoteBridgeResponse {
    let (vte, kind) = match find_pane_vte_and_kind(pane_id) {
        Ok(pair) => pair,
        Err(m) => return RemoteBridgeResponse::NotFound(m),
    };
    feed_bytes(vte, action.to_bytes(kind));
    topbar::refresh_ui();
    RemoteBridgeResponse::OkJson(format!("{{\"ok\":true,\"pane_id\":\"{pane_id}\",\"action\":\"{}\"}}", action.as_str()))
}

fn handle_send(pane_id: &str, text: &str, enter: bool) -> RemoteBridgeResponse {
    let (vte, _) = match find_pane_vte_and_kind(pane_id) {
        Ok(pair) => pair,
        Err(m) => return RemoteBridgeResponse::NotFound(m),
    };
    let payload = if enter { format!("{text}\r") } else { text.to_owned() };
    feed_bytes(vte, payload.as_bytes());
    RemoteBridgeResponse::OkJson(format!("{{\"ok\":true,\"pane_id\":\"{pane_id}\",\"bytes_sent\":{}}}", payload.len()))
}

pub fn send_pane_key(pane_id: &str, key: RemoteKey) -> Result<(), String> {
    let (vte, _) = find_pane_vte_and_kind(pane_id)?;
    feed_bytes(vte, key.to_bytes());
    Ok(())
}

fn handle_key(pane_id: &str, key: RemoteKey) -> RemoteBridgeResponse {
    match send_pane_key(pane_id, key) {
        Ok(()) => RemoteBridgeResponse::OkJson(format!("{{\"ok\":true,\"pane_id\":\"{pane_id}\",\"key\":\"{}\"}}", key.as_str())),
        Err(m) => RemoteBridgeResponse::NotFound(m),
    }
}
