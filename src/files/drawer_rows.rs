//! Interactive draggable file/folder row widgets for the side Files drawer.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;

use crate::app::app_state::{with_app, with_app_mut};
use crate::ffi::*;
use crate::files::local_fs::default_downloads_dir;
use crate::files::mime::*;
use crate::files::transfer_banner::notify_transfer_started;
use crate::files::{format_bytes, FileEndpoint, FileEntry, FileKind, RemoteFileItem};
use crate::telemetry::ssh_connection::SshRoute;
use crate::terminal::file_drop::insert_shell_paths;
use crate::ui_text::i18n_text::{FILES_DOWNLOAD_TIP, FILES_INSERT_TIP, FILES_OPEN_TIP};

struct RowPayload {
    entry: FileEntry,
    endpoint: FileEndpoint,
}

/// Remove and destroy every child widget inside `container`.
pub(crate) fn clear_container(container: *mut GtkWidget) {
    extern "C" fn destroy_child(w: *mut GtkWidget, _data: *mut c_void) {
        unsafe { gtk_widget_destroy(w) };
    }
    if !container.is_null() {
        unsafe { gtk_container_foreach(container as *mut GtkContainer, destroy_child, ptr::null_mut()) };
    }
}

/// Show a muted status/empty-state label inside `box_w`.
pub(crate) fn empty_notice(box_w: *mut GtkWidget, text: &str) {
    let c = CString::new(text).unwrap_or_default();
    unsafe {
        let lbl = gtk_label_new(c.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "files-path-label");
        gtk_box_pack_start(box_w as *mut GtkBox, lbl, 0, 0, 4);
        gtk_widget_show_all(lbl);
    }
}

/// Build and append one interactive file row into `rows_box`.
pub(crate) fn pack_drawer_row(rows_box: *mut GtkWidget, entry: &FileEntry, ep: &FileEndpoint) {
    let row = build_file_row(entry, ep);
    unsafe {
        gtk_box_pack_start(rows_box as *mut GtkBox, row, 0, 0, 0);
        gtk_widget_show_all(row);
    }
}

extern "C" fn drop_row_payload(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        let boxed = unsafe { Box::from_raw(data.cast::<RowPayload>()) };
        drop(boxed);
    }
}

fn active_terminal_vte() -> *mut GtkWidget {
    with_app(|app| {
        app.sessions.iter().find(|s| s.id == app.active_id).map_or(ptr::null_mut(), |s| s.active_vte())
    })
    .unwrap_or(ptr::null_mut())
}

fn start_ssh_file_download(route: &SshRoute, host: &str, path: &str) {
    let dest = default_downloads_dir();
    let files = vec![path.to_owned()];
    let (r, h) = (route.clone(), host.to_owned());
    with_app_mut(|app| app.transfer_manager.start_ssh_download(r, h, files, dest, ptr::null_mut(), false));
    notify_transfer_started();
}

extern "C" fn on_insert_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let Some(row) = (unsafe { data.cast::<RowPayload>().as_ref() }) else { return };
    let vte = active_terminal_vte();
    if !vte.is_null() {
        insert_shell_paths(vte, std::slice::from_ref(&row.entry.full_path));
    }
}

extern "C" fn on_download_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let Some(row) = (unsafe { data.cast::<RowPayload>().as_ref() }) else { return };
    if let FileEndpoint::Remote { route, host, .. } = &row.endpoint {
        start_ssh_file_download(route, host, &row.entry.full_path);
    }
}

extern "C" fn on_open_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let Some(row) = (unsafe { data.cast::<RowPayload>().as_ref() }) else { return };
    if row.entry.kind == FileKind::Dir {
        crate::files::drawer::navigate_to(&row.entry.full_path);
        return;
    }
    match &row.endpoint {
        FileEndpoint::Local { .. } => open_local_entry(&row.entry.full_path),
        FileEndpoint::Remote { route, host, .. } => open_remote_entry(route, host, &row.entry.full_path),
    }
}

fn open_local_entry(full_path: &str) {
    let path = PathBuf::from(full_path);
    if crate::markdown::accepts(&path) {
        crate::markdown::open(&path);
    } else {
        let _ = crate::platform::open_containing_folder(&path);
    }
}

fn open_remote_entry(route: &SshRoute, host: &str, full_path: &str) {
    if crate::markdown::accepts(Path::new(full_path)) {
        if let Some(doc) = crate::markdown::remote_document(active_terminal_vte(), full_path) {
            crate::markdown::open_remote(doc);
            return;
        }
    }
    start_ssh_file_download(route, host, full_path);
}

fn send_akhterm_selection(selection_data: *mut GtkSelectionData, row: &RowPayload) {
    let bytes = match &row.endpoint {
        FileEndpoint::Local { .. } => encode_local_drag(&[PathBuf::from(&row.entry.full_path)]),
        FileEndpoint::Remote { host, ssh_pid, .. } => encode_remote_drag(&[RemoteFileItem {
            host: host.clone(), ssh_pid: *ssh_pid, path: row.entry.full_path.clone(), kind: row.entry.kind, size: row.entry.size,
        }]),
    };
    let atom = unsafe { gdk_atom_intern_static_string(MIME_AKHTERM_FILES.as_ptr()) };
    unsafe { gtk_selection_data_set(selection_data, atom, 8, bytes.as_ptr(), bytes.len() as c_int) };
}

fn send_uri_selection(selection_data: *mut GtkSelectionData, path: &str) {
    let Ok(c_path) = CString::new(path) else { return };
    unsafe {
        let uri = g_filename_to_uri(c_path.as_ptr(), ptr::null(), ptr::null_mut());
        if uri.is_null() {
            return;
        }
        let uris: [*const c_char; 2] = [uri, ptr::null()];
        gtk_selection_data_set_uris(selection_data, uris.as_ptr());
        g_free(uri as *mut c_void);
    }
}

extern "C" fn on_row_drag_data_get(
    _w: *mut GtkWidget, _ctx: *mut GdkDragContext, data: *mut GtkSelectionData, info: c_uint, _t: c_uint, ptr: *mut c_void,
) {
    let Some(row) = (unsafe { ptr.cast::<RowPayload>().as_ref() }) else { return };
    if info == INFO_AKHTERM_FILES {
        send_akhterm_selection(data, row);
    } else if info == INFO_URI_LIST && matches!(row.endpoint, FileEndpoint::Local { .. }) {
        send_uri_selection(data, &row.entry.full_path);
    }
}

fn target_entry(mime: &CStr, info: c_uint) -> GtkTargetEntry {
    GtkTargetEntry {
        target: mime.as_ptr(),
        flags: 0,
        info,
    }
}

fn wire_drag_source(ev: *mut GtkWidget, kind: FileKind, is_local: bool, raw: *mut c_void) {
    let targets = [target_entry(MIME_AKHTERM_FILES, INFO_AKHTERM_FILES), target_entry(MIME_URI_LIST, INFO_URI_LIST)];
    let count = if is_local { 2 } else { 1 };
    let icon = if kind == FileKind::Dir { c"folder" } else { c"text-x-generic" };
    unsafe {
        gtk_drag_source_set(ev, GDK_BUTTON1_MASK, targets.as_ptr(), count, GDK_ACTION_COPY);
        gtk_drag_source_set_icon_name(ev, icon.as_ptr());
        g_signal_connect_data(
            ev, c"drag-data-get".as_ptr(), on_row_drag_data_get as *const c_void,
            raw, drop_row_payload as *const c_void, 0,
        );
    }
}

fn row_action_btn(label: &CStr, tip: &CStr, cb: extern "C" fn(*mut GtkWidget, *mut c_void), raw: *mut c_void) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new_with_label(label.as_ptr());
        add_class(btn, "files-row-btn");
        gtk_widget_set_can_focus(btn, 0);
        gtk_widget_set_tooltip_text(btn, tip.as_ptr());
        g_signal_connect_data(btn, c"clicked".as_ptr(), cb as *const c_void, raw, ptr::null(), 0);
        btn
    }
}

fn build_name_label(entry: &FileEntry) -> *mut GtkWidget {
    let icon = match entry.kind { FileKind::Dir => "📁", FileKind::Symlink => "🔗", FileKind::File => "📄" };
    let c_text = CString::new(format!("{icon} {}", entry.name)).unwrap_or_default();
    unsafe {
        let lbl = gtk_label_new(c_text.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        gtk_label_set_ellipsize(lbl, 3);
        add_class(lbl, "files-row-name");
        set_tooltip(lbl, &entry.full_path);
        lbl
    }
}

fn build_size_label(entry: &FileEntry) -> *mut GtkWidget {
    let size = if entry.kind == FileKind::File { format_bytes(entry.size) } else { String::new() };
    let c_size = CString::new(size).unwrap_or_default();
    unsafe {
        let lbl = gtk_label_new(c_size.as_ptr());
        gtk_label_set_xalign(lbl, 1.0);
        add_class(lbl, "files-row-size");
        lbl
    }
}

unsafe fn pack_row_controls(row: *mut GtkWidget, is_remote: bool, raw: *mut c_void) {
    let insert_b = row_action_btn(c"↵", FILES_INSERT_TIP.cstr(), on_insert_clicked, raw);
    gtk_box_pack_start(row as *mut GtkBox, insert_b, 0, 0, 0);
    if is_remote {
        let dl_b = row_action_btn(c"⬇", FILES_DOWNLOAD_TIP.cstr(), on_download_clicked, raw);
        gtk_box_pack_start(row as *mut GtkBox, dl_b, 0, 0, 0);
    }
    let open_b = row_action_btn(c"↗", FILES_OPEN_TIP.cstr(), on_open_clicked, raw);
    gtk_box_pack_start(row as *mut GtkBox, open_b, 0, 0, 0);
}

/// Build a single interactive, draggable file row for the drawer list.
pub(crate) fn build_file_row(entry: &FileEntry, endpoint: &FileEndpoint) -> *mut GtkWidget {
    let raw = Box::into_raw(Box::new(RowPayload { entry: entry.clone(), endpoint: endpoint.clone() })) as *mut c_void;
    let is_local = matches!(endpoint, FileEndpoint::Local { .. });
    unsafe {
        let ev = gtk_event_box_new();
        gtk_event_box_set_visible_window(ev, 0);
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        add_class(row, "files-row");
        gtk_box_pack_start(row as *mut GtkBox, build_name_label(entry), 1, 1, 0);
        gtk_box_pack_start(row as *mut GtkBox, build_size_label(entry), 0, 0, 2);
        pack_row_controls(row, !is_local, raw);
        gtk_container_add(ev as *mut GtkContainer, row);
        wire_drag_source(ev, entry.kind, is_local, raw);
        ev
    }
}
