//! Right-side file browser drawer for active local and SSH panes.

use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_uint, c_void, CStr};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::app::app_state::{connect_click, connect_entry_changed, set_footer_message};
use crate::ffi::*;
use crate::files::drawer_rows::{clear_container, empty_notice, pack_drawer_row};
use crate::files::drawer_transfer::{on_upload_clicked, setup_drawer_drop};
use crate::files::local_fs::{default_downloads_dir, list_local_dir};
use crate::files::resolve::{endpoint_short_label, resolve_active_endpoint};
use crate::files::ssh_ops::list_remote_dir;
use crate::files::{FileEndpoint, FileEntry};
use crate::telemetry::ssh_connection::SshRoute;
use crate::ui_text::i18n_text::*;

const DRAWER_WIDTH_PX: i32 = 300;
const KEYCODE_O: u16 = 32;
static REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);

struct FilesDrawer {
    root: *mut GtkWidget,
    labels: [*mut GtkWidget; 3],
    btns: [*mut GtkWidget; 4],
    search_entry: *mut GtkWidget,
    rows_box: *mut GtkWidget,
    endpoint: Option<FileEndpoint>,
    entries: Vec<FileEntry>,
    show_hidden: bool,
    query: String,
    loading: bool,
}

thread_local! {
    static DRAWER: RefCell<Option<FilesDrawer>> = const { RefCell::new(None) };
    static TOPBAR_BTN: Cell<*mut GtkWidget> = const { Cell::new(ptr::null_mut()) };
}

/// Filter directory entries case-insensitively (safe for Cyrillic & UTF-8 names).
pub(crate) fn filter_entries(entries: &[FileEntry], query: &str) -> Vec<FileEntry> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return entries.to_vec();
    }
    let mut out = Vec::new();
    for entry in entries {
        if entry.name.to_lowercase().contains(&needle) {
            out.push(entry.clone());
        }
    }
    out
}

/// Match `Ctrl+Shift+O` across Latin `'O'/'o'` and Cyrillic `'Щ'/'щ'` (keycode 32).
pub fn is_drawer_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    let mods = GDK_CONTROL_MASK | GDK_SHIFT_MASK | GDK_MOD1_MASK;
    if (state & mods) != (GDK_CONTROL_MASK | GDK_SHIFT_MASK) {
        return false;
    }
    matches!(char::from_u32(keyval), Some('O' | 'o')) || keycode == KEYCODE_O
}

/// Handle `Ctrl+Shift+O` by toggling the drawer when matched.
pub fn try_toggle_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if !is_drawer_shortcut(keyval, state, keycode) {
        return false;
    }
    toggle();
    true
}

extern "C" fn on_topbar_toggle(_btn: *mut GtkWidget, _data: *mut c_void) {
    if TOPBAR_BTN.get().is_null() {
        return;
    }
    toggle();
}

extern "C" fn on_topbar_btn_destroy(w: *mut GtkWidget, _: *mut c_void) {
    let cur = TOPBAR_BTN.get();
    if cur == w {
        TOPBAR_BTN.set(ptr::null_mut());
    }
}

/// Create and mount the topbar Files toggle button cleanly inside `topbar`.
pub fn mount_topbar_button(topbar: *mut GtkWidget) {
    unsafe {
        let btn = gtk_button_new_with_label(FILES_BTN.cstr().as_ptr());
        gtk_widget_set_tooltip_text(btn, FILES_TIP.cstr().as_ptr());
        gtk_box_pack_end(topbar, btn, 0, 0, 0);
        connect_click(btn, on_topbar_toggle, ptr::null_mut());
        g_signal_connect_data(btn, c"destroy".as_ptr(), on_topbar_btn_destroy as *const c_void, ptr::null_mut(), ptr::null(), 0);
        TOPBAR_BTN.set(btn);
    }
}

fn refresh_topbar_btn() {
    let btn = TOPBAR_BTN.get();
    if btn.is_null() {
        return;
    }
    unsafe {
        gtk_button_set_label(btn as *mut GtkButton, FILES_BTN.cstr().as_ptr());
        gtk_widget_set_tooltip_text(btn, FILES_TIP.cstr().as_ptr());
    }
}

impl FilesDrawer {
    fn render_rows(&self) {
        clear_container(self.rows_box);
        if self.loading {
            return empty_notice(self.rows_box, FILES_LOADING.as_str());
        }
        let Some(ref ep) = self.endpoint else { return };
        let visible = filter_entries(&self.entries, &self.query);
        if visible.is_empty() {
            return empty_notice(self.rows_box, FILES_EMPTY.as_str());
        }
        for entry in &visible {
            pack_drawer_row(self.rows_box, entry, ep);
        }
    }

    fn apply_locale(&self) {
        set_label_text(self.labels[0], FILES_TITLE.as_str());
        set_label_text(self.labels[2], FILES_DROP_HINT.as_str());
        unsafe { gtk_entry_set_placeholder_text(self.search_entry, FILES_SEARCH_PLACEHOLDER.cstr().as_ptr()) };
        unsafe { gtk_button_set_label(self.btns[3] as *mut GtkButton, FILES_UPLOAD_BTN.cstr().as_ptr()) };
        let tips = [FILES_UP_TIP, FILES_REFRESH_TIP, FILES_HIDDEN_TIP, FILES_UPLOAD_TIP];
        for (btn, tip) in self.btns.iter().zip(tips) {
            unsafe { gtk_widget_set_tooltip_text(*btn, tip.cstr().as_ptr()) };
        }
        if self.entries.is_empty() {
            self.render_rows();
        }
    }

    fn sync_header_path(&self) {
        let Some(ref ep) = self.endpoint else { return };
        set_label_text(self.labels[1], &endpoint_short_label(ep));
        let full = match ep {
            FileEndpoint::Local { cwd } => cwd.to_string_lossy().into_owned(),
            FileEndpoint::Remote { cwd, .. } => cwd.clone(),
        };
        set_tooltip(self.labels[1], &full);
    }
}

fn finish_remote_load(seq: u64, resolved: String, res: std::io::Result<Vec<FileEntry>>) {
    if REQUEST_SEQ.load(Ordering::Relaxed) != seq {
        return;
    }
    DRAWER.with_borrow_mut(|slot| {
        let Some(d) = slot else { return };
        d.loading = false;
        match res {
            Ok(entries) => {
                if let Some(FileEndpoint::Remote { ref mut cwd, .. }) = d.endpoint { *cwd = resolved; }
                d.entries = entries;
            }
            Err(err) => { d.entries.clear(); set_footer_message(&err.to_string()); }
        }
        d.sync_header_path();
        d.render_rows();
    });
}

extern "C" fn invoke_idle(data: *mut c_void) -> c_int {
    if data.is_null() {
        return 0;
    }
    let cb = unsafe { Box::from_raw(data.cast::<Box<dyn FnOnce()>>()) };
    cb();
    0
}

fn spawn_remote_listing(route: SshRoute, dir: String, show_hidden: bool) {
    let seq = REQUEST_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        let res = list_remote_dir(&route, &dir, show_hidden);
        let cb: Box<dyn FnOnce()> = Box::new(move || match res {
            Ok((p, list)) => finish_remote_load(seq, p, Ok(list)),
            Err(e) => finish_remote_load(seq, dir, Err(e)),
        });
        unsafe { g_idle_add(invoke_idle, Box::into_raw(Box::new(cb)).cast()) };
    });
}

fn load_local_into_drawer(cwd: PathBuf, show_hidden: bool) {
    let (resolved, entries) = match list_local_dir(&cwd, show_hidden) {
        Ok(pair) => pair,
        Err(err) => { set_footer_message(&err.to_string()); (cwd, Vec::new()) }
    };
    DRAWER.with_borrow_mut(|slot| {
        let Some(d) = slot else { return };
        d.loading = false;
        d.endpoint = Some(FileEndpoint::Local { cwd: resolved });
        d.entries = entries;
        d.sync_header_path();
        d.render_rows();
    });
}

fn load_remote_into_drawer(ep: FileEndpoint, show_hidden: bool) {
    let FileEndpoint::Remote { ref route, ref cwd, .. } = ep else { return };
    let (route, cwd) = (route.clone(), cwd.clone());
    DRAWER.with_borrow_mut(|slot| {
        let Some(d) = slot else { return };
        d.loading = true;
        d.endpoint = Some(ep);
        d.sync_header_path();
        d.render_rows();
    });
    spawn_remote_listing(route, cwd, show_hidden);
}

fn load_endpoint_into_drawer(ep: FileEndpoint) {
    let show_hidden = DRAWER.with_borrow(|slot| slot.as_ref().is_some_and(|d| d.show_hidden));
    match ep {
        FileEndpoint::Local { cwd } => load_local_into_drawer(cwd, show_hidden),
        remote @ FileEndpoint::Remote { .. } => load_remote_into_drawer(remote, show_hidden),
    }
}

pub(crate) fn current_endpoint() -> Option<FileEndpoint> {
    DRAWER.with_borrow(|slot| {
        let Some(drawer) = slot else { return None };
        drawer.endpoint.clone()
    })
}

/// Navigate the open drawer to `new_dir`.
pub fn navigate_to(new_dir: &str) {
    let Some(ep) = current_endpoint() else { return };
    let next = match ep {
        FileEndpoint::Local { .. } => FileEndpoint::Local { cwd: PathBuf::from(new_dir) },
        FileEndpoint::Remote { sid, host, ssh_pid, route, .. } => {
            FileEndpoint::Remote { sid, host, ssh_pid, route, cwd: new_dir.to_owned() }
        }
    };
    load_endpoint_into_drawer(next);
}

fn parent_path_of(ep: &FileEndpoint) -> Option<String> {
    match ep {
        FileEndpoint::Local { cwd } => cwd.parent().map(|p| p.to_string_lossy().into_owned()),
        FileEndpoint::Remote { cwd, .. } => {
            let p = Path::new(cwd).parent().and_then(|p| p.to_str()).unwrap_or("");
            let fallback = if cwd.starts_with('/') { "/" } else { ".." };
            Some(if p.is_empty() { fallback } else { p }.to_owned())
        }
    }
}

extern "C" fn on_up(_: *mut GtkWidget, _: *mut c_void) {
    let Some(ep) = current_endpoint() else { return };
    if let Some(dir) = parent_path_of(&ep) {
        navigate_to(&dir);
    }
}

extern "C" fn on_refresh(_: *mut GtkWidget, _: *mut c_void) {
    let Some(ep) = current_endpoint() else { return };
    load_endpoint_into_drawer(ep);
}

extern "C" fn on_toggle_hidden(_: *mut GtkWidget, _: *mut c_void) {
    let ep = DRAWER.with_borrow_mut(|slot| {
        let d = slot.as_mut()?;
        d.show_hidden = !d.show_hidden;
        d.endpoint.clone()
    });
    if let Some(ep) = ep {
        load_endpoint_into_drawer(ep);
    }
}

extern "C" fn on_close(_: *mut GtkWidget, _: *mut c_void) {
    let open = is_open();
    if open {
        close();
    }
}

extern "C" fn on_search_changed(entry: *mut GtkWidget, _: *mut c_void) {
    let text = unsafe { CStr::from_ptr(gtk_entry_get_text(entry)) }.to_string_lossy().into_owned();
    DRAWER.with_borrow_mut(|slot| {
        let Some(d) = slot else { return };
        d.query = text;
        d.render_rows();
    });
}

fn header_btn(label: &CStr, cb: extern "C" fn(*mut GtkWidget, *mut c_void)) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new_with_label(label.as_ptr());
        add_class(btn, "files-row-btn");
        gtk_widget_set_can_focus(btn, 0);
        connect_click(btn, cb, ptr::null_mut());
        btn
    }
}

unsafe fn make_header_bar() -> (*mut GtkWidget, *mut GtkWidget, [*mut GtkWidget; 4]) {
    let bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
    let title_lbl = gtk_label_new(FILES_TITLE.cstr().as_ptr());
    add_class(title_lbl, "files-header");
    gtk_label_set_xalign(title_lbl, 0.0);
    gtk_box_pack_start(bar as *mut GtkBox, title_lbl, 1, 1, 0);
    let btns = [
        header_btn(c"↑", on_up), header_btn(c"↻", on_refresh),
        header_btn(c".", on_toggle_hidden), header_btn(FILES_UPLOAD_BTN.cstr(), on_upload_clicked),
    ];
    for b in btns {
        gtk_box_pack_start(bar as *mut GtkBox, b, 0, 0, 0);
    }
    gtk_box_pack_start(bar as *mut GtkBox, header_btn(c"×", on_close), 0, 0, 0);
    (bar, title_lbl, btns)
}

unsafe fn make_path_search_rows() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let path_lbl = gtk_label_new(c"".as_ptr());
    add_class(path_lbl, "files-path-label");
    gtk_label_set_xalign(path_lbl, 0.0);
    gtk_label_set_ellipsize(path_lbl, 3);
    let search = gtk_entry_new();
    add_class(search, "files-search");
    connect_entry_changed(search, on_search_changed);
    let hint = gtk_label_new(FILES_DROP_HINT.cstr().as_ptr());
    add_class(hint, "files-hint");
    gtk_label_set_xalign(hint, 0.0);
    (path_lbl, search, hint)
}

unsafe fn make_scroll_list() -> (*mut GtkWidget, *mut GtkWidget) {
    let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
    gtk_scrolled_window_set_policy(scroll, 2, 1);
    let rows_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
    gtk_container_add(scroll as *mut GtkContainer, rows_box);
    (scroll, rows_box)
}

extern "C" fn on_root_destroy(w: *mut GtkWidget, _: *mut c_void) {
    DRAWER.with_borrow_mut(|slot| {
        if slot.as_ref().is_some_and(|d| d.root == w) {
            *slot = None;
        }
    });
}

fn build_drawer_widgets() -> FilesDrawer {
    unsafe {
        let root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(root, "files-drawer");
        gtk_widget_set_size_request(root, DRAWER_WIDTH_PX, -1);
        g_signal_connect_data(root, c"destroy".as_ptr(), on_root_destroy as *const c_void, ptr::null_mut(), ptr::null(), 0);
        let (bar, title_lbl, btns) = make_header_bar();
        let (path_lbl, search_entry, hint_lbl) = make_path_search_rows();
        let (scroll, rows_box) = make_scroll_list();
        for w in [bar, path_lbl, search_entry] {
            gtk_box_pack_start(root as *mut GtkBox, w, 0, 1, 0);
        }
        gtk_box_pack_start(root as *mut GtkBox, scroll, 1, 1, 0);
        gtk_box_pack_start(root as *mut GtkBox, hint_lbl, 0, 1, 0);
        setup_drawer_drop(root);
        FilesDrawer {
            root, labels: [title_lbl, path_lbl, hint_lbl], btns, search_entry, rows_box,
            endpoint: None, entries: Vec::new(), show_hidden: false, query: String::new(), loading: false,
        }
    }
}

/// Mount the Files drawer as the collapsible right pane of `parent`.
pub fn mount(parent: *mut GtkWidget, main_child: *mut GtkWidget) {
    let drawer = build_drawer_widgets();
    drawer.apply_locale();
    unsafe {
        let paned = gtk_paned_new(GTK_ORIENTATION_HORIZONTAL);
        gtk_paned_pack1(paned, main_child, 1, 1);
        gtk_paned_pack2(paned, drawer.root, 0, 1);
        gtk_box_pack_start(parent as *mut GtkBox, paned, 1, 1, 0);
        gtk_widget_show_all(paned);
        gtk_widget_set_no_show_all(drawer.root, 1);
        gtk_widget_set_visible(drawer.root, 0);
    }
    DRAWER.with_borrow_mut(|slot| *slot = Some(drawer));
}

/// Whether the Files drawer is currently visible.
pub fn is_open() -> bool {
    DRAWER.with_borrow(|slot| {
        let Some(d) = slot else { return false };
        unsafe { gtk_widget_get_visible(d.root) != 0 }
    })
}

/// Open the drawer and load the active tab/pane directory.
pub fn open() {
    DRAWER.with_borrow(|slot| {
        let Some(d) = slot else { return };
        unsafe { gtk_widget_set_visible(d.root, 1) };
    });
    let ep = resolve_active_endpoint().unwrap_or_else(|| FileEndpoint::Local { cwd: default_downloads_dir() });
    load_endpoint_into_drawer(ep);
}

/// Hide the Files drawer.
pub fn close() {
    DRAWER.with_borrow(|slot| {
        let Some(d) = slot else { return };
        unsafe { gtk_widget_set_visible(d.root, 0) };
    });
}

/// Toggle visibility of the Files drawer.
pub fn toggle() {
    if is_open() {
        close();
    } else {
        open();
    }
}

/// Refresh drawer contents when switching tabs while open.
pub fn sync_active_tab() {
    if !is_open() {
        return;
    }
    if let Some(ep) = resolve_active_endpoint() {
        load_endpoint_into_drawer(ep);
    }
}

/// Refresh localized labels, hints, and tooltips across the drawer and topbar button.
pub fn refresh_locale() {
    refresh_topbar_btn();
    DRAWER.with_borrow(|slot| {
        let Some(d) = slot else { return };
        d.apply_locale();
    });
}

fn focused_drawer_search(win: *mut GtkWidget) -> Option<*mut GtkWidget> {
    let focus = unsafe { gtk_window_get_focus(win) };
    DRAWER.with_borrow(|slot| {
        let d = slot.as_ref()?;
        let vis = unsafe { gtk_widget_get_visible(d.root) != 0 };
        (vis && !focus.is_null() && unsafe { gtk_widget_is_ancestor(focus, d.root) != 0 }).then_some(d.search_entry)
    })
}

/// Intercept Escape / typing when keyboard focus is inside the Files drawer.
pub fn route_key(win: *mut GtkWidget, key: c_uint, mods: c_uint) -> Option<c_int> {
    let search = focused_drawer_search(win)?;
    if key == 0xff1b {
        if DRAWER.with_borrow(|s| s.as_ref().is_some_and(|d| !d.query.is_empty())) {
            unsafe { gtk_entry_set_text(search, c"".as_ptr()) };
        } else {
            close();
        }
        return Some(1);
    }
    let mask = mods & (GDK_CONTROL_MASK | GDK_SHIFT_MASK | GDK_MOD1_MASK);
    (!((mask & GDK_MOD1_MASK) != 0 || mask == (GDK_CONTROL_MASK | GDK_SHIFT_MASK))).then_some(0)
}

#[cfg(test)]
pub(crate) fn test_drive_local_and_filter(dir: &Path, query: &str) -> (usize, usize) {
    load_endpoint_into_drawer(FileEndpoint::Local { cwd: dir.to_path_buf() });
    let initial = DRAWER.with_borrow(|s| s.as_ref().map_or(0, |d| filter_entries(&d.entries, &d.query).len()));
    if let Ok(c) = std::ffi::CString::new(query) {
        let entry = DRAWER.with_borrow(|s| s.as_ref().map(|d| d.search_entry)).unwrap_or(ptr::null_mut());
        if !entry.is_null() { unsafe { gtk_entry_set_text(entry, c.as_ptr()) }; }
    }
    let filtered = DRAWER.with_borrow(|s| s.as_ref().map_or(0, |d| filter_entries(&d.entries, &d.query).len()));
    (initial, filtered)
}
