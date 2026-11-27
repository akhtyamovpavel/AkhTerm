//! Drag-in, file chooser upload, and SSH download dispatch for the Files drawer.

use std::{ffi::*, path::PathBuf, ptr, time::*};

use crate::app::app_state::{with_app, with_app_mut};
use crate::ffi::*;
use crate::files::{local_fs::default_downloads_dir, mime::*, transfer_banner::notify_transfer_started, *};
use crate::telemetry::ssh_connection::SshRoute;
use crate::terminal::file_drop::extract_selection_payload;
use crate::ui_text::i18n_text::FILES_UPLOAD_BTN;

fn stage_png_upload(bytes: Vec<u8>) -> Option<DragPayload> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let dir = std::env::temp_dir().join("akhterm-drops");
    let path = stage_png_bytes(&bytes, &dir, stamp).ok()?;
    Some(DragPayload::LocalPaths(vec![path]))
}

fn route_for_remote_item(item: &RemoteFileItem) -> Option<SshRoute> {
    if item.ssh_pid > 0 {
        return SshRoute::from_client(item.ssh_pid, &item.host);
    }
    SshRoute::for_host(&item.host)
}

fn start_remote_to_local(src: SshRoute, host: String, paths: Vec<String>, cwd: PathBuf) -> bool {
    let vte = ptr::null_mut();
    let res = with_app_mut(|a| a.transfer_manager.start_ssh_download(src, host, paths, cwd, vte, false));
    if res.is_some() {
        return true;
    }
    false
}

fn start_remote_to_remote(src: SshRoute, paths: Vec<String>, dst: SshRoute, host: String, cwd: String) -> bool {
    let vte = ptr::null_mut();
    let res = with_app_mut(|a| a.transfer_manager.start_ssh_to_ssh(src, paths, dst, host, cwd, vte, false));
    if res.is_some() {
        return true;
    }
    false
}

fn enqueue_remote_drawer_drop(items: Vec<RemoteFileItem>, dst: FileEndpoint) -> bool {
    let Some(first) = items.first() else { return false };
    let Some(src) = route_for_remote_item(first) else { return false };
    let host = first.host.clone();
    let paths = items.into_iter().map(|i| i.path).collect();
    match dst {
        FileEndpoint::Local { cwd } => start_remote_to_local(src, host, paths, cwd),
        FileEndpoint::Remote { route, host: h, cwd, .. } => start_remote_to_remote(src, paths, route, h, cwd),
    }
}

fn start_local_drop(src: Vec<PathBuf>, target: FileEndpoint) -> bool {
    let vte = ptr::null_mut();
    match target {
        FileEndpoint::Local { cwd } => {
            with_app_mut(|a| a.transfer_manager.start_local_copy(src, cwd, vte, false)).is_some()
        }
        FileEndpoint::Remote { route, host, cwd, .. } => {
            with_app_mut(|a| a.transfer_manager.start_ssh_upload(route, host, src, cwd, false, vte, false)).is_some()
        }
    }
}

fn dispatch_payload_to_drawer(payload: DragPayload) -> bool {
    let Some(ep) = crate::files::drawer::current_endpoint() else { return false };
    let normalized = match payload {
        DragPayload::PngBytes(bytes) => {
            let Some(p) = stage_png_upload(bytes) else { return false };
            p
        }
        other => other,
    };
    let ok = match normalized {
        DragPayload::LocalPaths(src) => start_local_drop(src, ep),
        DragPayload::RemoteItems(items) => enqueue_remote_drawer_drop(items, ep),
        DragPayload::PngBytes(_) => false,
    };
    if ok { notify_transfer_started(); }
    ok
}

/// Start an SSH download to Downloads for a right-clicked remote file path.
pub(crate) fn start_remote_download_link(link: RemoteFileLink) {
    let dir = default_downloads_dir();
    let ok = start_remote_to_local(link.route, link.host, vec![link.path], dir);
    if ok {
        notify_transfer_started();
    }
}

extern "C" fn on_drawer_drag_data(
    _w: *mut GtkWidget, ctx: *mut GdkDragContext, _x: c_int, _y: c_int,
    data: *const GtkSelectionData, info: c_uint, time: c_uint, _u: *mut c_void,
) {
    let payload = extract_selection_payload(data, info);
    let ok = payload.is_some_and(dispatch_payload_to_drawer);
    finish_drag(ctx, ok, time);
}

unsafe fn collect_chooser_paths(dlg: *mut GtkWidget) -> Vec<PathBuf> {
    let list = gtk_file_chooser_get_filenames(dlg);
    let mut cur = list.cast::<GSList>();
    let mut paths = Vec::new();
    while !cur.is_null() {
        let p = (*cur).data.cast::<c_char>();
        if !p.is_null() {
            paths.push(PathBuf::from(CStr::from_ptr(p).to_string_lossy().into_owned()));
            g_free(p.cast());
        }
        cur = (*cur).next;
    }
    if !list.is_null() {
        g_slist_free(list);
    }
    paths
}

unsafe fn create_upload_dialog(parent: *mut GtkWidget) -> *mut GtkWidget {
    let title = FILES_UPLOAD_BTN.cstr().as_ptr();
    let dlg = gtk_file_chooser_dialog_new(
        title, parent, GTK_FILE_CHOOSER_ACTION_OPEN,
        c"Cancel".as_ptr(), GTK_RESPONSE_CANCEL, title, GTK_RESPONSE_ACCEPT, ptr::null::<c_char>(),
    );
    gtk_file_chooser_set_select_multiple(dlg, 1);
    dlg
}

/// Open a multi-file chooser dialog and copy/upload selected files into the drawer directory.
pub(crate) extern "C" fn on_upload_clicked(_: *mut GtkWidget, _: *mut c_void) {
    let parent = with_app(|a| a.win).unwrap_or(ptr::null_mut());
    unsafe {
        let dlg = create_upload_dialog(parent);
        let paths = if gtk_dialog_run(dlg) == GTK_RESPONSE_ACCEPT { collect_chooser_paths(dlg) } else { Vec::new() };
        gtk_widget_destroy(dlg);
        if !paths.is_empty() { dispatch_payload_to_drawer(DragPayload::LocalPaths(paths)); }
    }
}

fn drawer_drop_targets() -> [GtkTargetEntry; 4] {
    [
        GtkTargetEntry { target: MIME_AKHTERM_FILES.as_ptr(), flags: 0, info: INFO_AKHTERM_FILES },
        GtkTargetEntry { target: MIME_URI_LIST.as_ptr(), flags: 0, info: INFO_URI_LIST },
        GtkTargetEntry { target: MIME_GNOME_COPIED.as_ptr(), flags: 0, info: INFO_GNOME_COPIED },
        GtkTargetEntry { target: MIME_IMAGE_PNG.as_ptr(), flags: 0, info: INFO_IMAGE_PNG },
    ]
}

/// Enable file/URI/image drops onto the drawer `root` box.
pub(crate) fn setup_drawer_drop(root: *mut GtkWidget) {
    let entries = drawer_drop_targets();
    let cb = on_drawer_drag_data as *const c_void;
    unsafe {
        gtk_drag_dest_set(root, DEST_DEFAULT_ALL, entries.as_ptr(), entries.len() as c_int, GDK_ACTION_COPY);
        g_signal_connect_data(root, c"drag-data-received".as_ptr(), cb, ptr::null_mut(), ptr::null(), 0);
    }
}
