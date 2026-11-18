//! Topbar `Ports ▾` button, popover UI, radar refresh, and SSH Ctrl+Click auto-forwarding.

use std::cell::Cell;
use std::collections::HashSet;
use std::ffi::{c_int, c_void, CString};
use std::ptr;
use std::rc::Rc;

use crate::app_state::{self, AppState};
use crate::ffi::*;
use crate::i18n::UiLocale;
use crate::port_radar::{self, DetectedPort, LocalUrlTarget, RemotePortTarget};
use crate::session::{SessionCategory, TermSession};
use crate::ssh_tunnel::{TunnelEnsureResult, TunnelInfo, TunnelRequest};
use crate::{context_menu, handoff, i18n, proc_radar, topbar, ui_text as text};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortsSnapshot {
    pub session_id: usize,
    pub remote_host: Option<String>,
    pub ssh_pid: i32,
    pub local_ports: Vec<DetectedPort>,
    pub remote_ports: Vec<DetectedPort>,
    pub tunnels: Vec<TunnelInfo>,
}

#[derive(Clone)]
pub struct PortsMenu {
    button: *mut GtkWidget,
    popover: *mut GtkWidget,
    body_holder: *mut GtkWidget,
    inner_box: Rc<Cell<*mut GtkWidget>>,
    header_label: *mut GtkWidget,
    refresh_btn: *mut GtkWidget,
}

pub fn local_url_for(scheme: &str, port: u16) -> String {
    let proto = if scheme.eq_ignore_ascii_case("https") { "https" } else { "http" };
    format!("{proto}://127.0.0.1:{port}")
}

pub fn format_port_label(p: &DetectedPort) -> String {
    if p.process.is_empty() {
        format!(":{}", p.port)
    } else {
        format!(":{} · {}", p.port, p.process)
    }
}

pub fn format_tunnel_label(t: &TunnelInfo, locale: UiLocale) -> String {
    let to_word = text::PORTS_TO_WORD.for_locale(locale).to_string_lossy();
    let base = format!("{} :{} {to_word} :{}", t.host, t.remote_port, t.local_port);
    if t.process.is_empty() { base } else { format!("{base} · {}", t.process) }
}

fn unforwarded_remote_count(snap: &PortsSnapshot) -> usize {
    let Some(host) = snap.remote_host.as_deref() else { return 0 };
    snap.remote_ports
        .iter()
        .filter(|p| !snap.tunnels.iter().any(|t| t.host == host && t.remote_port == p.port))
        .count()
}

pub fn format_ports_button_label(snap: &PortsSnapshot, locale: UiLocale) -> String {
    let word = text::PORTS_BTN_WORD.for_locale(locale).to_string_lossy();
    let first_port = snap
        .tunnels
        .first()
        .map(|t| t.local_port)
        .or_else(|| snap.local_ports.first().map(|p| p.port))
        .or_else(|| snap.remote_ports.first().map(|p| p.port));
    let total = snap.tunnels.len() + snap.local_ports.len() + unforwarded_remote_count(snap);
    match (first_port, total) {
        (_, 0) | (None, _) => format!("{word} ▾"),
        (Some(p), 1) => format!("{word} :{p} ▾"),
        (Some(p), n) => format!("{word} :{p} +{} ▾", n - 1),
    }
}

fn make_popover_header(vbox: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let hdr = gtk_label_new(text::PORTS_HEADER.cstr().as_ptr());
        gtk_label_set_xalign(hdr, 0.0);
        add_class(hdr, "section-header");
        let refresh = gtk_button_new_with_label(c"↻".as_ptr());
        add_class(refresh, "btn-icon-sq");
        gtk_widget_set_tooltip_text(refresh, text::PORTS_REFRESH_TIP.cstr().as_ptr());
        app_state::connect_click(refresh, on_refresh_clicked, ptr::null_mut());
        gtk_box_pack_start(row, hdr, 1, 1, 0);
        gtk_box_pack_end(row, refresh, 0, 0, 0);
        gtk_box_pack_start(vbox, row, 0, 1, 0);
        (hdr, refresh)
    }
}

fn make_ports_scroll(vbox: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, 360);
        gtk_scrolled_window_set_max_content_height(scroll, 340);
        let holder = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        gtk_container_add(scroll, holder);
        gtk_box_pack_start(vbox, scroll, 1, 1, 0);
        holder
    }
}

fn build_ports_popover_root(popover: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(root, "popover-box");
        add_class(root, "ports-popover-box");
        let (header_label, refresh_btn) = make_popover_header(root);
        let body_holder = make_ports_scroll(root);
        gtk_container_add(popover, root);
        gtk_widget_show_all(root);
        (header_label, refresh_btn, body_holder)
    }
}

impl PortsMenu {
    pub fn new(topbar: *mut GtkWidget) -> Self {
        unsafe {
            let button = gtk_button_new_with_label(text::ports_btn().as_ptr());
            gtk_widget_set_tooltip_text(button, text::ports_tip().as_ptr());
            gtk_box_pack_end(topbar, button, 0, 0, 0);
            let popover = gtk_popover_new(button);
            let (header_label, refresh_btn, body_holder) = build_ports_popover_root(popover);
            app_state::connect_click(button, on_ports_btn_clicked, ptr::null_mut());
            Self { button, popover, body_holder, inner_box: Rc::new(Cell::new(ptr::null_mut())), header_label, refresh_btn }
        }
    }

    #[cfg(test)]
    pub fn button_widget(&self) -> *mut GtkWidget {
        self.button
    }

    #[cfg(test)]
    pub fn popover_widget(&self) -> *mut GtkWidget {
        self.popover
    }

    pub fn refresh_locale(&self) {
        if self.button.is_null() {
            return;
        }
        unsafe {
            gtk_widget_set_tooltip_text(self.button, text::ports_tip().as_ptr());
            gtk_label_set_text(self.header_label, text::PORTS_HEADER.cstr().as_ptr());
            gtk_widget_set_tooltip_text(self.refresh_btn, text::PORTS_REFRESH_TIP.cstr().as_ptr());
        }
    }

    pub fn sync_button(&self, snap: &PortsSnapshot, locale: UiLocale) {
        if self.button.is_null() {
            return;
        }
        let label = format_ports_button_label(snap, locale);
        if let Ok(c_lbl) = CString::new(label) {
            unsafe { gtk_button_set_label(self.button, c_lbl.as_ptr()) };
        }
        let has_tunnels = !snap.tunnels.is_empty();
        let has_ports = !snap.local_ports.is_empty() || !snap.remote_ports.is_empty();
        set_class(self.button, "ports-btn-tunnel", has_tunnels);
        set_class(self.button, "ports-btn-active", !has_tunnels && has_ports);
    }

    pub fn populate(&self, snap: &PortsSnapshot, locale: UiLocale) {
        if self.body_holder.is_null() {
            return;
        }
        self.refresh_locale();
        unsafe {
            let old = self.inner_box.get();
            if !old.is_null() {
                gtk_widget_destroy(old);
            }
            let inner = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
            populate_sections(inner, snap, locale);
            gtk_box_pack_start(self.body_holder, inner, 0, 1, 0);
            gtk_widget_show_all(inner);
            self.inner_box.set(inner);
        }
    }

    pub fn popup(&self) {
        if !self.popover.is_null() {
            unsafe { gtk_popover_popup(self.popover) };
        }
    }

    pub fn popdown(&self) {
        if !self.popover.is_null() {
            unsafe { gtk_popover_popdown(self.popover) };
        }
    }
}

enum PortRowAction {
    OpenUrl(String),
    CopyUrl(String),
    ForwardAndOpen {
        sid: usize,
        host: String,
        ssh_pid: i32,
        port: u16,
        scheme: String,
        process: String,
    },
    StopTunnel {
        id: u64,
        label: String,
    },
}

extern "C" fn drop_row_action(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data.cast::<PortRowAction>())) };
    }
}

fn connect_row_action(btn: *mut GtkWidget, action: PortRowAction) {
    let raw = Box::into_raw(Box::new(action));
    unsafe {
        g_signal_connect_data(
            btn,
            c"clicked".as_ptr(),
            on_row_action_clicked as *const c_void,
            raw.cast(),
            drop_row_action as *const c_void,
            0,
        );
    }
}

fn pack_section_header(parent: *mut GtkWidget, title: &str) {
    let Ok(c_title) = CString::new(title) else { return };
    unsafe {
        let lbl = gtk_label_new(c_title.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-header");
        gtk_box_pack_start(parent, lbl, 0, 0, 2);
    }
}

fn make_action_button(label: &str, css: &str, action: PortRowAction) -> *mut GtkWidget {
    let c_lbl = CString::new(label).unwrap_or_default();
    unsafe {
        let btn = gtk_button_new_with_label(c_lbl.as_ptr());
        add_class(btn, css);
        connect_row_action(btn, action);
        btn
    }
}

fn make_row_box() -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        add_class(row, "ports-row");
        row
    }
}

fn make_row_label(text_str: &str) -> *mut GtkWidget {
    let c_txt = CString::new(text_str).unwrap_or_default();
    unsafe {
        let lbl = gtk_label_new(c_txt.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "ports-row-label");
        lbl
    }
}

fn pack_tunnel_row(parent: *mut GtkWidget, t: &TunnelInfo, locale: UiLocale) {
    let label_str = format_tunnel_label(t, locale);
    let url = local_url_for(&t.scheme, t.local_port);
    let open_txt = text::PORTS_OPEN.for_locale(locale).to_string_lossy();
    let copy_txt = text::PORTS_COPY.for_locale(locale).to_string_lossy();
    let row = make_row_box();
    let lbl = make_row_label(&label_str);
    let b_open = make_action_button(&open_txt, "ports-action-btn", PortRowAction::OpenUrl(url.clone()));
    let b_copy = make_action_button(&copy_txt, "ports-action-btn", PortRowAction::CopyUrl(url));
    let b_stop = make_action_button("×", "ports-stop-btn", PortRowAction::StopTunnel { id: t.id, label: label_str });
    unsafe {
        gtk_widget_set_tooltip_text(b_stop, text::PORTS_STOP_TIP.for_locale(locale).as_ptr());
        gtk_box_pack_start(row, lbl, 1, 1, 2);
        gtk_box_pack_end(row, b_stop, 0, 0, 0);
        gtk_box_pack_end(row, b_copy, 0, 0, 0);
        gtk_box_pack_end(row, b_open, 0, 0, 0);
        gtk_box_pack_start(parent, row, 0, 1, 0);
    }
}

fn pack_local_port_row(parent: *mut GtkWidget, p: &DetectedPort, locale: UiLocale) {
    let url = local_url_for(&p.scheme, p.port);
    let copy_txt = text::PORTS_COPY.for_locale(locale).to_string_lossy();
    let row = make_row_box();
    let b_open = make_action_button(&format_port_label(p), "ports-action-btn", PortRowAction::OpenUrl(url.clone()));
    let b_copy = make_action_button(&copy_txt, "ports-action-btn", PortRowAction::CopyUrl(url.clone()));
    unsafe {
        if let Ok(c_tip) = CString::new(url.as_str()) {
            gtk_widget_set_tooltip_text(b_open, c_tip.as_ptr());
        }
        gtk_box_pack_start(row, b_open, 1, 1, 0);
        gtk_box_pack_end(row, b_copy, 0, 0, 0);
        gtk_box_pack_start(parent, row, 0, 1, 0);
    }
}

fn build_remote_row_action(
    snap: &PortsSnapshot,
    p: &DetectedPort,
    host: &str,
    existing: Option<&TunnelInfo>,
    locale: UiLocale,
) -> (String, PortRowAction) {
    if let Some(t) = existing {
        let lbl = format!("{} :{}", text::PORTS_OPEN.for_locale(locale).to_string_lossy(), t.local_port);
        return (lbl, PortRowAction::OpenUrl(local_url_for(&t.scheme, t.local_port)));
    }
    let lbl = text::PORTS_FORWARD_OPEN.for_locale(locale).to_string_lossy().into_owned();
    let act = PortRowAction::ForwardAndOpen {
        sid: snap.session_id,
        host: host.to_owned(),
        ssh_pid: snap.ssh_pid,
        port: p.port,
        scheme: p.scheme.clone(),
        process: p.process.clone(),
    };
    (lbl, act)
}

fn pack_remote_port_row(parent: *mut GtkWidget, snap: &PortsSnapshot, p: &DetectedPort, host: &str, locale: UiLocale) {
    let existing = snap.tunnels.iter().find(|t| t.host == host && t.remote_port == p.port);
    let copy_url = existing.map_or_else(|| local_url_for(&p.scheme, p.port), |t| local_url_for(&t.scheme, t.local_port));
    let copy_txt = text::PORTS_COPY.for_locale(locale).to_string_lossy();
    let (act_lbl, act) = build_remote_row_action(snap, p, host, existing, locale);
    let row = make_row_box();
    let lbl = make_row_label(&format_port_label(p));
    let b_act = make_action_button(&act_lbl, "ports-action-btn", act);
    let b_copy = make_action_button(&copy_txt, "ports-action-btn", PortRowAction::CopyUrl(copy_url));
    unsafe {
        gtk_box_pack_start(row, lbl, 1, 1, 2);
        gtk_box_pack_end(row, b_copy, 0, 0, 0);
        gtk_box_pack_end(row, b_act, 0, 0, 0);
        gtk_box_pack_start(parent, row, 0, 1, 0);
    }
}

fn pack_empty_hint(parent: *mut GtkWidget, is_remote: bool, locale: UiLocale) {
    let msg = if is_remote {
        text::PORTS_EMPTY_REMOTE.for_locale(locale)
    } else {
        text::PORTS_EMPTY_LOCAL.for_locale(locale)
    };
    unsafe {
        let lbl = gtk_label_new(msg.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        gtk_label_set_line_wrap(lbl, 1);
        add_class(lbl, "ports-empty-label");
        gtk_box_pack_start(parent, lbl, 0, 1, 2);
    }
}

fn populate_tunnels_section(parent: *mut GtkWidget, tunnels: &[TunnelInfo], locale: UiLocale) {
    if tunnels.is_empty() {
        return;
    }
    pack_section_header(parent, &text::PORTS_SECTION_TUNNELS.for_locale(locale).to_string_lossy());
    for t in tunnels {
        pack_tunnel_row(parent, t, locale);
    }
}

fn populate_remote_section(parent: *mut GtkWidget, snap: &PortsSnapshot, host: &str, locale: UiLocale) {
    let hdr = format!("{} · {host}", text::PORTS_SECTION_REMOTE.for_locale(locale).to_string_lossy());
    pack_section_header(parent, &hdr);
    for p in &snap.remote_ports {
        pack_remote_port_row(parent, snap, p, host, locale);
    }
    if snap.remote_ports.is_empty() && snap.tunnels.is_empty() {
        pack_empty_hint(parent, true, locale);
    }
}

fn populate_local_section(parent: *mut GtkWidget, snap: &PortsSnapshot, locale: UiLocale) {
    pack_section_header(parent, &text::PORTS_SECTION_LOCAL.for_locale(locale).to_string_lossy());
    for p in &snap.local_ports {
        pack_local_port_row(parent, p, locale);
    }
    if snap.local_ports.is_empty() && snap.tunnels.is_empty() {
        pack_empty_hint(parent, false, locale);
    }
}

fn populate_sections(parent: *mut GtkWidget, snap: &PortsSnapshot, locale: UiLocale) {
    populate_tunnels_section(parent, &snap.tunnels, locale);
    if let Some(host) = snap.remote_host.as_deref() {
        populate_remote_section(parent, snap, host, locale);
    } else {
        populate_local_section(parent, snap, locale);
    }
}

fn session_remote_host(s: &TermSession) -> Option<String> {
    if let Some(remote) = s.remote() {
        return Some(remote.host.clone());
    }
    (!s.proc_info.ssh_host.is_empty()).then(|| s.proc_info.ssh_host.clone())
}

fn collect_session_local_pids(s: &TermSession) -> Vec<i32> {
    let mut pids = Vec::new();
    for p in [s.child_pid, s.active_pid(), s.proc_info.active_pane_pid] {
        if p > 0 && !pids.contains(&p) {
            pids.push(p);
        }
    }
    for pane in s.pane_registry.panes() {
        for p in [pane.child_pid, pane.proc_info.active_pane_pid] {
            if p > 0 && !pids.contains(&p) {
                pids.push(p);
            }
        }
    }
    pids
}

fn refresh_remote_session_urls(app: &mut AppState, sid: usize, vte: *mut GtkWidget) {
    let text = handoff::extract_vte_text(vte.cast());
    app.remote_url_ports_by_sid.insert(sid, port_radar::extract_local_urls_from_text(&text));
}

fn collect_radar_updates(
    sessions: &[TermSession],
    active_id: usize,
    sockets: &[port_radar::ListeningSocket],
) -> (Vec<RemotePortTarget>, Vec<(usize, Vec<DetectedPort>)>, Option<(usize, *mut GtkWidget)>) {
    let mut remote_targets = Vec::new();
    let mut local_updates = Vec::new();
    let mut active_remote_vte = None;
    for s in sessions {
        if let Some(host) = session_remote_host(s) {
            remote_targets.push(RemotePortTarget { host, ssh_pid: s.proc_info.ssh_pid });
            if s.id == active_id {
                active_remote_vte = Some((s.id, s.active_vte()));
            }
        } else {
            let pids = collect_session_local_pids(s);
            local_updates.push((s.id, port_radar::detect_ports_for_pids_with_table(&pids, sockets)));
        }
    }
    (remote_targets, local_updates, active_remote_vte)
}

pub(crate) fn refresh_radar_ports(app: &mut AppState) {
    app.ssh_tunnels.prune_exited();
    let sockets = port_radar::read_local_listening_sockets();
    let live_ids: HashSet<usize> = app.sessions.iter().map(|s| s.id).collect();
    app.local_ports_by_sid.retain(|id, _| live_ids.contains(id));
    app.remote_url_ports_by_sid.retain(|id, _| live_ids.contains(id));
    let (remote_targets, local_updates, active_remote) =
        collect_radar_updates(&app.sessions, app.active_id, &sockets);
    for (sid, detected) in local_updates {
        app.local_ports_by_sid.insert(sid, detected);
        app.remote_url_ports_by_sid.remove(&sid);
    }
    if let Some((sid, vte)) = active_remote {
        app.local_ports_by_sid.remove(&sid);
        refresh_remote_session_urls(app, sid, vte);
    }
    app.remote_ports.sync_targets(&remote_targets);
}

pub(crate) fn collect_active_snapshot(app: &AppState) -> PortsSnapshot {
    let tunnels = app.ssh_tunnels.list_tunnels_snapshot();
    let Some(s) = app.sessions.iter().find(|s| s.id == app.active_id) else {
        return PortsSnapshot { tunnels, ..PortsSnapshot::default() };
    };
    let remote_host = session_remote_host(s);
    let remote_ports = remote_host.as_deref().map_or_else(Vec::new, |host| {
        let probed = app.remote_ports.ports_for_host(host);
        let urls = app.remote_url_ports_by_sid.get(&s.id).cloned().unwrap_or_default();
        port_radar::merge_detected_ports(probed, urls)
    });
    let local_ports = app.local_ports_by_sid.get(&s.id).cloned().unwrap_or_default();
    PortsSnapshot {
        session_id: s.id,
        remote_host,
        ssh_pid: s.proc_info.ssh_pid,
        local_ports,
        remote_ports,
        tunnels,
    }
}

fn report_tunnel_footer(prefix: text::UiText, info: &TunnelInfo) {
    let loc = i18n::current_locale();
    let p = prefix.for_locale(loc).to_string_lossy();
    let detail = format_tunnel_label(info, loc);
    app_state::set_footer_message(&format!("{p} {detail}"));
}

fn report_tunnel_error(host: &str, remote_port: u16, reason: &str) {
    let loc = i18n::current_locale();
    let p = text::PORTS_TUNNEL_FAILED.for_locale(loc).to_string_lossy();
    app_state::set_footer_message(&format!("{p} {host}:{remote_port} ({reason})"));
}

#[cfg(test)]
thread_local! {
    static TEST_OPENED_URLS: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

fn open_browser_url(url: &str) {
    #[cfg(test)]
    if TEST_OPENED_URLS.with(|slot| {
        if let Some(list) = slot.borrow_mut().as_mut() {
            list.push(url.to_owned());
            return true;
        }
        false
    }) {
        return;
    }
    proc_radar::open_external_link(url);
}

struct PendingTunnelOpen {
    tunnel_id: u64,
    local_port: u16,
    remote_port: u16,
    host: String,
    url: String,
    attempts: u8,
}

fn finish_pending_open(data: *mut c_void, open: bool) -> c_int {
    let pending = unsafe { Box::from_raw(data.cast::<PendingTunnelOpen>()) };
    if open {
        open_browser_url(&pending.url);
    }
    0
}

fn handle_inactive_tunnel_poll(alive: Option<bool>, data: *mut c_void, host: &str, port: u16) -> c_int {
    let rc = finish_pending_open(data, false);
    if alive == Some(false) {
        report_tunnel_error(host, port, "ssh exited");
        topbar::refresh_ui();
    }
    rc
}

extern "C" fn on_tunnel_ready_poll(data: *mut c_void) -> c_int {
    if data.is_null() {
        return 0;
    }
    let pending = unsafe { &mut *data.cast::<PendingTunnelOpen>() };
    if port_radar::is_local_port_listening(pending.local_port) {
        return finish_pending_open(data, true);
    }
    let alive = app_state::with_app_mut(|app| app.ssh_tunnels.is_tunnel_alive(pending.tunnel_id));
    if alive != Some(true) {
        let (host, port) = (pending.host.clone(), pending.remote_port);
        return handle_inactive_tunnel_poll(alive, data, &host, port);
    }
    pending.attempts = pending.attempts.saturating_add(1);
    if pending.attempts >= 30 { finish_pending_open(data, true) } else { 1 }
}

fn schedule_or_open_forwarded(res: &TunnelEnsureResult, url: String) {
    if res.reused || port_radar::is_local_port_listening(res.info.local_port) {
        open_browser_url(&url);
        return;
    }
    let pending = Box::into_raw(Box::new(PendingTunnelOpen {
        tunnel_id: res.info.id,
        local_port: res.info.local_port,
        remote_port: res.info.remote_port,
        host: res.info.host.clone(),
        url,
        attempts: 0,
    }));
    unsafe { g_timeout_add(100, on_tunnel_ready_poll, pending.cast()) };
}

fn ensure_and_open_tunnel(req: TunnelRequest, original_url: Option<&str>) -> bool {
    let host = req.host.clone();
    let rport = req.remote_port;
    let outcome = app_state::with_app_mut(|app| app.ssh_tunnels.ensure_tunnel(req));
    let Some(res) = outcome else { return false };
    match res {
        Ok(ensured) => {
            let target_url = original_url
                .and_then(|raw| port_radar::rewrite_local_url(raw, ensured.info.local_port))
                .unwrap_or_else(|| local_url_for(&ensured.info.scheme, ensured.info.local_port));
            report_tunnel_footer(text::PORTS_TUNNEL_READY, &ensured.info);
            schedule_or_open_forwarded(&ensured, target_url);
            topbar::refresh_ui();
            true
        }
        Err(err) => {
            report_tunnel_error(&host, rport, &err.to_string());
            false
        }
    }
}

fn remote_target_in_session(s: &TermSession, vte: *mut GtkWidget) -> Option<(usize, String, i32)> {
    let pane = s.pane_registry.by_vte(vte);
    if let Some(remote) = s.remote() {
        let ssh_pid = pane.map_or(s.proc_info.ssh_pid, |p| p.proc_info.ssh_pid);
        return Some((s.id, remote.host.clone(), ssh_pid));
    }
    if let Some(p) = pane.filter(|p| !p.proc_info.ssh_host.is_empty()) {
        return Some((s.id, p.proc_info.ssh_host.clone(), p.proc_info.ssh_pid));
    }
    let is_rem = s.effective_category() == SessionCategory::Remote && !s.proc_info.ssh_host.is_empty();
    is_rem.then(|| (s.id, s.proc_info.ssh_host.clone(), s.proc_info.ssh_pid))
}

fn lookup_remote_vte_target(vte: *mut GtkWidget) -> Option<(usize, String, i32)> {
    app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.has_vte(vte))?;
        remote_target_in_session(s, vte)
    })
    .flatten()
}

pub(crate) fn intercept_vte_link_open(vte: *mut GtkWidget, url: &str) -> bool {
    let Some(LocalUrlTarget { scheme, port, .. }) = port_radar::parse_local_url(url) else {
        return false;
    };
    let Some((sid, host, ssh_pid)) = lookup_remote_vte_target(vte) else {
        return false;
    };
    let req = TunnelRequest {
        host,
        ssh_pid,
        remote_port: port,
        scheme,
        process: String::new(),
        session_id: sid,
    };
    ensure_and_open_tunnel(req, Some(url))
}

pub(crate) fn intercept_active_link_open(url: &str) -> bool {
    let vte = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.id == app.active_id).map(TermSession::active_vte)
    })
    .flatten()
    .unwrap_or(ptr::null_mut());
    !vte.is_null() && intercept_vte_link_open(vte, url)
}

fn dismiss_ports_popover() {
    if let Some(menu) = app_state::with_app(|app| app.ports_menu.clone()) {
        menu.popdown();
    }
}

fn copy_port_url(url: &str) {
    context_menu::copy_text_to_clipboard(url);
    let prefix = text::PORTS_COPIED.for_locale(i18n::current_locale()).to_string_lossy();
    app_state::set_footer_message(&format!("{prefix} {url}"));
}

fn stop_tunnel_row_action(id: u64, label: &str) {
    let stopped = app_state::with_app_mut(|app| app.ssh_tunnels.stop_tunnel(id)).unwrap_or(false);
    if stopped {
        let prefix = text::PORTS_TUNNEL_STOPPED.for_locale(i18n::current_locale()).to_string_lossy();
        app_state::set_footer_message(&format!("{prefix} {label}"));
    }
    refresh_and_repopulate_popover(false);
    topbar::refresh_ui();
}

fn handle_row_action(action: &PortRowAction) {
    match action {
        PortRowAction::OpenUrl(url) => {
            dismiss_ports_popover();
            open_browser_url(url);
        }
        PortRowAction::CopyUrl(url) => copy_port_url(url),
        PortRowAction::ForwardAndOpen { sid, host, ssh_pid, port, scheme, process } => {
            dismiss_ports_popover();
            let req = TunnelRequest {
                host: host.clone(), ssh_pid: *ssh_pid, remote_port: *port,
                scheme: scheme.clone(), process: process.clone(), session_id: *sid,
            };
            let _ = ensure_and_open_tunnel(req, None);
        }
        PortRowAction::StopTunnel { id, label } => stop_tunnel_row_action(*id, label),
    }
}

extern "C" fn on_row_action_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let action = unsafe { &*data.cast::<PortRowAction>() };
    handle_row_action(action);
}

fn refresh_and_repopulate_popover(open: bool) {
    let snap = app_state::with_app_mut(|app| {
        refresh_radar_ports(app);
        let snap = collect_active_snapshot(app);
        if let Some(host) = snap.remote_host.as_deref() {
            app.remote_ports.trigger_refresh(host);
        }
        (app.ports_menu.clone(), snap)
    });
    let Some((menu, snap)) = snap else { return };
    let locale = i18n::current_locale();
    menu.sync_button(&snap, locale);
    menu.populate(&snap, locale);
    if open {
        menu.popup();
    }
}

extern "C" fn on_ports_btn_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    refresh_and_repopulate_popover(true);
}

extern "C" fn on_refresh_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    refresh_and_repopulate_popover(false);
}

pub(crate) fn open_ports_popover_for_app() {
    refresh_and_repopulate_popover(true);
}

extern "C" fn on_remote_ports_ready(_data: *mut c_void) -> c_int {
    topbar::refresh_ui();
    0
}

pub(crate) fn remote_ports_ready_cb() -> extern "C" fn(*mut c_void) -> c_int {
    on_remote_ports_ready
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_radar::BindScope;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use std::ffi::CStr;

    fn sample_tunnel(host: &str, remote_port: u16, local_port: u16) -> TunnelInfo {
        TunnelInfo {
            id: 1,
            host: host.to_owned(),
            remote_port,
            local_port,
            scheme: "http".to_owned(),
            process: "uvicorn".to_owned(),
            session_ids: vec![1],
            pid: 9999,
        }
    }

    fn sample_port(port: u16, process: &str) -> DetectedPort {
        DetectedPort {
            port,
            process: process.to_owned(),
            scheme: "http".to_owned(),
            scope: BindScope::Loopback,
        }
    }

    #[test]
    fn formats_bilingual_button_and_tunnel_labels_without_text_arrows() {
        let idle = PortsSnapshot::default();
        assert_eq!(format_ports_button_label(&idle, UiLocale::Ru), "Порты ▾");
        assert_eq!(format_ports_button_label(&idle, UiLocale::En), "Ports ▾");

        let single = PortsSnapshot { local_ports: vec![sample_port(8000, "uvicorn")], ..idle.clone() };
        assert_eq!(format_ports_button_label(&single, UiLocale::Ru), "Порты :8000 ▾");
        assert_eq!(format_ports_button_label(&single, UiLocale::En), "Ports :8000 ▾");

        let multi = PortsSnapshot {
            remote_host: Some("gpu01".into()),
            remote_ports: vec![sample_port(8000, "uvicorn"), sample_port(5173, "vite")],
            tunnels: vec![sample_tunnel("gpu01", 8000, 18000)],
            ..idle
        };
        assert_eq!(format_ports_button_label(&multi, UiLocale::Ru), "Порты :18000 +1 ▾");
        assert_eq!(format_ports_button_label(&multi, UiLocale::En), "Ports :18000 +1 ▾");
    }

    #[test]
    fn formats_tunnel_labels_in_ru_and_en_without_text_arrows() {
        let t = sample_tunnel("gpu01", 8000, 18000);
        let t_ru = format_tunnel_label(&t, UiLocale::Ru);
        let t_en = format_tunnel_label(&t, UiLocale::En);
        assert_eq!(t_ru, "gpu01 :8000 на :18000 · uvicorn");
        assert_eq!(t_en, "gpu01 :8000 to :18000 · uvicorn");
        for s in [&t_ru, &t_en] {
            assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"));
        }
    }

    fn btn_label_text(btn: *mut GtkWidget) -> String {
        unsafe { CStr::from_ptr(gtk_button_get_label(btn)).to_string_lossy().into_owned() }
    }

    fn remote_demo_snap() -> PortsSnapshot {
        PortsSnapshot {
            session_id: 2,
            remote_host: Some("gpu-box".into()),
            remote_ports: vec![sample_port(8000, "uvicorn"), sample_port(8888, "jupyter")],
            tunnels: vec![sample_tunnel("gpu-box", 8000, 18000)],
            ..PortsSnapshot::default()
        }
    }

    fn mount_test_ports_bar() -> (*mut GtkWidget, PortsMenu) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            let bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
            gtk_container_add(win, bar);
            let menu = PortsMenu::new(bar);
            gtk_widget_show_all(win);
            (win, menu)
        }
    }

    #[test]
    fn headless_broadway_ports_popover_renders_local_ports() {
        with_headless_gtk(|| unsafe {
            let (win, menu) = mount_test_ports_bar();
            let local = PortsSnapshot {
                session_id: 1,
                local_ports: vec![sample_port(8000, "uvicorn"), sample_port(5173, "node")],
                ..PortsSnapshot::default()
            };
            menu.sync_button(&local, UiLocale::En);
            menu.populate(&local, UiLocale::En);
            menu.popup();
            pump_gtk_events();
            assert!(!menu.popover_widget().is_null());
            assert_eq!(btn_label_text(menu.button_widget()), "Ports :8000 +1 ▾");
            menu.popdown();
            gtk_widget_destroy(win);
            pump_gtk_events();
        });
    }

    #[test]
    fn headless_broadway_ports_popover_renders_remote_and_tunnels() {
        with_headless_gtk(|| unsafe {
            let (win, menu) = mount_test_ports_bar();
            let remote = remote_demo_snap();
            menu.sync_button(&remote, UiLocale::Ru);
            menu.populate(&remote, UiLocale::Ru);
            menu.popup();
            pump_gtk_events();
            assert_eq!(btn_label_text(menu.button_widget()), "Порты :18000 +1 ▾");
            menu.popdown();
            gtk_widget_destroy(win);
            pump_gtk_events();
        });
    }

    fn take_test_urls() -> Vec<String> {
        TEST_OPENED_URLS.with(|slot| slot.borrow_mut().take().unwrap_or_default())
    }

    fn setup_ctrl_click_tabs() -> (*mut GtkWidget, usize, usize, *mut GtkWidget, *mut GtkWidget) {
        let (win, _) = crate::tests::install_test_window();
        app_state::with_app_mut(|app| {
            app.ssh_tunnels = crate::ssh_tunnel::SshTunnelManager::with_test_command("sleep", &["60"]);
        });
        let local_sid = crate::session_ops::add_tab(
            "Local", "", "/tmp", SessionCategory::Project, &["sleep", "30"], crate::tabs::Tab::local("shell"),
        );
        let remote_sid = crate::session_ops::add_tab(
            "GPU", "", "/tmp", SessionCategory::Remote, &["sleep", "30"], crate::tabs::Tab::remote("gpu01", None),
        );
        let (lv, rv) = app_state::with_app(|app| (app.sessions[0].vte, app.sessions[1].vte)).unwrap();
        (win, local_sid, remote_sid, lv, rv)
    }

    #[test]
    fn ctrl_click_hook_forwards_remote_localhost_and_cleans_up_on_tab_close() {
        with_headless_gtk(|| {
            TEST_OPENED_URLS.with(|slot| *slot.borrow_mut() = Some(Vec::new()));
            let (win, local_sid, remote_sid, local_vte, remote_vte) = setup_ctrl_click_tabs();
            assert!(!intercept_vte_link_open(local_vte, "http://localhost:8000/docs"));
            assert!(!intercept_vte_link_open(remote_vte, "https://example.com:8000/docs"));
            assert!(intercept_vte_link_open(remote_vte, "http://localhost:8000/docs?x=1"));
            assert!(intercept_vte_link_open(remote_vte, "http://localhost:8000/docs?x=1"));
            let tunnels = app_state::with_app(|app| app.ssh_tunnels.list_tunnels_snapshot()).unwrap();
            assert_eq!(tunnels.len(), 1);
            let expected = format!("http://127.0.0.1:{}/docs?x=1", tunnels[0].local_port);
            assert!(take_test_urls().contains(&expected));
            let pid = tunnels[0].pid;
            crate::session_ops::close_session(remote_sid);
            assert!(app_state::with_app(|app| app.ssh_tunnels.list_tunnels_snapshot()).unwrap().is_empty());
            assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
            crate::tests::cleanup_test_window(win, &[local_sid]);
        });
    }

    #[test]
    fn command_palette_includes_bilingual_ports_entry() {
        let src = crate::palette::PaletteSource {
            sessions: &[], tmux_items: &[], profiles: &[], ssh_hosts: &[],
            briefs_dir: None, extra_handoffs: &[], active_git: None, active_is_worktree: false,
        };
        let ru = crate::palette::collect_items_for_locale(&src, UiLocale::Ru);
        let en = crate::palette::collect_items_for_locale(&src, UiLocale::En);
        assert!(ru.iter().any(|i| i.action == crate::palette::PaletteAction::OpenPortsMenu && i.title.contains("Порты")));
        assert!(en.iter().any(|i| i.action == crate::palette::PaletteAction::OpenPortsMenu && i.title.contains("Ports")));
    }

    #[test]
    fn dark_and_light_theme_css_load_ports_classes_cleanly() {
        with_headless_gtk(|| {
            crate::theme::apply_gtk_theme_mode(crate::theme_mode::ThemeMode::Dark);
            crate::theme::apply_gtk_theme_mode(crate::theme_mode::ThemeMode::Light);
            crate::theme::apply_gtk_theme_mode(crate::theme_mode::ThemeMode::Dark);
        });
    }
}
