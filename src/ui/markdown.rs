//! Read local Markdown alongside the workspace. GTK owns the widgets for the lifetime of the window.
#[path = "markdown_buffer.rs"]
mod buffer;
#[path = "markdown_file.rs"]
mod file;
pub use file::accepts;
#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;

use crate::{app_state, context_menu, ffi::*, ui_text as text};
use std::{
    cell::RefCell,
    ffi::{c_int, c_void, CStr, CString},
    fs,
    path::{Path, PathBuf},
    ptr,
    time::SystemTime,
};

extern "C" {
    fn gtk_adjustment_get_value(adjustment: *mut GtkAdjustment) -> f64;
    fn gtk_adjustment_set_value(adjustment: *mut GtkAdjustment, value: f64);
    fn vte_terminal_paste_text(vte: *mut GtkWidget, text: *const i8);
    fn gtk_widget_is_ancestor(widget: *mut GtkWidget, ancestor: *mut GtkWidget) -> c_int;
}

struct Reader {
    pane: *mut GtkWidget,
    root: *mut GtkWidget,
    name: *mut GtkWidget,
    search: *mut GtkWidget,
    scroll: *mut GtkWidget,
    buffer: buffer::Buffer,
    path: PathBuf,
    stamp: Option<(SystemTime, u64)>,
    source: String,
    blocks: Vec<(usize, usize)>,
}

thread_local! { static READER: RefCell<Option<Reader>> = const { RefCell::new(None) }; }

fn button(
    row: *mut GtkWidget,
    label: text::UiText,
    cb: extern "C" fn(*mut GtkWidget, *mut c_void),
) {
    unsafe {
        let widget = gtk_button_new_with_label(label.as_ptr());
        crate::connect_click(widget, cb, ptr::null_mut());
        gtk_box_pack_start(row, widget, 0, 0, 0);
    }
}

pub fn mount(parent: *mut GtkWidget, terminal: *mut GtkWidget) {
    unsafe {
        let pane = gtk_paned_new(GTK_ORIENTATION_HORIZONTAL);
        let root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        gtk_paned_pack1(pane, terminal, 1, 1);
        gtk_paned_pack2(pane, root, 0, 1);
        gtk_box_pack_start(parent, pane, 1, 1, 0);
        let bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        let name = gtk_label_new(ptr::null());
        gtk_label_set_ellipsize(name, 3);
        gtk_box_pack_start(bar, name, 1, 1, 0);
        button(bar, text::MD_CLOSE, close);
        gtk_box_pack_start(root, bar, 0, 1, 0);
        let search = gtk_search_entry_new();
        gtk_entry_set_placeholder_text(search, text::MD_SEARCH.as_ptr());
        gtk_box_pack_start(root, search, 0, 1, 0);
        crate::connect_entry_changed(search, search_changed);
        g_signal_connect_data(
            search,
            c"activate".as_ptr(),
            search_next as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );
        let actions = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        button(actions, text::MD_CODE, copy_code);
        button(actions, text::MD_PASTE, paste);
        gtk_box_pack_start(root, actions, 0, 1, 0);
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        let buffer = buffer::Buffer::new();
        gtk_container_add(scroll, buffer.view);
        gtk_box_pack_start(root, scroll, 1, 1, 0);
        gtk_widget_show_all(root);
        gtk_widget_set_no_show_all(root, 1);
        gtk_widget_set_visible(root, 0);
        g_signal_connect_data(
            root,
            c"destroy".as_ptr(),
            destroyed as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );
        READER.replace(Some(Reader {
            pane,
            root,
            name,
            search,
            scroll,
            buffer,
            path: PathBuf::new(),
            stamp: None,
            source: String::new(),
            blocks: Vec::new(),
        }));
    }
}

extern "C" fn destroyed(_: *mut GtkWidget, _: *mut c_void) {
    READER.replace(None);
}

extern "C" fn close(_: *mut GtkWidget, _: *mut c_void) {
    READER.with_borrow(|r| {
        if let Some(r) = r {
            unsafe { gtk_widget_set_visible(r.root, 0) };
        }
    });
    app_state::focus_active_vte();
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

impl Reader {
    fn update(&mut self, source: String, preserve: bool) {
        let (start, end) = self.buffer.bounds();
        let adj = unsafe { gtk_scrolled_window_get_vadjustment(self.scroll) };
        let y = unsafe { gtk_adjustment_get_value(adj) };
        self.blocks = file::code_blocks(&source);
        self.buffer.render(&source, &self.blocks);
        self.source = source;
        if preserve {
            self.buffer.select(start, end, false);
        }
        unsafe { gtk_adjustment_set_value(adj, if preserve { y } else { 0.0 }) };
    }

    fn find(&self, next: bool) {
        let query = unsafe { CStr::from_ptr(gtk_entry_get_text(self.search)) }.to_string_lossy();
        if query.is_empty() {
            return;
        }
        let from = if next {
            file::byte_at(&self.source, self.buffer.bounds().1)
        } else {
            0
        };
        let hit = self.source[from..]
            .find(query.as_ref())
            .map(|n| n + from)
            .or_else(|| self.source.find(query.as_ref()));
        if let Some(byte) = hit {
            let start = self.source[..byte].chars().count() as i32;
            self.buffer
                .select(start, start + query.chars().count() as i32, true);
        }
    }
}

pub fn open(path: &Path) {
    let observed = stamp(path);
    let source = match file::read(path) {
        Ok(text) => text,
        Err(err) => return app_state::set_footer_message(&format!("{} · {err}", path.display())),
    };
    READER.with_borrow_mut(|r| {
        let Some(r) = r else { return };
        let preserve = r.path == path;
        r.path = path.to_path_buf();
        r.stamp = observed;
        set_label_text(
            r.name,
            &path.file_name().unwrap_or_default().to_string_lossy(),
        );
        set_tooltip(r.name, &path.to_string_lossy());
        r.update(source, preserve);
        unsafe {
            if gtk_widget_get_visible(r.root) == 0 {
                gtk_paned_set_position(r.pane, gtk_widget_get_allocated_width(r.pane) / 2);
                gtk_widget_set_visible(r.root, 1);
            }
            gtk_widget_grab_focus(r.buffer.view);
        }
    });
}

pub fn reload() {
    READER.with_borrow_mut(|slot| {
        let Some(r) = slot else { return };
        if unsafe { gtk_widget_get_visible(r.root) } == 0 {
            return;
        }
        let observed = stamp(&r.path);
        if r.stamp == observed {
            return;
        }
        r.stamp = observed;
        match file::read(&r.path) {
            Ok(source) => r.update(source, true),
            Err(err) => app_state::set_footer_message(&format!("{} · {err}", r.path.display())),
        }
    });
}

extern "C" fn search_changed(_: *mut GtkWidget, _: *mut c_void) {
    READER.with_borrow(|r| {
        if let Some(r) = r {
            r.find(false);
        }
    });
}
extern "C" fn search_next(_: *mut GtkWidget, _: *mut c_void) {
    READER.with_borrow(|r| {
        if let Some(r) = r {
            r.find(true);
        }
    });
}

extern "C" fn copy_code(_: *mut GtkWidget, _: *mut c_void) {
    READER.with_borrow(|r| {
        let Some(r) = r else { return };
        let cursor = file::byte_at(&r.source, r.buffer.bounds().0);
        if let Some(&(a, b)) = r.blocks.iter().find(|&&(a, b)| a <= cursor && cursor <= b) {
            context_menu::copy_text_to_clipboard(&r.source[a..b]);
        } else {
            app_state::set_footer_message(text::MD_BLOCK_HINT.as_str());
        }
    });
}

extern "C" fn paste(_: *mut GtkWidget, _: *mut c_void) {
    let selected = READER.with_borrow(|slot| {
        let r = slot.as_ref()?;
        let (a, b) = r.buffer.bounds();
        (a != b)
            .then(|| {
                CString::new(&r.source[file::byte_at(&r.source, a)..file::byte_at(&r.source, b)])
                    .ok()
            })
            .flatten()
    });
    let Some(text) = selected else { return };
    let vte = crate::shortcuts::active_search_vte();
    if app_state::has_live_vte(vte) {
        unsafe {
            gtk_widget_grab_focus(vte);
            vte_terminal_paste_text(vte, text.as_ptr());
        }
    }
}

pub fn open_last() {
    let path = app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        let cwd = s.active_local_cwd()?;
        file::last_path(&crate::handoff::extract_vte_text(s.active_vte()), &cwd)
    })
    .flatten();
    if let Some(path) = path {
        open(&path);
    } else {
        app_state::set_footer_message(text::MD_EMPTY.as_str());
    }
}

pub fn route_key(win: *mut GtkWidget, key: u32, mods: u32) -> Option<c_int> {
    let focus = unsafe { gtk_window_get_focus(win) };
    let search = READER.with_borrow(|r| {
        let r = r.as_ref()?;
        (!focus.is_null() && unsafe { gtk_widget_is_ancestor(focus, r.root) != 0 })
            .then_some(r.search)
    })?;
    if key == 0xff1b {
        close(ptr::null_mut(), ptr::null_mut());
        return Some(1);
    }
    let modifiers = mods & (GDK_CONTROL_MASK | GDK_SHIFT_MASK | GDK_MOD1_MASK);
    if modifiers == GDK_CONTROL_MASK && key == u32::from(b'f') {
        unsafe { gtk_widget_grab_focus(search) };
        return Some(1);
    }
    if modifiers & GDK_MOD1_MASK != 0 || modifiers == GDK_CONTROL_MASK | GDK_SHIFT_MASK {
        return None;
    }
    if modifiers == GDK_CONTROL_MASK && matches!(char::from_u32(key), Some('k' | ',')) {
        return None;
    }
    Some(0)
}
