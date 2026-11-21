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
use crate::ssh_tunnel::{PinnedTunnel, TunnelEnsureResult, TunnelInfo, TunnelRequest, TunnelStatus};
use crate::{context_menu, handoff, i18n, proc_radar, topbar, ui_text as text};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortsSnapshot {
    pub session_id: usize,
    pub remote_host: Option<String>,
    pub ssh_pid: i32,
    pub local_ports: Vec<DetectedPort>,
    pub remote_ports: Vec<DetectedPort>,
    pub tunnels: Vec<TunnelInfo>,
    pub pinned_tunnels: Vec<PinnedTunnel>,
}

#[derive(Clone)]
pub struct PortsMenu {
    button: *mut GtkWidget,
    popover: *mut GtkWidget,
    body_holder: *mut GtkWidget,
    inner_box: Rc<Cell<*mut GtkWidget>>,
    is_open: Rc<Cell<bool>>,
    header_label: *mut GtkWidget,
    refresh_btn: *mut GtkWidget,
    #[cfg(test)]
    populate_gen: Rc<Cell<usize>>,
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
        gtk_scrolled_window_set_min_content_width(scroll, 430);
        gtk_scrolled_window_set_max_content_height(scroll, 340);
        gtk_scrolled_window_set_propagate_natural_height(scroll, 1);
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

extern "C" fn on_ports_popover_closed(_pop: *mut GtkWidget, data: *mut c_void) {
    if !data.is_null() {
        unsafe { (&*data.cast::<Cell<bool>>()).set(false) };
    }
}

extern "C" fn drop_open_flag(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Rc::from_raw(data.cast::<Cell<bool>>())) };
    }
}

fn connect_popover_closed(popover: *mut GtkWidget, is_open: &Rc<Cell<bool>>) {
    let raw = Rc::into_raw(Rc::clone(is_open));
    unsafe {
        g_signal_connect_data(
            popover,
            c"closed".as_ptr(),
            on_ports_popover_closed as *const c_void,
            raw.cast_mut().cast(),
            drop_open_flag as *const c_void,
            0,
        );
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
            let is_open = Rc::new(Cell::new(false));
            connect_popover_closed(popover, &is_open);
            app_state::connect_click(button, on_ports_btn_clicked, ptr::null_mut());
            Self {
                button, popover, body_holder, inner_box: Rc::new(Cell::new(ptr::null_mut())),
                is_open, header_label, refresh_btn,
                #[cfg(test)]
                populate_gen: Rc::new(Cell::new(0)),
            }
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
        let has_active = snap.tunnels.iter().any(|t| t.status == TunnelStatus::Active);
        let has_disc = snap.tunnels.iter().any(|t| t.status == TunnelStatus::Disconnected);
        let has_ports = !snap.local_ports.is_empty() || !snap.remote_ports.is_empty();
        set_class(self.button, "ports-btn-tunnel", has_active);
        set_class(self.button, "ports-btn-disconnected", has_disc && !has_active);
        set_class(self.button, "ports-btn-active", !has_active && !has_disc && has_ports);
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
            #[cfg(test)]
            self.populate_gen.set(self.populate_gen.get().wrapping_add(1));
        }
    }

    pub fn popup(&self) {
        if !self.popover.is_null() {
            self.is_open.set(true);
            unsafe { gtk_popover_popup(self.popover) };
        }
    }

    pub fn popdown(&self) {
        self.is_open.set(false);
        if !self.popover.is_null() {
            unsafe { gtk_popover_popdown(self.popover) };
        }
    }

    pub fn is_visible(&self) -> bool {
        self.is_open.get() && !self.popover.is_null() && unsafe { gtk_widget_get_visible(self.popover) != 0 }
    }

    #[cfg(test)]
    pub fn inner_widget(&self) -> *mut GtkWidget {
        self.inner_box.get()
    }

    #[cfg(test)]
    pub fn populate_count(&self) -> usize {
        self.populate_gen.get()
    }
}

#[derive(Clone)]
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
    ReconnectTunnel {
        id: u64,
        ssh_pid: i32,
        host: String,
        remote_port: u16,
    },
    PinTunnel(PinnedTunnel),
    UnpinTunnel {
        host: String,
        remote_port: u16,
    },
    RestorePinned {
        host: String,
        ssh_pid: i32,
        sid: usize,
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

fn tunnel_to_pinned(t: &TunnelInfo) -> PinnedTunnel {
    PinnedTunnel {
        host: t.host.clone(), remote_port: t.remote_port, local_port: t.local_port,
        scheme: t.scheme.clone(), process: t.process.clone(),
    }
}

fn make_pin_toggle_btn(t: &TunnelInfo, is_pinned: bool, locale: UiLocale) -> *mut GtkWidget {
    let (lbl, tip, css, act) = if is_pinned {
        (
            text::PORTS_UNPIN.for_locale(locale).to_string_lossy(),
            text::PORTS_UNPIN_TIP.for_locale(locale),
            "ports-pin-btn-active",
            PortRowAction::UnpinTunnel { host: t.host.clone(), remote_port: t.remote_port },
        )
    } else {
        (
            text::PORTS_PIN.for_locale(locale).to_string_lossy(),
            text::PORTS_PIN_TIP.for_locale(locale),
            "ports-action-btn",
            PortRowAction::PinTunnel(tunnel_to_pinned(t)),
        )
    };
    let btn = make_action_button(&lbl, css, act);
    unsafe { gtk_widget_set_tooltip_text(btn, tip.as_ptr()) };
    btn
}

fn pack_disconnected_tunnel_controls(row: *mut GtkWidget, t: &TunnelInfo, ssh_pid: i32, locale: UiLocale) {
    add_class(row, "ports-row-disconnected");
    let badge_c = text::PORTS_TUNNEL_DISCONNECTED.for_locale(locale);
    let rec_txt = text::PORTS_RECONNECT.for_locale(locale).to_string_lossy();
    let rec_act = PortRowAction::ReconnectTunnel { id: t.id, ssh_pid, host: t.host.clone(), remote_port: t.remote_port };
    let b_rec = make_action_button(&rec_txt, "ports-reconnect-btn", rec_act);
    unsafe {
        let badge = gtk_label_new(badge_c.as_ptr());
        add_class(badge, "ports-badge-disconnected");
        gtk_box_pack_start(row, badge, 0, 0, 2);
        gtk_box_pack_end(row, b_rec, 0, 0, 0);
    }
}

fn pack_active_tunnel_controls(row: *mut GtkWidget, t: &TunnelInfo, locale: UiLocale) {
    let url = local_url_for(&t.scheme, t.local_port);
    let open_txt = text::PORTS_OPEN.for_locale(locale).to_string_lossy();
    let copy_txt = text::PORTS_COPY.for_locale(locale).to_string_lossy();
    let b_open = make_action_button(&open_txt, "ports-action-btn", PortRowAction::OpenUrl(url.clone()));
    let b_copy = make_action_button(&copy_txt, "ports-action-btn", PortRowAction::CopyUrl(url));
    unsafe {
        gtk_box_pack_end(row, b_copy, 0, 0, 0);
        gtk_box_pack_end(row, b_open, 0, 0, 0);
    }
}

fn pack_tunnel_row(parent: *mut GtkWidget, t: &TunnelInfo, snap: &PortsSnapshot, locale: UiLocale) {
    let label_str = format_tunnel_label(t, locale);
    let is_pinned = snap.pinned_tunnels.iter().any(|p| p.host == t.host && p.remote_port == t.remote_port);
    let row = make_row_box();
    let lbl = make_row_label(&label_str);
    let b_pin = make_pin_toggle_btn(t, is_pinned, locale);
    let b_stop = make_action_button("×", "ports-stop-btn", PortRowAction::StopTunnel { id: t.id, label: label_str });
    unsafe {
        gtk_widget_set_tooltip_text(b_stop, text::PORTS_STOP_TIP.for_locale(locale).as_ptr());
        gtk_box_pack_start(row, lbl, 1, 1, 2);
        gtk_box_pack_end(row, b_stop, 0, 0, 0);
        gtk_box_pack_end(row, b_pin, 0, 0, 0);
    }
    if t.status == TunnelStatus::Disconnected {
        pack_disconnected_tunnel_controls(row, t, snap.ssh_pid, locale);
    } else {
        pack_active_tunnel_controls(row, t, locale);
    }
    unsafe { gtk_box_pack_start(parent, row, 0, 1, 0) };
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
    let existing = snap
        .tunnels
        .iter()
        .find(|t| t.host == host && t.remote_port == p.port && t.status == TunnelStatus::Active);
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

fn populate_tunnels_section(parent: *mut GtkWidget, snap: &PortsSnapshot, locale: UiLocale) {
    if snap.tunnels.is_empty() {
        return;
    }
    pack_section_header(parent, &text::PORTS_SECTION_TUNNELS.for_locale(locale).to_string_lossy());
    for t in &snap.tunnels {
        pack_tunnel_row(parent, t, snap, locale);
    }
}

fn pack_pinned_header_with_restore(parent: *mut GtkWidget, snap: &PortsSnapshot, host: &str, locale: UiLocale) {
    let title = format!("{} · {host}", text::PORTS_SECTION_PINNED.for_locale(locale).to_string_lossy());
    let restore_lbl = text::PORTS_RESTORE_PINNED.for_locale(locale).to_string_lossy();
    let restore_act = PortRowAction::RestorePinned { host: host.to_owned(), ssh_pid: snap.ssh_pid, sid: snap.session_id };
    let b_restore = make_action_button(&restore_lbl, "ports-restore-btn", restore_act);
    let Ok(c_title) = CString::new(title) else { return };
    unsafe {
        gtk_widget_set_tooltip_text(b_restore, text::PORTS_RESTORE_PINNED_TIP.for_locale(locale).as_ptr());
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let lbl = gtk_label_new(c_title.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-header");
        gtk_box_pack_start(row, lbl, 1, 1, 0);
        gtk_box_pack_end(row, b_restore, 0, 0, 0);
        gtk_box_pack_start(parent, row, 0, 1, 2);
    }
}

fn format_pinned_desc(p: &PinnedTunnel, to_word: &str) -> String {
    let base = format!("{} :{} {to_word} :{}", p.host, p.remote_port, p.local_port);
    if p.process.is_empty() { base } else { format!("{base} · {}", p.process) }
}

fn pack_pinned_row(parent: *mut GtkWidget, p: &PinnedTunnel, to_word: &str, unpin_lbl: &str) {
    let row = make_row_box();
    let lbl = make_row_label(&format_pinned_desc(p, to_word));
    let act = PortRowAction::UnpinTunnel { host: p.host.clone(), remote_port: p.remote_port };
    let b_unpin = make_action_button(unpin_lbl, "ports-pin-btn-active", act);
    unsafe {
        gtk_box_pack_start(row, lbl, 1, 1, 2);
        gtk_box_pack_end(row, b_unpin, 0, 0, 0);
        gtk_box_pack_start(parent, row, 0, 1, 0);
    }
}

fn populate_pinned_section(parent: *mut GtkWidget, snap: &PortsSnapshot, host: &str, locale: UiLocale) {
    let host_pinned: Vec<&PinnedTunnel> = snap.pinned_tunnels.iter().filter(|p| p.host == host).collect();
    if host_pinned.is_empty() {
        return;
    }
    pack_pinned_header_with_restore(parent, snap, host, locale);
    let to_word = text::PORTS_TO_WORD.for_locale(locale).to_string_lossy();
    let unpin_lbl = text::PORTS_UNPIN.for_locale(locale).to_string_lossy();
    for p in host_pinned {
        if !snap.tunnels.iter().any(|t| t.host == p.host && t.remote_port == p.remote_port) {
            pack_pinned_row(parent, p, &to_word, &unpin_lbl);
        }
    }
}

fn populate_remote_section(parent: *mut GtkWidget, snap: &PortsSnapshot, host: &str, locale: UiLocale) {
    let hdr = format!("{} · {host}", text::PORTS_SECTION_REMOTE.for_locale(locale).to_string_lossy());
    pack_section_header(parent, &hdr);
    for p in &snap.remote_ports {
        pack_remote_port_row(parent, snap, p, host, locale);
    }
    let has_pinned = snap.pinned_tunnels.iter().any(|p| p.host == host);
    if snap.remote_ports.is_empty() && snap.tunnels.is_empty() && !has_pinned {
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
    populate_tunnels_section(parent, snap, locale);
    if let Some(host) = snap.remote_host.as_deref() {
        populate_pinned_section(parent, snap, host, locale);
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
    let ephemeral = port_radar::read_ephemeral_port_range();
    for s in sessions {
        if let Some(host) = session_remote_host(s) {
            remote_targets.push(RemotePortTarget { host, ssh_pid: s.proc_info.ssh_pid });
            if s.id == active_id {
                active_remote_vte = Some((s.id, s.active_vte()));
            }
        } else {
            let pids = collect_session_local_pids(s);
            let detected = port_radar::detect_ports_for_pids_with_table(&pids, sockets);
            local_updates.push((s.id, port_radar::filter_radar_ports(detected, ephemeral)));
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
    let pinned_tunnels = crate::ssh_tunnel::list_pinned_tunnels();
    let Some(s) = app.sessions.iter().find(|s| s.id == app.active_id) else {
        return PortsSnapshot { tunnels, pinned_tunnels, ..PortsSnapshot::default() };
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
        pinned_tunnels,
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

fn reconnect_tunnel_row_action(id: u64, ssh_pid: i32, host: &str, remote_port: u16) {
    let res = app_state::with_app_mut(|app| app.ssh_tunnels.reconnect_tunnel_with_ssh_pid(id, ssh_pid));
    match res {
        Some(Ok(info)) => report_tunnel_footer(text::PORTS_TUNNEL_RECONNECTED, &info),
        Some(Err(err)) => report_tunnel_error(host, remote_port, &err.to_string()),
        None => {}
    }
    refresh_and_repopulate_popover(false);
    topbar::refresh_ui();
}

fn pin_tunnel_row_action(entry: PinnedTunnel) {
    let target = format!("{}:{}", entry.host, entry.remote_port);
    if crate::ssh_tunnel::pin_tunnel(entry).is_ok() {
        let prefix = text::PORTS_TUNNEL_PINNED.for_locale(i18n::current_locale()).to_string_lossy();
        app_state::set_footer_message(&format!("{prefix} {target}"));
    }
    refresh_and_repopulate_popover(false);
}

fn unpin_tunnel_row_action(host: &str, remote_port: u16) {
    if crate::ssh_tunnel::unpin_tunnel(host, remote_port).unwrap_or(false) {
        let prefix = text::PORTS_TUNNEL_UNPINNED.for_locale(i18n::current_locale()).to_string_lossy();
        app_state::set_footer_message(&format!("{prefix} {host}:{remote_port}"));
    }
    refresh_and_repopulate_popover(false);
}

fn report_restore_outcome(host: &str, restored: usize, errors: &[(u16, String)]) {
    let loc = i18n::current_locale();
    if restored > 0 {
        let prefix = text::PORTS_TUNNELS_RESTORED.for_locale(loc).to_string_lossy();
        app_state::set_footer_message(&format!("{prefix} {restored} ({host})"));
    } else if let Some((port, err)) = errors.first() {
        report_tunnel_error(host, *port, err);
    } else {
        app_state::set_footer_message(&text::PORTS_TUNNELS_NONE_TO_RESTORE.for_locale(loc).to_string_lossy());
    }
}

fn restore_pinned_row_action(host: &str, ssh_pid: i32, sid: usize) {
    let Some((restored, errors)) =
        app_state::with_app_mut(|app| app.ssh_tunnels.restore_pinned_for_host(host, ssh_pid, sid))
    else {
        return;
    };
    report_restore_outcome(host, restored.len(), &errors);
    refresh_and_repopulate_popover(false);
    topbar::refresh_ui();
}

fn handle_row_action(action: PortRowAction) {
    match action {
        PortRowAction::OpenUrl(url) => {
            dismiss_ports_popover();
            open_browser_url(&url);
        }
        PortRowAction::CopyUrl(url) => copy_port_url(&url),
        PortRowAction::ForwardAndOpen { sid, host, ssh_pid, port, scheme, process } => {
            dismiss_ports_popover();
            let req = TunnelRequest { host, ssh_pid, remote_port: port, scheme, process, session_id: sid };
            let _ = ensure_and_open_tunnel(req, None);
        }
        PortRowAction::StopTunnel { id, label } => stop_tunnel_row_action(id, &label),
        PortRowAction::ReconnectTunnel { id, ssh_pid, host, remote_port } => {
            reconnect_tunnel_row_action(id, ssh_pid, &host, remote_port);
        }
        PortRowAction::PinTunnel(entry) => pin_tunnel_row_action(entry),
        PortRowAction::UnpinTunnel { host, remote_port } => unpin_tunnel_row_action(&host, remote_port),
        PortRowAction::RestorePinned { host, ssh_pid, sid } => restore_pinned_row_action(&host, ssh_pid, sid),
    }
}

extern "C" fn on_row_action_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let action = unsafe { (&*data.cast::<PortRowAction>()).clone() };
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

fn repopulate_if_popover_open() {
    let pair = app_state::with_app(|app| (app.ports_menu.clone(), collect_active_snapshot(app)));
    if let Some((menu, snap)) = pair {
        if menu.is_visible() {
            menu.populate(&snap, i18n::current_locale());
        }
    }
}

extern "C" fn on_remote_ports_ready(_data: *mut c_void) -> c_int {
    repopulate_if_popover_open();
    topbar::refresh_ui();
    0
}

pub(crate) fn remote_ports_ready_cb() -> extern "C" fn(*mut c_void) -> c_int {
    on_remote_ports_ready
}

#[cfg(test)]
#[path = "ports_ui_tests.rs"]
mod tests;
