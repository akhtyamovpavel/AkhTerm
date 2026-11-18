use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static REQUEST: AtomicU64 = AtomicU64::new(1);

pub(super) fn mount_path(root: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let entry = gtk_entry_new();
        gtk_entry_set_placeholder_text(entry, text::MD_ABSOLUTE.as_ptr());
        gtk_widget_set_no_show_all(entry, 1);
        gtk_box_pack_start(root, entry, 0, 1, 0);
        g_signal_connect_data(
            entry,
            c"activate".as_ptr(),
            submit_path as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );
        entry
    }
}

extern "C" fn submit_path(entry: *mut GtkWidget, _: *mut c_void) {
    let path = PathBuf::from(
        unsafe { CStr::from_ptr(gtk_entry_get_text(entry)) }
            .to_string_lossy()
            .as_ref(),
    );
    if !path.is_absolute()
        || !file::accepts(&path)
        || path.to_string_lossy().chars().any(char::is_control)
    {
        return app_state::set_footer_message(text::MD_ABSOLUTE.as_str());
    }
    let doc = READER.with_borrow(|r| r.as_ref()?.remote.clone());
    if let Some(mut doc) = doc {
        doc.path = path;
        open_remote(doc);
    }
}

pub(super) extern "C" fn refresh(_: *mut GtkWidget, _: *mut c_void) {
    if let Some(doc) = READER.with_borrow(|r| r.as_ref()?.remote.clone()) {
        open_remote(doc);
    }
}

pub fn open_remote(doc: RemoteDocument) {
    let id = REQUEST.fetch_add(1, Ordering::Relaxed);
    let ready = READER.with_borrow_mut(|slot| {
        let r = slot.as_mut()?;
        let preserve = r.remote.as_ref() == Some(&doc);
        if !preserve {
            r.update(String::new(), false);
            r.picker.reset(Some(&doc.origin));
        }
        r.path = doc.path.clone();
        r.remote = Some(doc.clone());
        r.request = Some(id);
        r.stamp = None;
        let title = format!("{} · {}", doc.host, doc.path.display());
        set_label_text(r.name, &title);
        set_tooltip(r.name, &title);
        let absolute = doc.path.is_absolute();
        unsafe {
            gtk_widget_set_visible(r.refresh, 1);
            gtk_widget_set_visible(r.entry, i32::from(!absolute));
        }
        r.show();
        if !absolute {
            if let Ok(path) = CString::new(doc.path.to_string_lossy().as_bytes()) {
                unsafe {
                    gtk_entry_set_text(r.entry, path.as_ptr());
                    gtk_widget_grab_focus(r.entry);
                }
            }
            app_state::set_footer_message(text::MD_ABSOLUTE.as_str());
            return None;
        }
        let Some(conn) = doc.connection else {
            app_state::set_footer_message(text::MD_CONNECTION.as_str());
            return None;
        };
        app_state::set_footer_message(text::MD_LOADING.as_str());
        Some((conn, doc.path))
    });
    if let Some((conn, path)) = ready {
        std::thread::spawn(move || {
            let result = Box::new((id, conn.read(&path)));
            unsafe {
                g_idle_add(finished, Box::into_raw(result).cast());
            }
        });
    }
}

type ResultMessage = (u64, std::io::Result<String>);

extern "C" fn finished(data: *mut c_void) -> c_int {
    let (id, result) = unsafe { *Box::from_raw(data.cast::<ResultMessage>()) };
    complete(id, result);
    0
}

pub(super) fn complete(id: u64, result: std::io::Result<String>) {
    READER.with_borrow_mut(|r| {
        let Some(r) = r.as_mut().filter(|r| r.request == Some(id)) else {
            return;
        };
        r.request = None;
        match result {
            Ok(source) => {
                r.update(source, true);
                app_state::set_footer_message(&r.path.display().to_string());
            }
            Err(err) => app_state::set_footer_message(&format!("{} · {err}", r.path.display())),
        }
    });
}
