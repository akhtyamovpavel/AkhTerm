//! Right-click context menus for VTE terminal panes, tab strip buttons, and sidebar rows.

use std::cell::Cell;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;

use crate::ffi::*;
use crate::file_drop::decode_file_uri;
use crate::handoff::{captures_dir, current_unix_secs, extract_vte_text, write_capture_file, CaptureMeta};
use crate::paths::{expand_home, strip_anchor_or_line, strip_link_punctuation};
use crate::ui_text as text;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextAction {
    CopySelection(*mut GtkWidget),
    PasteSmart { sid: usize, vte: *mut GtkWidget },
    OpenLink(String),
    CopyText(String),
    OpenContainingFolder(PathBuf),
    ForwardToTab { source_sid: usize, source_vte: *mut GtkWidget, target_sid: usize, selected_text: Option<String> },
    SplitRight { sid: usize, vte: *mut GtkWidget },
    SplitDown { sid: usize, vte: *mut GtkWidget },
    ClosePane { sid: usize, vte: *mut GtkWidget },
    ClearBuffer(*mut GtkWidget),
    RenameTab { sid: usize, anchor: *mut GtkWidget },
    DuplicateTab(usize),
    ReconnectTab(usize),
    CloseTab(usize),
    CloseOtherTabs(usize),
}

pub type DispatchFn = Rc<dyn Fn(ContextAction)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTarget {
    pub sid: usize,
    pub label: String,
}

pub fn format_session_target(sid: usize, title: &str, ssh_host: &str, source_sid: usize) -> SessionTarget {
    let base = if !ssh_host.is_empty() && !title.contains(ssh_host) {
        format!("{title} [{ssh_host}]")
    } else {
        title.to_owned()
    };
    let label = if sid == source_sid {
        format!("{base} {}", text::ctx_current_session_str())
    } else {
        base
    };
    SessionTarget { sid, label }
}

fn existing_local_candidate(raw: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(raw);
    if direct.exists() {
        return Some(direct);
    }
    let stripped = strip_anchor_or_line(raw);
    (!stripped.is_empty() && stripped != raw)
        .then(|| PathBuf::from(stripped))
        .filter(|p| p.exists())
}

fn resolve_file_uri_local_path(trimmed: &str) -> Option<PathBuf> {
    if let Some(full) = decode_file_uri(trimmed).filter(|p| p.exists()) {
        return Some(full);
    }
    let base_uri = trimmed.split(['#', '?']).next().unwrap_or(trimmed);
    let decoded = decode_file_uri(base_uri)?;
    existing_local_candidate(&decoded.to_string_lossy())
}

pub fn resolve_local_link_path(raw: &str) -> Option<PathBuf> {
    let trimmed = strip_link_punctuation(raw);
    if trimmed.is_empty() || trimmed.starts_with("mailto:") {
        return None;
    }
    if trimmed.starts_with("file://") {
        return resolve_file_uri_local_path(trimmed);
    }
    resolve_non_uri_local_path(trimmed)
}

fn resolve_non_uri_local_path(trimmed: &str) -> Option<PathBuf> {
    if trimmed.contains("://") {
        return None;
    }
    if trimmed == "~" || trimmed.starts_with("~/") {
        return existing_local_candidate(&expand_home(trimmed));
    }
    existing_local_candidate(trimmed)
}

pub fn should_passthrough_vte_click(button: u32, state: u32) -> bool {
    let shift_held = (state & GDK_SHIFT_MASK) != 0;
    let right_btn = button == 3;
    right_btn && shift_held
}

pub fn is_right_button_press(event: *const GdkEvent) -> bool {
    if event.is_null() {
        return false;
    }
    let mut button: c_uint = 0;
    unsafe {
        let is_press = gdk_event_get_event_type(event) == GDK_BUTTON_PRESS;
        is_press && gdk_event_get_button(event, &mut button) != 0 && button == 3
    }
}

pub fn read_vte_selection(vte: *mut GtkWidget) -> Option<String> {
    if vte.is_null() || unsafe { vte_terminal_get_has_selection(vte) } == 0 {
        return None;
    }
    let ptr = unsafe { vte_terminal_get_text_selected(vte, VTE_FORMAT_TEXT) };
    if ptr.is_null() {
        return None;
    }
    let selected = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    unsafe { g_free(ptr as *mut c_void) };
    (!selected.is_empty()).then_some(selected)
}

pub fn copy_text_to_clipboard(value: &str) {
    let Ok(c_text) = CString::new(value) else {
        return;
    };
    unsafe {
        let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
        if !clip.is_null() {
            gtk_clipboard_set_text(clip, c_text.as_ptr(), value.len() as c_int);
        }
    }
}

pub fn copy_vte_selection_to_clipboard(vte: *mut GtkWidget, clear_after: bool) -> bool {
    if vte.is_null() || unsafe { vte_terminal_get_has_selection(vte) } == 0 {
        return false;
    }
    let Some(sel) = read_vte_selection(vte) else {
        return false;
    };
    unsafe { vte_terminal_copy_clipboard_format(vte, VTE_FORMAT_TEXT) };
    copy_text_to_clipboard(&sel);
    if clear_after {
        unsafe { vte_terminal_unselect_all(vte) };
    }
    true
}

pub fn save_forward_capture(
    briefs_dir: Option<&Path>,
    pane_vte: *mut GtkWidget,
    title: &str,
    cwd: &str,
    ssh_host: &str,
    selected_text: Option<String>,
) -> Result<PathBuf, String> {
    let raw = selected_text.unwrap_or_else(|| extract_vte_text(pane_vte));
    let meta = CaptureMeta { title, cwd, ssh_host, timestamp_secs: current_unix_secs() };
    let fallback = captures_dir();
    write_capture_file(briefs_dir, &fallback, &meta, &raw)
        .map_err(|err| format!("⚠ Ошибка сохранения снимка: {err}"))
}

pub struct VteMenuSpec {
    pub sid: usize,
    pub vte: *mut GtkWidget,
    pub has_selection: bool,
    pub selected_text: Option<String>,
    pub link_target: Option<String>,
    pub is_local: bool,
    pub targets: Vec<SessionTarget>,
}

pub struct TabMenuSpec {
    pub sid: usize,
    pub anchor: *mut GtkWidget,
    pub local_cwd: Option<String>,
    pub is_remote: bool,
    pub can_close_others: bool,
}

pub struct BuiltMenu {
    pub menu: *mut GtkWidget,
    pub items: Vec<*mut GtkWidget>,
    pub submenu_items: Vec<*mut GtkWidget>,
}

struct ItemBinding {
    action: ContextAction,
    dispatch: DispatchFn,
}

thread_local! {
    static ACTIVE_CTX_MENU: Cell<*mut GtkWidget> = const { Cell::new(ptr::null_mut()) };
    static ACTIVE_RENAME_POP: Cell<*mut GtkWidget> = const { Cell::new(ptr::null_mut()) };
    static RENAME_OPEN: Cell<bool> = const { Cell::new(false) };
}

pub fn is_rename_open() -> bool {
    RENAME_OPEN.with(Cell::get)
}

pub fn dismiss_rename_popover() {
    RENAME_OPEN.with(|open| open.set(false));
    let prev = ACTIVE_RENAME_POP.with(|slot| slot.replace(ptr::null_mut()));
    if !prev.is_null() {
        unsafe { gtk_widget_destroy(prev) };
    }
}

extern "C" fn on_item_activate(_item: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let binding = unsafe { &*data.cast::<ItemBinding>() };
    let action = binding.action.clone();
    let dispatch = Rc::clone(&binding.dispatch);
    dispatch(action);
}

extern "C" fn drop_item_binding(data: *mut c_void, _closure: *mut c_void) {
    if data.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(data.cast::<ItemBinding>())) };
}

extern "C" fn on_menu_destroyed(menu: *mut GtkWidget, _data: *mut c_void) {
    ACTIVE_CTX_MENU.with(|slot| {
        if slot.get() == menu {
            slot.set(ptr::null_mut());
        }
    });
}

extern "C" fn destroy_menu_idle(data: *mut c_void) -> c_int {
    let target = data as *mut GtkWidget;
    let should_destroy = ACTIVE_CTX_MENU.with(|slot| {
        let matches = slot.get() == target;
        if matches {
            slot.set(ptr::null_mut());
        }
        matches
    });
    if should_destroy && !target.is_null() {
        unsafe { gtk_widget_destroy(target) };
    }
    0
}

extern "C" fn on_menu_selection_done(menu: *mut GtkWidget, _data: *mut c_void) {
    if menu.is_null() {
        return;
    }
    unsafe { g_idle_add(destroy_menu_idle, menu as *mut c_void) };
}

fn new_styled_menu() -> *mut GtkWidget {
    unsafe {
        let menu = gtk_menu_new();
        add_class(menu, "context-menu");
        let handler = on_menu_destroyed as *const c_void;
        g_signal_connect_data(menu, c"destroy".as_ptr(), handler, ptr::null_mut(), ptr::null(), 0);
        menu
    }
}

fn append_separator(menu: *mut GtkWidget) {
    unsafe {
        let sep = gtk_separator_menu_item_new();
        gtk_menu_shell_append(menu, sep);
    }
}

fn append_bound_item(menu: *mut GtkWidget, label: *const c_char, sensitive: bool, action: ContextAction, dispatch: &DispatchFn) -> *mut GtkWidget {
    let payload = Box::into_raw(Box::new(ItemBinding { action, dispatch: Rc::clone(dispatch) }));
    unsafe {
        let item = gtk_menu_item_new_with_label(label);
        gtk_widget_set_sensitive(item, c_int::from(sensitive));
        let cb = on_item_activate as *const c_void;
        let drop_cb = drop_item_binding as *const c_void;
        g_signal_connect_data(item, c"activate".as_ptr(), cb, payload.cast(), drop_cb, 0);
        gtk_menu_shell_append(menu, item);
        item
    }
}

fn populate_forward_targets(sub: *mut GtkWidget, spec: &VteMenuSpec, dispatch: &DispatchFn, sub_out: &mut Vec<*mut GtkWidget>) {
    for target in &spec.targets {
        let Ok(lbl_c) = CString::new(target.label.as_str()) else { continue };
        let act = ContextAction::ForwardToTab {
            source_sid: spec.sid,
            source_vte: spec.vte,
            target_sid: target.sid,
            selected_text: spec.selected_text.clone(),
        };
        sub_out.push(append_bound_item(sub, lbl_c.as_ptr(), true, act, dispatch));
    }
}

fn append_forward_submenu(menu: *mut GtkWidget, spec: &VteMenuSpec, dispatch: &DispatchFn, items: &mut Vec<*mut GtkWidget>, sub_out: &mut Vec<*mut GtkWidget>) {
    let parent = unsafe { gtk_menu_item_new_with_label(text::ctx_send_to_tab().as_ptr()) };
    let enabled = !spec.targets.is_empty();
    unsafe { gtk_widget_set_sensitive(parent, c_int::from(enabled)) };
    if enabled {
        let sub = unsafe { gtk_menu_new() };
        add_class(sub, "context-menu");
        populate_forward_targets(sub, spec, dispatch, sub_out);
        unsafe { gtk_menu_item_set_submenu(parent, sub) };
    }
    unsafe { gtk_menu_shell_append(menu, parent) };
    items.push(parent);
}

fn append_vte_link_items(menu: *mut GtkWidget, link_target: Option<&str>, dispatch: &DispatchFn, items: &mut Vec<*mut GtkWidget>) {
    let Some(url) = link_target else {
        return;
    };
    append_separator(menu);
    let open_act = ContextAction::OpenLink(url.to_owned());
    items.push(append_bound_item(menu, text::ctx_open_link().as_ptr(), true, open_act, dispatch));
    let copy_act = ContextAction::CopyText(url.to_owned());
    items.push(append_bound_item(menu, text::ctx_copy_link().as_ptr(), true, copy_act, dispatch));
    if let Some(local_path) = resolve_local_link_path(url) {
        let folder_act = ContextAction::OpenContainingFolder(local_path);
        items.push(append_bound_item(menu, text::ctx_open_folder().as_ptr(), true, folder_act, dispatch));
    }
}

fn append_vte_pane_items(menu: *mut GtkWidget, spec: &VteMenuSpec, dispatch: &DispatchFn, items: &mut Vec<*mut GtkWidget>) {
    let local = spec.is_local;
    let right = ContextAction::SplitRight { sid: spec.sid, vte: spec.vte };
    items.push(append_bound_item(menu, text::ctx_split_right().as_ptr(), local, right, dispatch));
    let down = ContextAction::SplitDown { sid: spec.sid, vte: spec.vte };
    items.push(append_bound_item(menu, text::ctx_split_down().as_ptr(), local, down, dispatch));
    let close = ContextAction::ClosePane { sid: spec.sid, vte: spec.vte };
    items.push(append_bound_item(menu, text::ctx_close_pane().as_ptr(), local, close, dispatch));
}

pub fn build_vte_menu(spec: &VteMenuSpec, dispatch: &DispatchFn) -> BuiltMenu {
    let menu = new_styled_menu();
    let mut items = Vec::with_capacity(10);
    let mut submenu_items = Vec::with_capacity(spec.targets.len());
    items.push(append_bound_item(menu, text::ctx_copy().as_ptr(), spec.has_selection, ContextAction::CopySelection(spec.vte), dispatch));
    items.push(append_bound_item(menu, text::ctx_paste().as_ptr(), true, ContextAction::PasteSmart { sid: spec.sid, vte: spec.vte }, dispatch));
    append_vte_link_items(menu, spec.link_target.as_deref(), dispatch, &mut items);
    append_separator(menu);
    append_forward_submenu(menu, spec, dispatch, &mut items, &mut submenu_items);
    append_separator(menu);
    append_vte_pane_items(menu, spec, dispatch, &mut items);
    append_separator(menu);
    items.push(append_bound_item(menu, text::ctx_clear_buffer().as_ptr(), true, ContextAction::ClearBuffer(spec.vte), dispatch));
    BuiltMenu { menu, items, submenu_items }
}

pub fn build_tab_menu(spec: &TabMenuSpec, dispatch: &DispatchFn) -> BuiltMenu {
    let menu = new_styled_menu();
    let mut items = Vec::with_capacity(6);
    items.push(append_bound_item(menu, text::ctx_rename_tab().as_ptr(), true, ContextAction::RenameTab { sid: spec.sid, anchor: spec.anchor }, dispatch));
    items.push(append_bound_item(menu, text::ctx_duplicate_tab().as_ptr(), true, ContextAction::DuplicateTab(spec.sid), dispatch));
    let cwd = spec.local_cwd.clone().unwrap_or_default();
    items.push(append_bound_item(menu, text::ctx_copy_path().as_ptr(), spec.local_cwd.is_some(), ContextAction::CopyText(cwd), dispatch));
    items.push(append_bound_item(menu, text::reconnect().as_ptr(), spec.is_remote, ContextAction::ReconnectTab(spec.sid), dispatch));
    append_separator(menu);
    items.push(append_bound_item(menu, text::ctx_close_tab().as_ptr(), true, ContextAction::CloseTab(spec.sid), dispatch));
    items.push(append_bound_item(menu, text::ctx_close_other_tabs().as_ptr(), spec.can_close_others, ContextAction::CloseOtherTabs(spec.sid), dispatch));
    BuiltMenu { menu, items, submenu_items: Vec::new() }
}

fn replace_active_menu(menu: *mut GtkWidget) {
    let prev = ACTIVE_CTX_MENU.with(|slot| slot.replace(ptr::null_mut()));
    if !prev.is_null() {
        unsafe { gtk_widget_destroy(prev) };
    }
    ACTIVE_CTX_MENU.with(|slot| slot.set(menu));
}

fn popup_built_menu(built: BuiltMenu, attach_widget: *mut GtkWidget, event: *const GdkEvent) {
    if built.items.is_empty() && built.submenu_items.is_empty() {
        unsafe { gtk_widget_destroy(built.menu) };
        return;
    }
    let menu = built.menu;
    replace_active_menu(menu);
    unsafe {
        let done_cb = on_menu_selection_done as *const c_void;
        g_signal_connect_data(menu, c"selection-done".as_ptr(), done_cb, ptr::null_mut(), ptr::null(), 0);
        if !attach_widget.is_null() {
            gtk_menu_attach_to_widget(menu, attach_widget, ptr::null());
        }
        gtk_widget_show_all(menu);
        gtk_menu_popup_at_pointer(menu, event);
    }
}

pub fn popup_vte_menu(spec: &VteMenuSpec, event: *const GdkEvent, dispatch: DispatchFn) {
    let built = build_vte_menu(spec, &dispatch);
    let target = spec.vte;
    popup_built_menu(built, target, event);
}

pub fn popup_tab_menu(spec: &TabMenuSpec, event: *const GdkEvent, dispatch: DispatchFn) {
    let built = build_tab_menu(spec, &dispatch);
    let anchor = spec.anchor;
    popup_built_menu(built, anchor, event);
}

struct RenameSession {
    popover: *mut GtkWidget,
    entry: *mut GtkWidget,
    sid: usize,
    done: Cell<bool>,
    on_commit: Box<dyn Fn(usize, String)>,
    on_cancel: Box<dyn Fn()>,
}

extern "C" fn drop_rename_rc(data: *mut c_void, _closure: *mut c_void) {
    if data.is_null() {
        return;
    }
    unsafe { drop(Rc::from_raw(data.cast::<RenameSession>())) };
}

fn read_entry_string(entry: *mut GtkWidget) -> String {
    unsafe {
        let ptr = gtk_entry_get_text(entry);
        if ptr.is_null() {
            String::new()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}

extern "C" fn on_rename_submit(_widget: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let session = unsafe { &*data.cast::<RenameSession>() };
    if session.done.replace(true) {
        return;
    }
    let value = read_entry_string(session.entry);
    RENAME_OPEN.with(|open| open.set(false));
    unsafe { gtk_popover_popdown(session.popover) };
    (session.on_commit)(session.sid, value);
}

extern "C" fn on_rename_key_press(_entry: *mut GtkWidget, event: *const GdkEvent, data: *mut c_void) -> c_int {
    if data.is_null() || event.is_null() {
        return 0;
    }
    let mut keyval: c_uint = 0;
    if unsafe { gdk_event_get_keyval(event, &mut keyval) } == 0 || keyval != 0xff1b {
        return 0;
    }
    let session = unsafe { &*data.cast::<RenameSession>() };
    if !session.done.replace(true) {
        RENAME_OPEN.with(|open| open.set(false));
        unsafe { gtk_popover_popdown(session.popover) };
        (session.on_cancel)();
    }
    1
}

extern "C" fn on_rename_destroyed(pop: *mut GtkWidget, _data: *mut c_void) {
    ACTIVE_RENAME_POP.with(|slot| {
        if slot.get() == pop {
            RENAME_OPEN.with(|open| open.set(false));
            slot.set(ptr::null_mut());
        }
    });
}

extern "C" fn destroy_rename_idle(data: *mut c_void) -> c_int {
    let pop = data as *mut GtkWidget;
    let should_destroy = ACTIVE_RENAME_POP.with(|slot| {
        let matches = slot.get() == pop;
        if matches {
            slot.set(ptr::null_mut());
        }
        matches
    });
    if should_destroy && !pop.is_null() {
        unsafe { gtk_widget_destroy(pop) };
    }
    0
}

extern "C" fn on_rename_closed(pop: *mut GtkWidget, data: *mut c_void) {
    if ACTIVE_RENAME_POP.with(|slot| slot.get() == pop || slot.get().is_null()) {
        RENAME_OPEN.with(|open| open.set(false));
    }
    if !data.is_null() {
        let session = unsafe { &*data.cast::<RenameSession>() };
        if !session.done.replace(true) {
            (session.on_cancel)();
        }
    }
    if !pop.is_null() {
        unsafe { g_idle_add(destroy_rename_idle, pop as *mut c_void) };
    }
}

fn wire_rename_signal(widget: *mut GtkWidget, signal: &CStr, handler: *const c_void, session: &Rc<RenameSession>) {
    let raw = Rc::into_raw(Rc::clone(session)) as *mut c_void;
    let drop_cb = drop_rename_rc as *const c_void;
    unsafe {
        g_signal_connect_data(widget, signal.as_ptr(), handler, raw, drop_cb, 0);
    }
}

fn create_rename_row(current_title: &str) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let init_c = CString::new(current_title).unwrap_or_default();
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        add_class(row, "popover-box");
        let entry = gtk_entry_new();
        gtk_entry_set_placeholder_text(entry, text::rename_placeholder().as_ptr());
        gtk_entry_set_text(entry, init_c.as_ptr());
        gtk_widget_set_size_request(entry, 220, -1);
        let save_btn = gtk_button_new_with_label(text::rename_save().as_ptr());
        gtk_box_pack_start(row, entry, 1, 1, 0);
        gtk_box_pack_start(row, save_btn, 0, 0, 0);
        gtk_widget_show_all(row);
        (row, entry, save_btn)
    }
}

pub fn build_rename_popover(
    anchor: *mut GtkWidget,
    sid: usize,
    current_title: &str,
    on_commit: impl Fn(usize, String) + 'static,
    on_cancel: impl Fn() + 'static,
) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let pop = unsafe { gtk_popover_new(anchor) };
    let (row, entry, save_btn) = create_rename_row(current_title);
    unsafe { gtk_container_add(pop, row) };
    let session = Rc::new(RenameSession {
        popover: pop,
        entry,
        sid,
        done: Cell::new(false),
        on_commit: Box::new(on_commit),
        on_cancel: Box::new(on_cancel),
    });
    let destroy_cb = on_rename_destroyed as *const c_void;
    unsafe { g_signal_connect_data(pop, c"destroy".as_ptr(), destroy_cb, ptr::null_mut(), ptr::null(), 0) };
    wire_rename_signal(entry, c"activate", on_rename_submit as *const c_void, &session);
    wire_rename_signal(save_btn, c"clicked", on_rename_submit as *const c_void, &session);
    wire_rename_signal(entry, c"key-press-event", on_rename_key_press as *const c_void, &session);
    wire_rename_signal(pop, c"closed", on_rename_closed as *const c_void, &session);
    (pop, entry, save_btn)
}

pub fn popup_rename_popover(
    anchor: *mut GtkWidget,
    sid: usize,
    current_title: &str,
    on_commit: impl Fn(usize, String) + 'static,
    on_cancel: impl Fn() + 'static,
) {
    if anchor.is_null() {
        return;
    }
    dismiss_rename_popover();
    let (pop, entry, _) = build_rename_popover(anchor, sid, current_title, on_commit, on_cancel);
    ACTIVE_RENAME_POP.with(|slot| slot.set(pop));
    RENAME_OPEN.with(|open| open.set(true));
    unsafe {
        gtk_popover_popup(pop);
        gtk_widget_grab_focus(entry);
        gtk_editable_select_region(entry, 0, -1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use std::cell::RefCell;
    use std::thread;
    use std::time::Duration;

    fn item_label(item: *mut GtkWidget) -> String {
        let ptr = unsafe { gtk_menu_item_get_label(item) };
        if ptr.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
        }
    }

    fn is_sensitive(widget: *mut GtkWidget) -> bool {
        let flag = unsafe { gtk_widget_get_sensitive(widget) };
        flag != 0
    }

    #[test]
    fn shift_right_click_passes_through_and_target_labels_format_cleanly() {
        crate::i18n::with_locale_lock(|| {
            crate::i18n::set_locale(crate::i18n::UiLocale::Ru);
            const LOCK_AND_MOD2: u32 = (1 << 1) | (1 << 4);
            assert!(should_passthrough_vte_click(3, GDK_SHIFT_MASK));
            assert!(should_passthrough_vte_click(3, GDK_SHIFT_MASK | LOCK_AND_MOD2));
            assert!(!should_passthrough_vte_click(3, 0));
            assert!(!should_passthrough_vte_click(3, LOCK_AND_MOD2));
            assert!(!should_passthrough_vte_click(3, GDK_CONTROL_MASK));
            assert!(!should_passthrough_vte_click(1, GDK_SHIFT_MASK));
            let cur = format_session_target(2, "Проект", "", 2);
            assert_eq!(cur.label, "Проект (текущая)");
            let rem = format_session_target(5, "Claude", "devbox", 2);
            assert_eq!(rem.label, "Claude [devbox]");
            crate::i18n::set_locale(crate::i18n::UiLocale::En);
            assert_eq!(format_session_target(2, "Proj", "", 2).label, "Proj (current)");
        });
    }

    #[test]
    fn resolves_local_file_links_with_cyrillic_and_line_suffixes_and_skips_web_urls() {
        assert_eq!(resolve_local_link_path("https://example.org/docs"), None);
        assert_eq!(resolve_local_link_path("http://127.0.0.1:8080/api"), None);
        assert_eq!(resolve_local_link_path("mailto:user@example.com"), None);
        assert_eq!(resolve_local_link_path("/tmp/definitely_missing_akhterm_path_123"), None);

        let dir = std::env::temp_dir().join(format!("akhterm_link_{}", std::process::id()));
        let sub = dir.join("папка проекта");
        let file = sub.join("отчёт.rs");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let with_line = format!("{}:42:7", file.display());
        assert_eq!(resolve_local_link_path(&with_line), Some(file.clone()));
        let uri = crate::platform::encode_file_uri(&file).expect("file uri");
        assert_eq!(resolve_local_link_path(&uri), Some(file.clone()));
        assert_eq!(resolve_local_link_path(&sub.to_string_lossy()), Some(sub.clone()));

        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_menu_module_meets_grade_b_mi_target() {
        let (_, _, ctx_mi) = crate::mi::compute_file_mi(include_str!("context_menu.rs"));
        assert!(ctx_mi >= 58.0, "context_menu.rs MI {ctx_mi:.2} < 58.0");
        let (_, _, main_mi) = crate::mi::compute_file_mi(include_str!("main.rs"));
        assert!(main_mi >= 54.87, "main.rs MI {main_mi:.2} < 54.87");
    }

    fn check_remote_vte_menu(vte: *mut GtkWidget, dispatch: &DispatchFn) {
        let spec = VteMenuSpec {
            sid: 4, vte, has_selection: false, selected_text: None, link_target: None, is_local: false, targets: Vec::new(),
        };
        let built = build_vte_menu(&spec, dispatch);
        assert_eq!(built.items.len(), 7);
        assert_eq!(item_label(built.items[0]), "Копировать");
        assert!(!is_sensitive(built.items[0]));
        assert!(is_sensitive(built.items[1]));
        assert!(!is_sensitive(built.items[2]));
        assert!(!is_sensitive(built.items[3]));
        assert!(!is_sensitive(built.items[5]));
        assert!(is_sensitive(built.items[6]));
        unsafe { gtk_widget_destroy(built.menu) };
    }

    fn check_local_vte_menu(vte: *mut GtkWidget, dispatch: &DispatchFn, log: &Rc<RefCell<Vec<ContextAction>>>) {
        let spec = VteMenuSpec {
            sid: 2,
            vte,
            has_selection: true,
            selected_text: Some("ошибка".to_owned()),
            link_target: Some("https://example.org".to_owned()),
            is_local: true,
            targets: vec![SessionTarget { sid: 7, label: "Агент · Claude".to_owned() }],
        };
        let built = build_vte_menu(&spec, dispatch);
        assert_eq!(built.items.len(), 9);
        assert_eq!(item_label(built.items[2]), "Открыть ссылку");
        assert_eq!(item_label(built.submenu_items[0]), "Агент · Claude");
        unsafe {
            gtk_menu_item_activate(built.items[2]);
            gtk_menu_item_activate(built.submenu_items[0]);
            gtk_menu_item_activate(built.items[5]);
            gtk_widget_destroy(built.menu);
        }
        assert_eq!(log.borrow().len(), 3);
    }

    fn check_local_tab_menu(anchor: *mut GtkWidget, dispatch: &DispatchFn) {
        let local_tab = TabMenuSpec {
            sid: 3, anchor, local_cwd: Some("/tmp/проект".to_owned()), is_remote: false, can_close_others: false,
        };
        let built = build_tab_menu(&local_tab, dispatch);
        assert_eq!(built.items.len(), 6);
        assert!(is_sensitive(built.items[2]));
        assert!(!is_sensitive(built.items[3]));
        assert!(!is_sensitive(built.items[5]));
        unsafe {
            gtk_menu_item_activate(built.items[2]);
            gtk_widget_destroy(built.menu);
        }
    }

    fn check_remote_tab_menu(anchor: *mut GtkWidget, dispatch: &DispatchFn) {
        let remote_tab = TabMenuSpec {
            sid: 9, anchor, local_cwd: None, is_remote: true, can_close_others: true,
        };
        let built = build_tab_menu(&remote_tab, dispatch);
        assert!(!is_sensitive(built.items[2]));
        assert!(is_sensitive(built.items[3]));
        assert!(is_sensitive(built.items[5]));
        unsafe {
            gtk_menu_item_activate(built.items[3]);
            gtk_menu_item_activate(built.items[5]);
            gtk_widget_destroy(built.menu);
        }
    }

    fn check_tab_menus(anchor: *mut GtkWidget) {
        let log: Rc<RefCell<Vec<ContextAction>>> = Rc::new(RefCell::new(Vec::new()));
        let log_cb = Rc::clone(&log);
        let dispatch: DispatchFn = Rc::new(move |act| log_cb.borrow_mut().push(act));
        check_local_tab_menu(anchor, &dispatch);
        check_remote_tab_menu(anchor, &dispatch);
        assert_eq!(
            *log.borrow(),
            vec![
                ContextAction::CopyText("/tmp/проект".to_owned()),
                ContextAction::ReconnectTab(9),
                ContextAction::CloseOtherTabs(9),
            ]
        );
    }

    fn check_rename_popover(anchor: *mut GtkWidget) {
        popup_rename_popover(anchor, 4, "Черновик", |_, _| {}, || {});
        assert!(is_rename_open());
        let active_pop = ACTIVE_RENAME_POP.with(Cell::get);
        unsafe { gtk_popover_popdown(active_pop) };
        assert!(!is_rename_open());
        pump_gtk_events();
        let renamed: Rc<RefCell<Option<(usize, String)>>> = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&renamed);
        let (pop, entry, save_btn) = build_rename_popover(
            anchor, 3, "Старое имя", move |sid, name| *sink.borrow_mut() = Some((sid, name)), || {},
        );
        RENAME_OPEN.with(|open| open.set(true));
        assert_eq!(read_entry_string(entry), "Старое имя");
        let next_name = CString::new("Бэкенд ЦУ").unwrap();
        unsafe {
            gtk_entry_set_text(entry, next_name.as_ptr());
            gtk_button_clicked(save_btn);
            assert!(!is_rename_open());
            gtk_widget_destroy(pop);
        }
        assert_eq!(*renamed.borrow(), Some((3, "Бэкенд ЦУ".to_owned())));
    }

    fn read_clipboard_text() -> String {
        unsafe {
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            let raw = gtk_clipboard_wait_for_text(clip);
            assert!(!raw.is_null());
            let out = CStr::from_ptr(raw).to_string_lossy().into_owned();
            g_free(raw as *mut c_void);
            out
        }
    }

    fn check_smart_copy_persists_after_unselect(vte: *mut GtkWidget) {
        assert!(!copy_vte_selection_to_clipboard(ptr::null_mut(), true));
        unsafe {
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            let pix = gdk_pixbuf_new(0, 1, 8, 4, 4);
            gtk_clipboard_set_image(clip, pix);
            g_object_unref(pix);
            pump_gtk_events();
        }
        assert!(crate::proc_radar::clipboard_has_direct_image());
        unsafe { vte_terminal_select_all(vte) };
        pump_gtk_events();
        assert!(copy_vte_selection_to_clipboard(vte, false));
        assert!(unsafe { vte_terminal_get_has_selection(vte) != 0 });
        assert!(!crate::proc_radar::clipboard_has_direct_image());
        assert!(read_clipboard_text().contains("Привет из терминала"));
        assert!(copy_vte_selection_to_clipboard(vte, true));
        pump_gtk_events();
        assert_eq!(unsafe { vte_terminal_get_has_selection(vte) }, 0);
        assert!(read_clipboard_text().contains("Привет из терминала"));
        assert!(!copy_vte_selection_to_clipboard(vte, true));
    }

    fn check_vte_selection_and_clear(vte: *mut GtkWidget) {
        let sample = "Привет из терминала\r\nВторая строка\r\n";
        unsafe { vte_terminal_feed(vte, sample.as_ptr() as *const c_char, sample.len() as isize) };
        thread::sleep(Duration::from_millis(50));
        pump_gtk_events();
        assert_eq!(read_vte_selection(vte), None);
        assert!(!copy_vte_selection_to_clipboard(vte, true));
        check_smart_copy_persists_after_unselect(vte);
        unsafe { vte_terminal_reset(vte, 0, 1) };
        thread::sleep(Duration::from_millis(30));
        pump_gtk_events();
        assert!(extract_vte_text(vte).trim().is_empty());
    }

    fn check_cyrillic_clipboard() {
        copy_text_to_clipboard("/tmp/кириллический_путь");
        pump_gtk_events();
        assert_eq!(read_clipboard_text(), "/tmp/кириллический_путь");
    }

    fn build_link_test_spec(vte: *mut GtkWidget, link: String) -> VteMenuSpec {
        VteMenuSpec {
            sid: 2,
            vte,
            has_selection: false,
            selected_text: None,
            link_target: Some(link),
            is_local: true,
            targets: Vec::new(),
        }
    }

    fn verify_link_menu_en_ru(spec: &VteMenuSpec, dispatch: &DispatchFn) {
        crate::i18n::set_locale(crate::i18n::UiLocale::En);
        let en_built = build_vte_menu(spec, dispatch);
        assert_eq!(item_label(en_built.items[4]), "Open Containing Folder");
        unsafe { gtk_widget_destroy(en_built.menu) };
        crate::i18n::set_locale(crate::i18n::UiLocale::Ru);
    }

    fn check_local_file_link_menu(vte: *mut GtkWidget, dispatch: &DispatchFn, log: &Rc<RefCell<Vec<ContextAction>>>) {
        let dir = std::env::temp_dir().join(format!("akhterm_ctx_file_{}", std::process::id()));
        let file = dir.join("пример.rs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "fn ok() {}\n").unwrap();
        let spec = build_link_test_spec(vte, format!("{}:12:3", file.display()));
        let built = build_vte_menu(&spec, dispatch);
        assert_eq!(built.items.len(), 10);
        assert_eq!(item_label(built.items[4]), "Открыть папку");
        let before = log.borrow().len();
        unsafe { gtk_menu_item_activate(built.items[4]) };
        unsafe { gtk_widget_destroy(built.menu) };
        assert_eq!(log.borrow().get(before), Some(&ContextAction::OpenContainingFolder(file.clone())));
        verify_link_menu_en_ru(&spec, dispatch);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn headless_broadway_menus_selection_and_clear_buffer() {
        crate::i18n::with_locale_lock(|| {
            crate::i18n::set_locale(crate::i18n::UiLocale::Ru);
            with_headless_gtk(|| unsafe {
                let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
                gtk_window_set_default_size(win, 800, 600);
                let box_w = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
                let anchor = gtk_button_new_with_label(c"Вкладка".as_ptr());
                let vte = vte_terminal_new();
                gtk_box_pack_start(box_w, anchor, 0, 0, 0);
                gtk_box_pack_start(box_w, vte, 1, 1, 0);
                gtk_container_add(win, box_w);
                gtk_widget_show_all(win);
                pump_gtk_events();
                let log: Rc<RefCell<Vec<ContextAction>>> = Rc::new(RefCell::new(Vec::new()));
                let log_cb = Rc::clone(&log);
                let dispatch: DispatchFn = Rc::new(move |act| log_cb.borrow_mut().push(act));
                check_remote_vte_menu(vte, &dispatch);
                check_local_vte_menu(vte, &dispatch, &log);
                check_local_file_link_menu(vte, &dispatch, &log);
                check_tab_menus(anchor);
                check_rename_popover(anchor);
                check_vte_selection_and_clear(vte);
                check_cyrillic_clipboard();
                gtk_widget_destroy(win);
                pump_gtk_events();
            });
        });
    }
}
