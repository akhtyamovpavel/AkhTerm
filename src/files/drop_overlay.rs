//! Non-interactive two-zone drop overlay rendered over each VTE pane.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_void, CString};

use crate::ffi::*;
use crate::files::DropMode;
use crate::ui_text::i18n_text::*;

#[derive(Clone, Copy)]
struct OverlayState {
    drop_box: *mut GtkWidget,
    left: [*mut GtkWidget; 3],
    right: [*mut GtkWidget; 3],
    last_mode: DropMode,
}

thread_local! {
    static OVERLAYS: RefCell<HashMap<usize, OverlayState>> = RefCell::new(HashMap::new());
}

fn zone_label(cls: &str, text: &str) -> *mut GtkWidget {
    let c = CString::new(text).unwrap_or_default();
    unsafe {
        let lbl = gtk_label_new(c.as_ptr());
        gtk_label_set_xalign(lbl, 0.5);
        add_class(lbl, cls);
        lbl
    }
}

fn zone_card(title: &str, sub: &str) -> [*mut GtkWidget; 3] {
    unsafe {
        let card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        add_class(card, "drop-zone-card");
        gtk_widget_set_valign(card, GTK_ALIGN_CENTER);
        let t = zone_label("drop-zone-title", title);
        let s = zone_label("drop-zone-sub", sub);
        gtk_box_pack_start(card as *mut GtkBox, t, 0, 0, 0);
        gtk_box_pack_start(card as *mut GtkBox, s, 0, 0, 0);
        [card, t, s]
    }
}

extern "C" fn on_scroll_destroy(scroll: *mut GtkWidget, _data: *mut c_void) {
    if !scroll.is_null() {
        OVERLAYS.with(|map| {
            map.borrow_mut().remove(&(scroll as usize));
        });
    }
}

fn build_overlay_state() -> OverlayState {
    let left = zone_card(DROP_INSERT_TITLE.as_str(), DROP_INSERT_LOCAL_SUB.as_str());
    let right = zone_card(DROP_TRANSFER_LOCAL_TITLE.as_str(), DROP_TRANSFER_SUB.as_str());
    unsafe {
        let drop_box = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 16);
        add_class(drop_box, "drop-overlay-backdrop");
        gtk_widget_set_no_show_all(drop_box, 1);
        gtk_widget_set_visible(drop_box, 0);
        gtk_box_pack_start(drop_box as *mut GtkBox, left[0], 1, 1, 0);
        gtk_box_pack_start(drop_box as *mut GtkBox, right[0], 1, 1, 0);
        OverlayState { drop_box, left, right, last_mode: DropMode::InsertPath }
    }
}

/// Wrap `scroll` in a `GtkOverlay` containing a pass-through two-zone drop indicator.
pub fn wrap_pane_scroll(scroll: *mut GtkWidget) -> *mut GtkWidget {
    if scroll.is_null() {
        return scroll;
    }
    let st = build_overlay_state();
    let cb = on_scroll_destroy as *const c_void;
    unsafe {
        let overlay = gtk_overlay_new();
        gtk_container_add(overlay as *mut GtkContainer, scroll);
        gtk_overlay_add_overlay(overlay, st.drop_box);
        gtk_overlay_set_overlay_pass_through(overlay, st.drop_box, 1);
        g_signal_connect_data(scroll, c"destroy".as_ptr(), cb, std::ptr::null_mut(), std::ptr::null(), 0);
        OVERLAYS.with(|map| map.borrow_mut().insert(scroll as usize, st));
        overlay
    }
}

/// Choose left (`InsertPath`) or right (`CopyToCwd`) zone based on horizontal cursor `x`.
pub fn mode_for_x(width: i32, x: i32) -> DropMode {
    if width <= 0 || x.saturating_mul(2) < width {
        DropMode::InsertPath
    } else {
        DropMode::CopyToCwd
    }
}

fn find_scroll_key(vte: *mut GtkWidget) -> Option<usize> {
    if vte.is_null() {
        return None;
    }
    OVERLAYS.with(|map| {
        let map = map.borrow();
        let mut cur = vte;
        for _ in 0..4 {
            cur = unsafe { gtk_widget_get_parent(cur) };
            if map.contains_key(&(cur as usize)) {
                return Some(cur as usize);
            }
        }
        None
    })
}

fn sync_overlay_copy(st: &OverlayState, is_remote: bool, is_image: bool) {
    let left_sub = match (is_image, is_remote) {
        (true, _) => DROP_PASTE_IMAGE_SUB.as_str(),
        (false, true) => DROP_INSERT_REMOTE_SUB.as_str(),
        (false, false) => DROP_INSERT_LOCAL_SUB.as_str(),
    };
    let right_title = if is_remote { DROP_TRANSFER_REMOTE_TITLE } else { DROP_TRANSFER_LOCAL_TITLE };
    set_label_text(st.left[1], DROP_INSERT_TITLE.as_str());
    set_label_text(st.left[2], left_sub);
    set_label_text(st.right[1], right_title.as_str());
    set_label_text(st.right[2], DROP_TRANSFER_SUB.as_str());
}

fn with_vte_overlay(vte: *mut GtkWidget, mut f: impl FnMut(&mut OverlayState)) {
    let Some(key) = find_scroll_key(vte) else {
        return;
    };
    OVERLAYS.with(|map| {
        if let Some(st) = map.borrow_mut().get_mut(&key) {
            f(st);
        }
    });
}

fn vte_drop_mode(vte: *mut GtkWidget, x: i32) -> DropMode {
    let width = if vte.is_null() {
        0
    } else {
        unsafe { gtk_widget_get_allocated_width(vte) }
    };
    mode_for_x(width, x)
}

/// Show the drop overlay above `vte` and highlight the active zone for `x`.
pub fn show_for_vte(vte: *mut GtkWidget, x: i32, is_remote: bool, is_image: bool) -> DropMode {
    let mode = vte_drop_mode(vte, x);
    with_vte_overlay(vte, |st| {
        st.last_mode = mode;
        sync_overlay_copy(st, is_remote, is_image);
        set_class(st.left[0], "drop-zone-active", mode == DropMode::InsertPath);
        set_class(st.right[0], "drop-zone-active", mode == DropMode::CopyToCwd);
        unsafe { gtk_widget_set_visible(st.drop_box, 1) };
    });
    mode
}

/// Hide the drop overlay above `vte` while retaining `last_mode` until drop completes.
pub fn hide_for_vte(vte: *mut GtkWidget) {
    if vte.is_null() {
        return;
    }
    with_vte_overlay(vte, |st| {
        unsafe { gtk_widget_set_visible(st.drop_box, 0) };
    });
}

/// Update `last_mode` at the drop coordinates and return it.
pub fn record_drop_x(vte: *mut GtkWidget, x: i32) -> DropMode {
    let mode = vte_drop_mode(vte, x);
    with_vte_overlay(vte, |st| {
        st.last_mode = mode;
    });
    mode
}

/// Read the active zone recorded for `vte` (defaults to `DropMode::InsertPath`).
pub fn take_mode_for_vte(vte: *mut GtkWidget) -> DropMode {
    let Some(key) = find_scroll_key(vte) else {
        return DropMode::InsertPath;
    };
    OVERLAYS.with(|map| map.borrow().get(&key).map_or(DropMode::InsertPath, |s| s.last_mode))
}

#[cfg(test)]
pub(crate) fn inspect_vte_overlay(vte: *mut GtkWidget) -> Option<(bool, bool, bool)> {
    let key = find_scroll_key(vte)?;
    OVERLAYS.with(|map| {
        let map = map.borrow();
        let st = map.get(&key)?;
        Some((
            has_class(st.left[0], "drop-zone-active"),
            has_class(st.right[0], "drop-zone-active"),
            unsafe { gtk_widget_get_visible(st.drop_box) != 0 },
        ))
    })
}
