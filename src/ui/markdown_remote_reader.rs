use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static REQUEST: AtomicU64 = AtomicU64::new(1);

pub(super) fn mount_path(root: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let entry = gtk_entry_new();
        gtk_entry_set_placeholder_text(entry, text::MD_ABSOLUTE.as_ptr());
        gtk_widget_set_no_show_all(entry, 1);
        gtk_box_pack_start(root, entry, 0, 1, 0);
        connect_entry_activate(entry, submit_path);
        entry
    }
}

extern "C" fn submit_path(entry: *mut GtkWidget, _: *mut c_void) {
    let value = unsafe { CStr::from_ptr(gtk_entry_get_text(entry)) }.to_string_lossy();
    let Some(path) = remote::preview_path(&value).filter(|path| path.is_absolute()) else {
        return app_state::set_footer_message(text::MD_ABSOLUTE.as_str());
    };
    if let Some(mut doc) = current_document() {
        doc.path = path;
        open_remote(doc);
    }
}

fn current_document() -> Option<RemoteDocument> {
    READER.with_borrow(|slot| slot.as_ref()?.remote.clone())
}

pub(super) extern "C" fn refresh(_: *mut GtkWidget, _: *mut c_void) {
    if let Some(doc) = current_document() {
        open_remote(doc);
    }
}

impl Reader {
    fn show_remote(&mut self, doc: &RemoteDocument) -> Option<u64> {
        let id = REQUEST.fetch_add(1, Ordering::Relaxed);
        if self.remote.as_ref() != Some(doc) {
            self.update(String::new(), false);
            self.picker.reset(Some(&doc.origin));
        }
        self.path.clone_from(&doc.path);
        self.remote = Some(doc.clone());
        self.request = Some(id);
        self.stamp = None;
        self.show();
        if !doc.path.is_absolute() {
            self.prompt_for_path();
            return None;
        }
        Some(id)
    }

    fn prompt_for_path(&self) {
        if let Ok(path) = CString::new(self.path.to_string_lossy().as_bytes()) {
            unsafe {
                gtk_entry_set_text(self.entry, path.as_ptr());
                gtk_widget_grab_focus(self.entry);
            }
        }
        app_state::set_footer_message(text::MD_ABSOLUTE.as_str());
    }
}

pub fn open_remote(doc: RemoteDocument) {
    let request = READER.with_borrow_mut(|slot| slot.as_mut()?.show_remote(&doc));
    let Some(id) = request else { return };
    let Some(conn) = doc.connection else {
        return app_state::set_footer_message(text::MD_CONNECTION.as_str());
    };
    app_state::set_footer_message(text::MD_LOADING.as_str());
    std::thread::spawn(move || {
        let result = conn.read(&doc.path);
        on_gtk_idle(move || complete(id, result));
    });
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
