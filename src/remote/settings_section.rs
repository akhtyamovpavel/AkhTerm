//! Compact Mobile Remote Access settings card embedded inside SettingsDialog (`Ctrl+,`).

use std::cell::Cell;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::rc::Rc;

use crate::app_state;
use crate::ffi::{add_class, g_signal_connect_data, set_label_text, GtkWidget};
use crate::remote::auth::detect_bind_candidates;
use crate::remote::settings_io::{
    RemoteSettings, DEFAULT_REMOTE_BIND, MAX_REMOTE_PORT, MIN_REMOTE_PORT,
};
use crate::remote::wire;
use crate::ui_text;

const GTK_ORIENTATION_HORIZONTAL: c_int = 0;
const GTK_ORIENTATION_VERTICAL: c_int = 1;
const GTK_ALIGN_START: c_int = 1;
const GTK_ALIGN_CENTER: c_int = 3;

#[link(name = "gtk-3")]
extern "C" {
    fn gtk_box_new(orientation: c_int, spacing: c_int) -> *mut GtkWidget;
    fn gtk_box_pack_start(bx: *mut GtkWidget, child: *mut GtkWidget, expand: c_int, fill: c_int, pad: c_uint);
    fn gtk_box_pack_end(bx: *mut GtkWidget, child: *mut GtkWidget, expand: c_int, fill: c_int, pad: c_uint);
    fn gtk_label_new(text: *const c_char) -> *mut GtkWidget;
    fn gtk_label_set_line_wrap(label: *mut GtkWidget, wrap: c_int);
    fn gtk_switch_new() -> *mut GtkWidget;
    fn gtk_switch_set_active(sw: *mut GtkWidget, active: c_int);
    fn gtk_switch_get_active(sw: *mut GtkWidget) -> c_int;
    fn gtk_combo_box_text_new() -> *mut GtkWidget;
    fn gtk_combo_box_text_append(combo: *mut GtkWidget, id: *const c_char, text: *const c_char);
    fn gtk_combo_box_set_active_id(combo: *mut GtkWidget, id: *const c_char) -> c_int;
    fn gtk_combo_box_get_active_id(combo: *mut GtkWidget) -> *const c_char;
    fn gtk_spin_button_new_with_range(min: f64, max: f64, step: f64) -> *mut GtkWidget;
    fn gtk_spin_button_set_value(spin: *mut GtkWidget, value: f64);
    fn gtk_spin_button_get_value_as_int(spin: *mut GtkWidget) -> c_int;
    fn gtk_button_new_with_label(label: *const c_char) -> *mut GtkWidget;
    fn gtk_button_set_label(button: *mut GtkWidget, label: *const c_char);
    fn gtk_widget_set_hexpand(widget: *mut GtkWidget, expand: c_int);
    fn gtk_widget_set_halign(widget: *mut GtkWidget, align: c_int);
    fn gtk_widget_set_valign(widget: *mut GtkWidget, align: c_int);
    fn gtk_widget_set_sensitive(widget: *mut GtkWidget, sensitive: c_int);
}

struct RemoteSectionState {
    root: *mut GtkWidget,
    title_lbl: *mut GtkWidget,
    subtitle_lbl: *mut GtkWidget,
    enable_sw: *mut GtkWidget,
    bind_lbl: *mut GtkWidget,
    bind_combo: *mut GtkWidget,
    port_lbl: *mut GtkWidget,
    port_spin: *mut GtkWidget,
    mute_lbl: *mut GtkWidget,
    mute_sw: *mut GtkWidget,
    qr_btn: *mut GtkWidget,
    status_lbl: *mut GtkWidget,
    updating: Cell<bool>,
    on_changed: Rc<dyn Fn(RemoteSettings)>,
}

#[derive(Clone)]
pub(crate) struct RemoteSection {
    state: Rc<RemoteSectionState>,
}

impl RemoteSection {
    pub(crate) fn widget(&self) -> *mut GtkWidget {
        self.state.root
    }

    pub(crate) fn refresh_labels(&self) {
        let s = &self.state;
        set_label_text(s.title_lbl, &ui_text::settings_remote_title().to_string_lossy());
        set_label_text(s.subtitle_lbl, &ui_text::settings_remote_subtitle().to_string_lossy());
        set_label_text(s.bind_lbl, &ui_text::settings_remote_bind().to_string_lossy());
        set_label_text(s.port_lbl, &ui_text::settings_remote_port().to_string_lossy());
        set_label_text(s.mute_lbl, &ui_text::settings_remote_mute().to_string_lossy());
        unsafe { gtk_button_set_label(s.qr_btn, ui_text::settings_remote_qr_btn().as_ptr()) };
        refresh_status_line(s, None);
    }
}

fn make_lbl(text: &CStr, class_name: &str) -> *mut GtkWidget {
    let lbl = unsafe { gtk_label_new(text.as_ptr()) };
    unsafe {
        gtk_widget_set_halign(lbl, GTK_ALIGN_START);
        gtk_widget_set_valign(lbl, GTK_ALIGN_CENTER);
        add_class(lbl, class_name);
    }
    lbl
}

fn populate_bind_combo(combo: *mut GtkWidget, current_ip: &str) {
    let cands = detect_bind_candidates();
    let mut has_current = false;
    for c in &cands {
        if c.ip == current_ip {
            has_current = true;
        }
        let id = CString::new(c.ip.as_str()).unwrap_or_default();
        let lbl = CString::new(c.display_label()).unwrap_or_default();
        unsafe { gtk_combo_box_text_append(combo, id.as_ptr(), lbl.as_ptr()) };
    }
    if !has_current && !current_ip.is_empty() {
        let id = CString::new(current_ip).unwrap_or_default();
        unsafe { gtk_combo_box_text_append(combo, id.as_ptr(), id.as_ptr()) };
    }
    let active = CString::new(current_ip).unwrap_or_default();
    unsafe { gtk_combo_box_set_active_id(combo, active.as_ptr()) };
}

pub(crate) fn build_remote_section(
    initial: &RemoteSettings,
    on_changed: Rc<dyn Fn(RemoteSettings)>,
) -> RemoteSection {
    let norm = initial.normalized();
    let root = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 6) };
    add_class(root, "settings-card");
    let (hdr, title_lbl, enable_sw) = build_header_row(norm.enabled);
    let subtitle_lbl = make_lbl(ui_text::settings_remote_subtitle(), "settings-row-hint");
    let (net_row, bind_lbl, bind_combo, port_lbl, port_spin) = build_net_row(&norm);
    let (mute_row, mute_lbl, mute_sw) = build_mute_row(norm.mute_when_focused);
    let (act_row, qr_btn, status_lbl) = build_action_row();
    unsafe {
        gtk_label_set_line_wrap(subtitle_lbl, 1);
        for w in [hdr, subtitle_lbl, net_row, mute_row, act_row] {
            gtk_box_pack_start(root, w, 0, 1, 0);
        }
    }
    let state = Rc::new(RemoteSectionState {
        root, title_lbl, subtitle_lbl, enable_sw, bind_lbl, bind_combo, port_lbl, port_spin,
        mute_lbl, mute_sw, qr_btn, status_lbl, updating: Cell::new(false), on_changed,
    });
    connect_signals(&state);
    refresh_status_line(&state, None);
    RemoteSection { state }
}

fn build_header_row(enabled: bool) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let title = make_lbl(ui_text::settings_remote_title(), "settings-row-title");
    let sw = unsafe { gtk_switch_new() };
    unsafe {
        gtk_switch_set_active(sw, c_int::from(enabled));
        gtk_widget_set_valign(sw, GTK_ALIGN_CENTER);
        gtk_box_pack_start(row, title, 1, 1, 0);
        gtk_box_pack_end(row, sw, 0, 0, 0);
    }
    (row, title, sw)
}

fn build_net_row(cfg: &RemoteSettings) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let bind_lbl = make_lbl(ui_text::settings_remote_bind(), "settings-key");
    let bind_combo = unsafe { gtk_combo_box_text_new() };
    populate_bind_combo(bind_combo, &cfg.bind_ip);
    let port_lbl = make_lbl(ui_text::settings_remote_port(), "settings-key");
    let port_spin = unsafe {
        gtk_spin_button_new_with_range(f64::from(MIN_REMOTE_PORT), f64::from(MAX_REMOTE_PORT), 1.0)
    };
    unsafe {
        gtk_spin_button_set_value(port_spin, f64::from(cfg.port));
        gtk_widget_set_hexpand(bind_combo, 1);
        add_class(bind_combo, "settings-control");
        add_class(port_spin, "settings-control");
        gtk_box_pack_start(row, bind_lbl, 0, 0, 0);
        gtk_box_pack_start(row, bind_combo, 1, 1, 0);
        gtk_box_pack_start(row, port_lbl, 0, 0, 4);
        gtk_box_pack_start(row, port_spin, 0, 0, 0);
    }
    (row, bind_lbl, bind_combo, port_lbl, port_spin)
}

fn build_mute_row(mute: bool) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let lbl = make_lbl(ui_text::settings_remote_mute(), "settings-row-hint");
    let sw = unsafe { gtk_switch_new() };
    unsafe {
        gtk_label_set_line_wrap(lbl, 1);
        gtk_switch_set_active(sw, c_int::from(mute));
        gtk_widget_set_valign(sw, GTK_ALIGN_CENTER);
        gtk_box_pack_start(row, lbl, 1, 1, 0);
        gtk_box_pack_end(row, sw, 0, 0, 0);
    }
    (row, lbl, sw)
}

fn build_action_row() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let qr_btn = unsafe { gtk_button_new_with_label(ui_text::settings_remote_qr_btn().as_ptr()) };
    add_class(qr_btn, "settings-preset-btn");
    let status_lbl = make_lbl(c"", "settings-row-hint");
    unsafe {
        gtk_label_set_line_wrap(status_lbl, 1);
        gtk_box_pack_start(row, qr_btn, 0, 0, 0);
        gtk_box_pack_start(row, status_lbl, 1, 1, 0);
    }
    (row, qr_btn, status_lbl)
}

fn read_from_state(s: &RemoteSectionState) -> RemoteSettings {
    let enabled = unsafe { gtk_switch_get_active(s.enable_sw) } != 0;
    let mute_when_focused = unsafe { gtk_switch_get_active(s.mute_sw) } != 0;
    let bind_ptr = unsafe { gtk_combo_box_get_active_id(s.bind_combo) };
    let bind_ip = if bind_ptr.is_null() {
        DEFAULT_REMOTE_BIND.to_owned()
    } else {
        unsafe { CStr::from_ptr(bind_ptr) }.to_string_lossy().into_owned()
    };
    let port = unsafe { gtk_spin_button_get_value_as_int(s.port_spin) }
        .clamp(i32::from(MIN_REMOTE_PORT), i32::from(MAX_REMOTE_PORT)) as u16;
    RemoteSettings { enabled, bind_ip, port, mute_when_focused }.normalized()
}

fn refresh_status_line(s: &RemoteSectionState, error_msg: Option<&str>) {
    let running = wire::running_info();
    unsafe { gtk_widget_set_sensitive(s.qr_btn, c_int::from(running.is_some())) };
    let msg = if let Some(err) = error_msg {
        format!("Error: {err}")
    } else if let Some(info) = running {
        format!(
            "{} {}:{} ({} {})",
            ui_text::settings_remote_running().to_string_lossy(),
            info.bind_ip,
            info.port,
            info.paired_count,
            ui_text::remote_paired_word().to_string_lossy(),
        )
    } else {
        ui_text::settings_remote_stopped().to_string_lossy().into_owned()
    };
    set_label_text(s.status_lbl, &msg);
}

fn handle_change(state: &RemoteSectionState) {
    if state.updating.get() {
        return;
    }
    let cfg = read_from_state(state);
    let _ = crate::remote::settings_io::save_remote_settings(&cfg);
    let err = wire::apply_settings(&cfg).err();
    refresh_status_line(state, err.as_deref());
    (state.on_changed)(cfg);
}

fn connect_signals(state: &Rc<RemoteSectionState>) {
    let connect = |w: *mut GtkWidget, sig: &CStr, cb: *const c_void| {
        let ptr = Rc::into_raw(Rc::clone(state)) as *mut c_void;
        unsafe {
            g_signal_connect_data(w, sig.as_ptr(), cb, ptr, drop_state as *const c_void, 0);
        }
    };
    connect(state.enable_sw, c"notify::active", on_notify_active as *const c_void);
    connect(state.mute_sw, c"notify::active", on_notify_active as *const c_void);
    connect(state.bind_combo, c"changed", on_widget_changed as *const c_void);
    connect(state.port_spin, c"value-changed", on_widget_changed as *const c_void);
    connect(state.qr_btn, c"clicked", on_qr_btn_clicked as *const c_void);
}

extern "C" fn drop_state(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Rc::from_raw(data.cast::<RemoteSectionState>())) };
    }
}

extern "C" fn on_notify_active(_sw: *mut GtkWidget, _pspec: *mut c_void, data: *mut c_void) {
    let state = unsafe { &*data.cast::<RemoteSectionState>() };
    handle_change(state);
}

extern "C" fn on_widget_changed(_w: *mut GtkWidget, data: *mut c_void) {
    let state = unsafe { &*data.cast::<RemoteSectionState>() };
    handle_change(state);
}

extern "C" fn on_qr_btn_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let menu = app_state::with_app(|app| app.remote_menu.clone());
    if let Some(m) = menu {
        m.open_popover();
    }
}
