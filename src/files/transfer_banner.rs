//! Bottom-left floating progress banner for asynchronous file transfers.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::PathBuf;
use std::rc::Rc;

use crate::app::app_state::with_app_mut;
use crate::ffi::*;
use crate::files::format_bytes;
use crate::files::manager::{TransferDirection, TransferSnapshot, TransferStatus};
use crate::platform::open_containing_folder;
use crate::terminal::file_drop::{feed_vte_text, format_shell_paths};
use crate::ui_text::i18n_text::*;

const BANNER_WIDTH_PX: i32 = 340;
const BANNER_MARGIN_PX: i32 = 16;
const POLL_INTERVAL_MS: u32 = 120;

struct BannerInner {
    card: *mut GtkWidget,
    badge: *mut GtkWidget,
    title: *mut GtkWidget,
    dest: *mut GtkWidget,
    stats: *mut GtkWidget,
    pbar: *mut GtkWidget,
    cancel: *mut GtkWidget,
    open: *mut GtkWidget,
    insert: *mut GtkWidget,
    dismiss: *mut GtkWidget,
    last_id: Cell<u64>,
    open_dir: RefCell<Option<PathBuf>>,
    insert_target: RefCell<(*mut GtkWidget, Vec<String>)>,
    polling: Cell<bool>,
}

/// Floating transfer status card mounted in the main window `GtkOverlay`.
#[derive(Clone)]
pub struct TransferBanner {
    inner: Rc<BannerInner>,
}

fn new_lbl(cls: &str) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, cls);
        lbl
    }
}

fn new_btn(cls: &str) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new_with_label(c"".as_ptr());
        add_class(btn, cls);
        gtk_widget_set_can_focus(btn, 0);
        gtk_widget_set_no_show_all(btn, 1);
        btn
    }
}

extern "C" fn drop_inner(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        let boxed = unsafe { Rc::from_raw(data.cast::<BannerInner>()) };
        drop(boxed);
    }
}

extern "C" fn on_cancel(_: *mut GtkWidget, data: *mut c_void) {
    if let Some(inner) = unsafe { data.cast::<BannerInner>().as_ref() } {
        let id = inner.last_id.get();
        with_app_mut(|app| app.transfer_manager.cancel(id));
        sync_app_banner();
    }
}

extern "C" fn on_dismiss(_: *mut GtkWidget, data: *mut c_void) {
    if let Some(inner) = unsafe { data.cast::<BannerInner>().as_ref() } {
        let id = inner.last_id.get();
        with_app_mut(|app| app.transfer_manager.dismiss(id));
        sync_app_banner();
    }
}

extern "C" fn on_open(_: *mut GtkWidget, data: *mut c_void) {
    if let Some(inner) = unsafe { data.cast::<BannerInner>().as_ref() } {
        if let Some(ref dir) = *inner.open_dir.borrow() {
            let _ = open_containing_folder(dir);
        }
    }
}

extern "C" fn on_insert(_: *mut GtkWidget, data: *mut c_void) {
    if let Some(inner) = unsafe { data.cast::<BannerInner>().as_ref() } {
        let (vte, paths) = inner.insert_target.borrow().clone();
        if !vte.is_null() && !paths.is_empty() {
            let text = format_shell_paths(&paths);
            feed_vte_text(vte, &text, false);
        }
    }
}

fn wire_btn(
    btn: *mut GtkWidget,
    cb: extern "C" fn(*mut GtkWidget, *mut c_void),
    inner: &Rc<BannerInner>,
) {
    let raw = Rc::into_raw(Rc::clone(inner)) as *mut c_void;
    let sig = c"clicked".as_ptr();
    unsafe {
        g_signal_connect_data(btn, sig, cb as *const c_void, raw, drop_inner as *const c_void, 0);
    }
}

fn stats_text(snap: &TransferSnapshot) -> String {
    let done = format_bytes(snap.bytes_done);
    match &snap.status {
        TransferStatus::Running if snap.bytes_total > 0 => format!("{done} / {}", format_bytes(snap.bytes_total)),
        TransferStatus::Running => done,
        TransferStatus::Done => format!("{} ({done})", TRANSFER_DONE.as_str()),
        TransferStatus::Cancelled => TRANSFER_CANCELLED.as_str().to_owned(),
        TransferStatus::Failed(err) => format!("{}: {err}", TRANSFER_FAILED.as_str()),
    }
}

fn update_chrome(inner: &BannerInner, snap: &TransferSnapshot) {
    let running = snap.status == TransferStatus::Running;
    let done = snap.status == TransferStatus::Done;
    unsafe {
        if running && snap.bytes_total == 0 {
            gtk_progress_bar_pulse(inner.pbar);
        } else {
            let frac = if snap.bytes_total > 0 { (snap.bytes_done as f64 / snap.bytes_total as f64).clamp(0.0, 1.0) } else { 1.0 };
            gtk_progress_bar_set_fraction(inner.pbar, frac);
        }
        gtk_widget_set_visible(inner.cancel, i32::from(running));
        gtk_widget_set_visible(inner.open, i32::from(done && snap.local_open_dir.is_some()));
        gtk_widget_set_visible(inner.insert, i32::from(done && !snap.target_vte.is_null() && !snap.insert_paths.is_empty()));
        gtk_widget_set_visible(inner.dismiss, i32::from(!running));
        gtk_widget_set_visible(inner.card, 1);
    }
}

fn sync_app_banner() {
    let res = with_app_mut(|app| (app.transfer_manager.latest_snapshot(), app.transfer_manager.has_running()));
    if let Some((snap, running)) = res {
        crate::app::app_state::with_app(|app| {
            app.transfer_banner.apply_snapshot(snap.as_ref());
            if running {
                app.transfer_banner.ensure_polling();
            }
        });
    }
}

extern "C" fn on_poll_tick(data: *mut c_void) -> i32 {
    let running = with_app_mut(|app| {
        let snap = app.transfer_manager.latest_snapshot();
        app.transfer_banner.apply_snapshot(snap.as_ref());
        app.transfer_manager.has_running()
    })
    .unwrap_or(false);
    if !running {
        if let Some(inner) = unsafe { data.cast::<BannerInner>().as_ref() } {
            inner.polling.set(false);
        }
        return 0;
    }
    1
}

fn configure_card_geometry(card: *mut GtkWidget) {
    unsafe {
        gtk_widget_set_size_request(card, BANNER_WIDTH_PX, -1);
        gtk_widget_set_halign(card, GTK_ALIGN_START);
        gtk_widget_set_valign(card, GTK_ALIGN_END);
        for s in [
            gtk_widget_set_margin_start,
            gtk_widget_set_margin_end,
            gtk_widget_set_margin_top,
            gtk_widget_set_margin_bottom,
        ] {
            s(card, BANNER_MARGIN_PX);
        }
        gtk_widget_set_can_focus(card, 0);
        gtk_widget_set_no_show_all(card, 1);
        gtk_widget_set_visible(card, 0);
    }
}

fn build_banner_inner() -> Rc<BannerInner> {
    unsafe {
        let card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(card, "transfer-card");
        configure_card_geometry(card);
        let [badge, title, dest, stats] = [
            "transfer-badge", "transfer-title", "transfer-dest", "transfer-stats",
        ]
        .map(new_lbl);
        let pbar = gtk_progress_bar_new();
        add_class(pbar, "transfer-progress");
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let [cancel, open, insert, dismiss] = [
            "transfer-btn-danger", "transfer-btn", "transfer-btn", "transfer-btn",
        ]
        .map(new_btn);
        for b in [cancel, open, insert, dismiss] {
            gtk_box_pack_start(row as *mut GtkBox, b, 0, 0, 0);
        }
        for w in [badge, title, dest, pbar, stats, row] {
            gtk_box_pack_start(card as *mut GtkBox, w, 0, 0, 0);
        }
        Rc::new(BannerInner {
            card, badge, title, dest, stats, pbar, cancel, open, insert, dismiss,
            last_id: Cell::new(0), open_dir: RefCell::new(None),
            insert_target: RefCell::new((std::ptr::null_mut(), Vec::new())), polling: Cell::new(false),
        })
    }
}

impl Default for TransferBanner {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferBanner {
    /// Build the floating transfer banner widget hierarchy.
    pub fn new() -> Self {
        let inner = build_banner_inner();
        let pairs: [(*mut GtkWidget, extern "C" fn(*mut GtkWidget, *mut c_void)); 4] = [
            (inner.cancel, on_cancel),
            (inner.open, on_open),
            (inner.insert, on_insert),
            (inner.dismiss, on_dismiss),
        ];
        for (b, cb) in pairs {
            wire_btn(b, cb, &inner);
        }
        let banner = Self { inner };
        banner.refresh_locale();
        banner
    }

    /// Add the card as an overlay child on the main workspace overlay.
    pub fn mount_in_overlay(&self, overlay: *mut GtkWidget) {
        if !overlay.is_null() {
            unsafe {
                gtk_overlay_add_overlay(overlay, self.inner.card);
            }
        }
    }

    /// Update button captions and badge text for the current UI locale.
    pub fn refresh_locale(&self) {
        set_label_text(self.inner.badge, TRANSFER_BADGE.as_str());
        for (b, t) in [
            (self.inner.cancel, TRANSFER_BTN_CANCEL),
            (self.inner.open, TRANSFER_BTN_OPEN_FOLDER),
            (self.inner.insert, TRANSFER_BTN_INSERT_PATH),
            (self.inner.dismiss, TRANSFER_BTN_DISMISS),
        ] {
            unsafe { gtk_button_set_label(b as *mut GtkButton, t.cstr().as_ptr()) };
        }
    }

    /// Render the given transfer snapshot (or hide the card if `None`).
    pub fn apply_snapshot(&self, snap: Option<&TransferSnapshot>) {
        let Some(snap) = snap else {
            unsafe { gtk_widget_set_visible(self.inner.card, 0) };
            return;
        };
        let dir = match snap.direction {
            TransferDirection::LocalCopy => TRANSFER_LOCAL_COPY.as_str(),
            TransferDirection::SshUpload => TRANSFER_SSH_UPLOAD.as_str(),
            TransferDirection::SshDownload => TRANSFER_SSH_DOWNLOAD.as_str(),
            TransferDirection::SshToSsh => TRANSFER_SSH_BRIDGE.as_str(),
        };
        self.inner.last_id.set(snap.id);
        *self.inner.open_dir.borrow_mut() = snap.local_open_dir.clone();
        *self.inner.insert_target.borrow_mut() = (snap.target_vte, snap.insert_paths.clone());
        set_label_text(self.inner.title, &format!("{dir} · {}", snap.label));
        set_label_text(self.inner.dest, &snap.destination);
        set_label_text(self.inner.stats, &stats_text(snap));
        update_chrome(&self.inner, snap);
    }

    /// Start periodic main-loop polling while a transfer is active.
    pub fn ensure_polling(&self) {
        if !self.inner.polling.replace(true) {
            let ptr = Rc::as_ptr(&self.inner) as *mut c_void;
            unsafe {
                g_timeout_add(POLL_INTERVAL_MS, on_poll_tick, ptr);
            }
        }
    }
}

/// Synchronize the global `TransferBanner` with `TransferManager` and keep polling if running.
pub fn notify_transfer_started() {
    sync_app_banner();
}

#[cfg(test)]
pub(crate) fn banner_visible(banner: &TransferBanner) -> bool {
    unsafe { gtk_widget_get_visible(banner.inner.card) != 0 }
}

#[cfg(test)]
pub(crate) fn banner_card_ptr(banner: &TransferBanner) -> *mut GtkWidget {
    banner.inner.card
}
