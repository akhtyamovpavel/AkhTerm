//! Dynamic Topbar Remote indicator button and QR pairing / device-revocation popover.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CString};
use std::ptr;
use std::rc::Rc;

use crate::app_state;
use crate::ffi::*;
use crate::remote::qr::{draw_qr_matrix, encode_qr, QrMatrix};
use crate::remote::wire;
use crate::{topbar, ui_text};

#[link(name = "gtk-3")]
extern "C" {
    fn gtk_drawing_area_new() -> *mut GtkWidget;
    fn gtk_widget_set_size_request(widget: *mut GtkWidget, width: c_int, height: c_int);
    fn gtk_widget_queue_draw(widget: *mut GtkWidget);
    fn gtk_widget_get_allocated_width(widget: *mut GtkWidget) -> c_int;
    fn gtk_widget_get_allocated_height(widget: *mut GtkWidget) -> c_int;
    fn gtk_clipboard_get(selection: GdkAtom) -> *mut GtkClipboard;
    fn gtk_clipboard_set_text(clipboard: *mut GtkClipboard, text: *const c_char, len: c_int);
    fn gtk_container_foreach(
        container: *mut GtkWidget,
        callback: extern "C" fn(*mut GtkWidget, *mut c_void),
        data: *mut c_void,
    );
}

struct RemoteMenuInner {
    button: *mut GtkWidget,
    popover: *mut GtkWidget,
    header_lbl: *mut GtkWidget,
    status_lbl: *mut GtkWidget,
    qr_area: *mut GtkWidget,
    url_lbl: *mut GtkWidget,
    devices_box: *mut GtkWidget,
    rotate_btn: *mut GtkWidget,
    copy_btn: *mut GtkWidget,
    settings_btn: *mut GtkWidget,
    stop_btn: *mut GtkWidget,
    current_qr: RefCell<Option<QrMatrix>>,
    current_url: RefCell<String>,
}

#[derive(Clone)]
pub struct RemoteMenu {
    inner: Rc<RemoteMenuInner>,
}

impl RemoteMenu {
    pub fn new(topbar: *mut GtkWidget) -> Self {
        let button = unsafe {
            let b = gtk_button_new_with_label(ui_text::remote_btn().as_ptr());
            gtk_widget_set_tooltip_text(b, ui_text::remote_tip().as_ptr());
            add_class(b, "remote-active-btn");
            gtk_box_pack_end(topbar, b, 0, 0, 0);
            gtk_widget_set_visible(b, 0);
            gtk_widget_set_no_show_all(b, 1);
            b
        };
        let inner = build_popover_widgets(button);
        let menu = Self { inner };
        connect_menu_signals(&menu);
        menu
    }

    #[allow(dead_code)]
    pub fn button_widget(&self) -> *mut GtkWidget {
        self.inner.button
    }

    pub fn sync_state(&self) {
        let info = wire::running_info();
        unsafe {
            gtk_widget_set_visible(self.inner.button, c_int::from(info.is_some()));
        }
        let Some(info) = info else { return };
        let word = ui_text::remote_btn_word().to_string_lossy();
        let label = if info.connected_clients > 0 {
            format!("{word} · {} ▾", info.connected_clients)
        } else {
            format!("{word} ▾")
        };
        let c_lbl = CString::new(label).unwrap_or_default();
        unsafe {
            gtk_button_set_label(self.inner.button, c_lbl.as_ptr());
            gtk_widget_set_tooltip_text(self.inner.button, ui_text::remote_tip().as_ptr());
        }
    }

    pub fn open_popover(&self) {
        if !wire::is_running() {
            topbar::open_settings_for_app();
            return;
        }
        self.sync_state();
        self.populate_popover(false);
        unsafe { gtk_popover_popup(self.inner.popover) };
    }

    fn populate_popover(&self, rotate: bool) {
        refresh_static_labels(&self.inner);
        let Some(info) = wire::running_info() else { return };
        let status_txt = format!(
            "{}:{} · {} {}",
            info.bind_ip,
            info.port,
            info.connected_clients,
            ui_text::remote_live_clients().to_string_lossy(),
        );
        set_label_text(self.inner.status_lbl, &status_txt);
        let pair_res = if rotate { wire::rotate_pairing_url() } else { wire::ensure_pairing_url() };
        if let Ok((url, ttl)) = pair_res {
            *self.inner.current_qr.borrow_mut() = encode_qr(url.as_bytes()).ok();
            *self.inner.current_url.borrow_mut() = url.clone();
            let short = format!("{url}\n({ttl}s)");
            set_label_text(self.inner.url_lbl, &short);
            unsafe { gtk_widget_queue_draw(self.inner.qr_area) };
        }
        rebuild_devices_list(&self.inner);
    }
}

fn build_popover_widgets(button: *mut GtkWidget) -> Rc<RemoteMenuInner> {
    unsafe {
        let popover = gtk_popover_new(button);
        let root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
        add_class(root, "popover-box");
        let header_lbl = make_lbl(ui_text::remote_header().as_ptr(), "section-header");
        let status_lbl = make_lbl(c"".as_ptr(), "settings-row-hint");
        let qr_area = gtk_drawing_area_new();
        gtk_widget_set_size_request(qr_area, 212, 212);
        let url_lbl = make_lbl(c"".as_ptr(), "settings-row-hint");
        gtk_label_set_line_wrap(url_lbl, 1);
        gtk_label_set_max_width_chars(url_lbl, 38);
        let (qr_btns, rotate_btn, copy_btn) = build_action_pair(
            ui_text::remote_rotate_qr().as_ptr(),
            ui_text::remote_copy_url().as_ptr(),
        );
        let devices_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        let (foot_btns, settings_btn, stop_btn) = build_action_pair(
            ui_text::settings_btn().as_ptr(),
            ui_text::remote_stop_btn().as_ptr(),
        );
        for w in [header_lbl, status_lbl, qr_area, url_lbl, qr_btns, devices_box, foot_btns] {
            gtk_box_pack_start(root, w, 0, 1, 0);
        }
        gtk_container_add(popover, root);
        gtk_widget_show_all(root);
        Rc::new(RemoteMenuInner {
            button, popover, header_lbl, status_lbl, qr_area, url_lbl, devices_box,
            rotate_btn, copy_btn, settings_btn, stop_btn,
            current_qr: RefCell::new(None), current_url: RefCell::new(String::new()),
        })
    }
}

unsafe fn make_lbl(text: *const c_char, class_name: &str) -> *mut GtkWidget {
    let lbl = gtk_label_new(text);
    gtk_label_set_xalign(lbl, 0.0);
    add_class(lbl, class_name);
    lbl
}

unsafe fn build_action_pair(a_lbl: *const c_char, b_lbl: *const c_char) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
    let a = gtk_button_new_with_label(a_lbl);
    let b = gtk_button_new_with_label(b_lbl);
    add_class(a, "settings-preset-btn");
    add_class(b, "settings-preset-btn");
    gtk_box_pack_start(row, a, 1, 1, 0);
    gtk_box_pack_start(row, b, 1, 1, 0);
    (row, a, b)
}

fn refresh_static_labels(inner: &RemoteMenuInner) {
    set_label_text(inner.header_lbl, &ui_text::remote_header().to_string_lossy());
    unsafe {
        gtk_button_set_label(inner.rotate_btn, ui_text::remote_rotate_qr().as_ptr());
        gtk_button_set_label(inner.copy_btn, ui_text::remote_copy_url().as_ptr());
        gtk_button_set_label(inner.settings_btn, ui_text::settings_btn().as_ptr());
        gtk_button_set_label(inner.stop_btn, ui_text::remote_stop_btn().as_ptr());
    }
}

extern "C" fn destroy_child(child: *mut GtkWidget, _data: *mut c_void) {
    unsafe { gtk_widget_destroy(child) };
}

fn rebuild_devices_list(inner: &Rc<RemoteMenuInner>) {
    unsafe { gtk_container_foreach(inner.devices_box, destroy_child, ptr::null_mut()) };
    let devices = wire::list_devices();
    let hdr = unsafe { make_lbl(ui_text::remote_devices_header().as_ptr(), "section-header") };
    unsafe { gtk_box_pack_start(inner.devices_box, hdr, 0, 1, 2) };
    if devices.is_empty() {
        let empty = unsafe { make_lbl(ui_text::remote_no_devices().as_ptr(), "settings-row-hint") };
        unsafe { gtk_box_pack_start(inner.devices_box, empty, 0, 1, 0) };
    } else {
        for dev in devices {
            pack_device_row(inner, &dev.device_id, &format!("{} ({})", dev.device_name, dev.platform));
        }
    }
    unsafe { gtk_widget_show_all(inner.devices_box) };
}

struct RevokePayload {
    inner: Rc<RemoteMenuInner>,
    device_id: String,
}

fn pack_device_row(inner: &Rc<RemoteMenuInner>, device_id: &str, label: &str) {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let c_txt = CString::new(label).unwrap_or_default();
        let lbl = make_lbl(c_txt.as_ptr(), "settings-row-title");
        let btn = gtk_button_new_with_label(ui_text::remote_revoke_btn().as_ptr());
        add_class(btn, "settings-preset-btn");
        gtk_box_pack_start(row, lbl, 1, 1, 0);
        gtk_box_pack_end(row, btn, 0, 0, 0);
        gtk_box_pack_start(inner.devices_box, row, 0, 1, 0);
        let payload = Box::into_raw(Box::new(RevokePayload {
            inner: Rc::clone(inner),
            device_id: device_id.to_owned(),
        }));
        g_signal_connect_data(
            btn, c"clicked".as_ptr(), on_revoke_clicked as *const c_void,
            payload.cast(), drop_revoke_payload as *const c_void, 0,
        );
    }
}

fn connect_menu_signals(menu: &RemoteMenu) {
    let connect = |widget: *mut GtkWidget, sig: &std::ffi::CStr, cb: *const c_void| {
        let ptr = Rc::into_raw(Rc::clone(&menu.inner)) as *mut c_void;
        unsafe {
            g_signal_connect_data(widget, sig.as_ptr(), cb, ptr, drop_inner as *const c_void, 0);
        }
    };
    connect(menu.inner.button, c"clicked", on_button_clicked as *const c_void);
    connect(menu.inner.qr_area, c"draw", on_qr_draw as *const c_void);
    connect(menu.inner.rotate_btn, c"clicked", on_rotate_clicked as *const c_void);
    connect(menu.inner.copy_btn, c"clicked", on_copy_clicked as *const c_void);
    connect(menu.inner.settings_btn, c"clicked", on_open_settings as *const c_void);
    connect(menu.inner.stop_btn, c"clicked", on_stop_clicked as *const c_void);
}

extern "C" fn drop_inner(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Rc::from_raw(data.cast::<RemoteMenuInner>())) };
    }
}

extern "C" fn drop_revoke_payload(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data.cast::<RevokePayload>())) };
    }
}

extern "C" fn on_button_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let inner = unsafe { &*data.cast::<RemoteMenuInner>() };
    let menu = RemoteMenu { inner: unsafe { Rc::increment_strong_count(inner); Rc::from_raw(inner) } };
    menu.open_popover();
}

extern "C" fn on_qr_draw(area: *mut GtkWidget, cr: *mut cairo_t, data: *mut c_void) -> c_int {
    let inner = unsafe { &*data.cast::<RemoteMenuInner>() };
    if let Some(qr) = inner.current_qr.borrow().as_ref() {
        let w = unsafe { gtk_widget_get_allocated_width(area) } as f64;
        let h = unsafe { gtk_widget_get_allocated_height(area) } as f64;
        draw_qr_matrix(cr, qr, w, h);
    }
    0
}

extern "C" fn on_rotate_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let inner = unsafe { &*data.cast::<RemoteMenuInner>() };
    let menu = RemoteMenu { inner: unsafe { Rc::increment_strong_count(inner); Rc::from_raw(inner) } };
    menu.populate_popover(true);
}

extern "C" fn on_copy_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let inner = unsafe { &*data.cast::<RemoteMenuInner>() };
    let url = inner.current_url.borrow().clone();
    if let Ok(c_url) = CString::new(url) {
        unsafe {
            let cb = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            if !cb.is_null() {
                gtk_clipboard_set_text(cb, c_url.as_ptr(), -1);
            }
        }
        app_state::set_footer_message(&ui_text::remote_url_copied().to_string_lossy());
    }
}

extern "C" fn on_open_settings(_btn: *mut GtkWidget, data: *mut c_void) {
    let inner = unsafe { &*data.cast::<RemoteMenuInner>() };
    unsafe { gtk_popover_popdown(inner.popover) };
    topbar::open_settings_for_app();
}

extern "C" fn on_stop_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let inner = unsafe { &*data.cast::<RemoteMenuInner>() };
    unsafe { gtk_popover_popdown(inner.popover) };
    let mut cfg = crate::remote::settings_io::load_remote_settings();
    cfg.enabled = false;
    let _ = crate::remote::settings_io::save_remote_settings(&cfg);
    let _ = wire::apply_settings(&cfg);
}

extern "C" fn on_revoke_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let payload = unsafe { &*data.cast::<RevokePayload>() };
    wire::revoke_device(&payload.device_id);
    rebuild_devices_list(&payload.inner);
}

#[link(name = "gtk-3")]
extern "C" {
    fn gtk_widget_set_no_show_all(widget: *mut GtkWidget, no_show_all: c_int);
    fn gtk_label_set_line_wrap(label: *mut GtkWidget, wrap: c_int);
    fn gtk_label_set_max_width_chars(label: *mut GtkWidget, n_chars: c_int);
}
