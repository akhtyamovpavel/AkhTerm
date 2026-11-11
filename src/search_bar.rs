//! Compact VTE-2.91 PCRE2 terminal output & scrollback search bar (`Ctrl+Shift+F`).

use std::cell::RefCell;
use std::ffi::{c_int, c_uint, c_void, CStr, CString};
use std::ptr;
use std::rc::Rc;

use crate::ffi::*;
use crate::ui_text as text;

pub const KEYCODE_F: u16 = 41;

const PCRE2_META: &[char] = &[
    '\\', '^', '$', '.', '[', ']', '|', '(', ')', '?', '*', '+', '{', '}',
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchStatus {
    Empty,
    Found,
    NotFound,
    RegexError,
}

impl SearchStatus {
    pub fn label_cstr(self) -> &'static CStr {
        match self {
            Self::Empty => c"",
            Self::Found => text::SEARCH_FOUND,
            Self::NotFound => text::SEARCH_NOT_FOUND,
            Self::RegexError => text::SEARCH_REGEX_ERR,
        }
    }
}

fn is_search_keyval(keyval: c_uint) -> bool {
    matches!(
        keyval,
        0x46 | 0x66 | 0x06c1 | 0x06e1 | 0x0410 | 0x0430 | 0x0100_0410 | 0x0100_0430
    )
}

pub fn is_search_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    let mods_ok = (state & GDK_CONTROL_MASK) != 0
        && (state & GDK_SHIFT_MASK) != 0
        && (state & GDK_MOD1_MASK) == 0;
    mods_ok && (is_search_keyval(keyval) || keycode == KEYCODE_F)
}

pub fn escape_pcre2_literal(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        if ch == '\0' {
            out.push_str("\\x00");
        } else if PCRE2_META.contains(&ch) {
            out.push('\\');
            out.push(ch);
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn build_search_pattern(query: &str, regex_mode: bool) -> String {
    if regex_mode {
        query.to_owned()
    } else {
        escape_pcre2_literal(query)
    }
}

pub fn search_pcre2_flags(case_sensitive: bool) -> c_uint {
    if case_sensitive {
        PCRE2_MULTILINE_UTF
    } else {
        PCRE2_SEARCH_CASELESS
    }
}

pub fn compile_search_regex(
    query: &str,
    case_sensitive: bool,
    regex_mode: bool,
) -> Result<*mut VteRegex, SearchStatus> {
    if query.is_empty() {
        return Err(SearchStatus::Empty);
    }
    let pat = build_search_pattern(query, regex_mode);
    let Ok(pat_c) = CString::new(pat) else {
        return Err(SearchStatus::RegexError);
    };
    let flags = search_pcre2_flags(case_sensitive);
    let reg = unsafe { vte_regex_new_for_search(pat_c.as_ptr(), -1, flags, ptr::null_mut()) };
    if reg.is_null() {
        Err(SearchStatus::RegexError)
    } else {
        Ok(reg)
    }
}

pub fn clear_vte_search(vte: *mut GtkWidget) {
    if !vte.is_null() {
        unsafe {
            vte_terminal_search_set_regex(vte, ptr::null_mut(), 0);
            vte_terminal_unselect_all(vte);
        }
    }
}

fn install_regex_on_vte(vte: *mut GtkWidget, reg: *mut VteRegex) {
    unsafe {
        if !vte.is_null() {
            vte_terminal_search_set_wrap_around(vte, 1);
            vte_terminal_search_set_regex(vte, reg, 0);
        }
        vte_regex_unref(reg);
    }
}

fn apply_query_to_vte(
    vte: *mut GtkWidget,
    query: &str,
    case_sensitive: bool,
    regex_mode: bool,
) -> SearchStatus {
    let reg = match compile_search_regex(query, case_sensitive, regex_mode) {
        Ok(r) => r,
        Err(st) => {
            clear_vte_search(vte);
            return st;
        }
    };
    if vte.is_null() {
        unsafe { vte_regex_unref(reg) };
        return SearchStatus::NotFound;
    }
    install_regex_on_vte(vte, reg);
    let hit = unsafe {
        vte_terminal_unselect_all(vte);
        vte_terminal_search_find_previous(vte) != 0 || vte_terminal_search_find_next(vte) != 0
    };
    if hit { SearchStatus::Found } else { SearchStatus::NotFound }
}

struct SearchState {
    open: bool,
    case_sensitive: bool,
    regex_mode: bool,
    status: SearchStatus,
    last_vte: *mut GtkWidget,
}

fn new_search_state() -> SearchState {
    SearchState {
        open: false,
        case_sensitive: false,
        regex_mode: false,
        status: SearchStatus::Empty,
        last_vte: ptr::null_mut(),
    }
}

struct SearchWidgets {
    bar: *mut GtkWidget,
    entry: *mut GtkWidget,
    prev_btn: *mut GtkWidget,
    next_btn: *mut GtkWidget,
    case_btn: *mut GtkWidget,
    regex_btn: *mut GtkWidget,
    status_lbl: *mut GtkWidget,
    close_btn: *mut GtkWidget,
}

struct SearchCtx {
    w: SearchWidgets,
    state: RefCell<SearchState>,
    active_vte: Box<dyn Fn() -> *mut GtkWidget>,
    on_close_vtes: Box<dyn Fn() -> (Vec<*mut GtkWidget>, *mut GtkWidget)>,
}

#[derive(Clone)]
pub struct SearchBar {
    ctx: Rc<SearchCtx>,
}

fn read_entry_text(entry: *mut GtkWidget) -> String {
    unsafe {
        let ptr = gtk_entry_get_text(entry);
        if ptr.is_null() {
            String::new()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}

fn update_status_widgets(w: &SearchWidgets, status: SearchStatus) {
    unsafe { gtk_label_set_text(w.status_lbl, status.label_cstr().as_ptr()) };
    set_class(w.entry, "term-search-entry-err", status == SearchStatus::RegexError);
    set_class(w.status_lbl, "term-search-status-ok", status == SearchStatus::Found);
    set_class(w.status_lbl, "term-search-status-miss", status == SearchStatus::NotFound);
    set_class(w.status_lbl, "term-search-status-err", status == SearchStatus::RegexError);
}

fn snapshot_opts(ctx: &SearchCtx) -> Option<(bool, bool, *mut GtkWidget, SearchStatus)> {
    let st = ctx.state.try_borrow().ok()?;
    Some((st.case_sensitive, st.regex_mode, st.last_vte, st.status))
}

fn record_result(ctx: &SearchCtx, vte: *mut GtkWidget, status: SearchStatus) {
    if let Ok(mut st) = ctx.state.try_borrow_mut() {
        st.last_vte = vte;
        st.status = status;
    }
    update_status_widgets(&ctx.w, status);
}

fn refresh_on_vte(ctx: &SearchCtx, vte: *mut GtkWidget) {
    let Some((case_sens, regex_mode, _, _)) = snapshot_opts(ctx) else { return };
    let query = read_entry_text(ctx.w.entry);
    let status = apply_query_to_vte(vte, &query, case_sens, regex_mode);
    record_result(ctx, vte, status);
}

fn ensure_vte_ready(
    vte: *mut GtkWidget,
    last_vte: *mut GtkWidget,
    query: &str,
    case_sens: bool,
    regex_mode: bool,
) -> Result<(), SearchStatus> {
    if vte == last_vte {
        return Ok(());
    }
    let reg = compile_search_regex(query, case_sens, regex_mode)?;
    install_regex_on_vte(vte, reg);
    Ok(())
}

fn step_search(ctx: &SearchCtx, forward: bool) {
    let Some((case_sens, regex_mode, last_vte, prev_st)) = snapshot_opts(ctx) else { return };
    let vte = (ctx.active_vte)();
    let query = read_entry_text(ctx.w.entry);
    if query.is_empty() {
        clear_vte_search(vte);
        return record_result(ctx, vte, SearchStatus::Empty);
    }
    if prev_st == SearchStatus::RegexError && vte == last_vte {
        return update_status_widgets(&ctx.w, SearchStatus::RegexError);
    }
    if vte.is_null() {
        return record_result(ctx, vte, SearchStatus::NotFound);
    }
    if let Err(err) = ensure_vte_ready(vte, last_vte, &query, case_sens, regex_mode) {
        clear_vte_search(vte);
        return record_result(ctx, vte, err);
    }
    let found = unsafe {
        if forward {
            vte_terminal_search_find_next(vte) != 0
        } else {
            vte_terminal_search_find_previous(vte) != 0
        }
    };
    let next_st = if found { SearchStatus::Found } else { SearchStatus::NotFound };
    record_result(ctx, vte, next_st);
}

fn close_bar(ctx: &SearchCtx) {
    if let Ok(mut st) = ctx.state.try_borrow_mut() {
        st.open = false;
        st.last_vte = ptr::null_mut();
        st.status = SearchStatus::Empty;
    }
    let (vtes, focus_vte) = (ctx.on_close_vtes)();
    for vte in vtes {
        clear_vte_search(vte);
    }
    update_status_widgets(&ctx.w, SearchStatus::Empty);
    unsafe {
        gtk_widget_set_visible(ctx.w.bar, 0);
        if !focus_vte.is_null() {
            gtk_widget_grab_focus(focus_vte);
        }
    }
}

fn open_bar(ctx: &SearchCtx) {
    if let Ok(mut st) = ctx.state.try_borrow_mut() {
        st.open = true;
    }
    let vte = (ctx.active_vte)();
    unsafe {
        gtk_widget_set_visible(ctx.w.bar, 1);
        gtk_widget_grab_focus(ctx.w.entry);
        gtk_editable_select_region(ctx.w.entry, 0, -1);
    }
    refresh_on_vte(ctx, vte);
}

fn handle_entry_key(ctx: &SearchCtx, keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if is_search_shortcut(keyval, state, keycode) || keyval == 0xff1b {
        close_bar(ctx);
        return true;
    }
    if matches!(keyval, 0xff0d | 0xff8d | 0xfe34) {
        let backward = (state & GDK_SHIFT_MASK) != 0;
        step_search(ctx, !backward);
        return true;
    }
    false
}

extern "C" fn on_entry_changed(_entry: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const SearchCtx) };
    let is_open = ctx.state.try_borrow().is_ok_and(|st| st.open);
    if is_open {
        let vte = (ctx.active_vte)();
        refresh_on_vte(ctx, vte);
    }
}

extern "C" fn on_entry_key_press(
    _entry: *mut GtkWidget,
    event: *const GdkEvent,
    data: *mut c_void,
) -> c_int {
    let mut keyval: c_uint = 0;
    let mut state: c_uint = 0;
    let mut keycode: u16 = 0;
    unsafe {
        if gdk_event_get_keyval(event, &mut keyval) == 0 {
            return 0;
        }
        gdk_event_get_state(event, &mut state);
        gdk_event_get_keycode(event, &mut keycode);
    }
    let ctx = unsafe { &*(data as *const SearchCtx) };
    i32::from(handle_entry_key(ctx, keyval, state, keycode))
}

extern "C" fn on_prev_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const SearchCtx) };
    step_search(ctx, false);
}

extern "C" fn on_next_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const SearchCtx) };
    step_search(ctx, true);
}

fn apply_toggle_change(ctx: &SearchCtx, btn: *mut GtkWidget, is_case: bool) {
    let active = unsafe { gtk_toggle_button_get_active(btn) != 0 };
    set_class(btn, "term-search-toggle-active", active);
    let mut is_open = false;
    if let Ok(mut st) = ctx.state.try_borrow_mut() {
        if is_case {
            st.case_sensitive = active;
        } else {
            st.regex_mode = active;
        }
        st.last_vte = ptr::null_mut();
        is_open = st.open;
    }
    if is_open {
        let vte = (ctx.active_vte)();
        refresh_on_vte(ctx, vte);
    }
}

extern "C" fn on_case_toggled(btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const SearchCtx) };
    apply_toggle_change(ctx, btn, true);
}

extern "C" fn on_regex_toggled(btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const SearchCtx) };
    apply_toggle_change(ctx, btn, false);
}

extern "C" fn on_close_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const SearchCtx) };
    close_bar(ctx);
}

fn make_action_button(label: &CStr, tip: &CStr, cls: &str) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new_with_label(label.as_ptr());
        gtk_widget_set_can_focus(btn, 0);
        gtk_widget_set_tooltip_text(btn, tip.as_ptr());
        add_class(btn, cls);
        btn
    }
}

fn make_toggle_button(label: &CStr, tip: &CStr) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_toggle_button_new_with_label(label.as_ptr());
        gtk_widget_set_can_focus(btn, 0);
        gtk_widget_set_tooltip_text(btn, tip.as_ptr());
        add_class(btn, "term-search-toggle");
        btn
    }
}

fn build_search_widgets() -> SearchWidgets {
    let w = unsafe {
        let bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        add_class(bar, "term-search-bar");
        let entry = gtk_search_entry_new();
        add_class(entry, "term-search-entry");
        gtk_entry_set_placeholder_text(entry, text::SEARCH_PLACEHOLDER.as_ptr());
        let prev_btn = make_action_button(text::SEARCH_PREV, text::SEARCH_PREV_TIP, "term-search-btn");
        let next_btn = make_action_button(text::SEARCH_NEXT, text::SEARCH_NEXT_TIP, "term-search-btn");
        let case_btn = make_toggle_button(text::SEARCH_CASE, text::SEARCH_CASE_TIP);
        let regex_btn = make_toggle_button(text::SEARCH_REGEX, text::SEARCH_REGEX_TIP);
        let status_lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(status_lbl, 0.0);
        add_class(status_lbl, "term-search-status");
        let close_btn = make_action_button(text::CLOSE_TAB, text::SEARCH_CLOSE_TIP, "term-search-close");
        SearchWidgets { bar, entry, prev_btn, next_btn, case_btn, regex_btn, status_lbl, close_btn }
    };
    pack_search_children(&w);
    w
}

fn pack_search_children(w: &SearchWidgets) {
    unsafe {
        gtk_box_pack_start(w.bar, w.entry, 0, 1, 0);
        gtk_box_pack_start(w.bar, w.prev_btn, 0, 0, 0);
        gtk_box_pack_start(w.bar, w.next_btn, 0, 0, 0);
        gtk_box_pack_start(w.bar, w.case_btn, 0, 0, 0);
        gtk_box_pack_start(w.bar, w.regex_btn, 0, 0, 0);
        gtk_box_pack_start(w.bar, w.status_lbl, 0, 0, 0);
        gtk_box_pack_end(w.bar, w.close_btn, 0, 0, 0);
        gtk_widget_show_all(w.bar);
        gtk_widget_set_visible(w.bar, 0);
        gtk_widget_set_no_show_all(w.bar, 1);
    }
}

fn wire_search_signals(ctx: &Rc<SearchCtx>) {
    let ptr = Rc::as_ptr(ctx) as *mut c_void;
    let w = &ctx.w;
    unsafe {
        g_signal_connect_data(w.entry, c"changed".as_ptr(), on_entry_changed as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(w.entry, c"key-press-event".as_ptr(), on_entry_key_press as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(w.prev_btn, c"clicked".as_ptr(), on_prev_clicked as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(w.next_btn, c"clicked".as_ptr(), on_next_clicked as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(w.case_btn, c"toggled".as_ptr(), on_case_toggled as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(w.regex_btn, c"toggled".as_ptr(), on_regex_toggled as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(w.close_btn, c"clicked".as_ptr(), on_close_clicked as *const c_void, ptr, ptr::null(), 0);
    }
}

impl SearchBar {
    pub fn new(
        active_vte: impl Fn() -> *mut GtkWidget + 'static,
        on_close_vtes: impl Fn() -> (Vec<*mut GtkWidget>, *mut GtkWidget) + 'static,
    ) -> Self {
        let ctx = Rc::new(SearchCtx {
            w: build_search_widgets(),
            state: RefCell::new(new_search_state()),
            active_vte: Box::new(active_vte),
            on_close_vtes: Box::new(on_close_vtes),
        });
        wire_search_signals(&ctx);
        Self { ctx }
    }

    pub fn widget(&self) -> *mut GtkWidget {
        self.ctx.w.bar
    }

    pub fn is_open(&self) -> bool {
        self.ctx.state.try_borrow().is_ok_and(|st| st.open)
    }

    pub fn entry_has_focus(&self) -> bool {
        let entry = self.ctx.w.entry;
        !entry.is_null()
            && unsafe { gtk_widget_is_focus(entry) != 0 || gtk_widget_has_focus(entry) != 0 }
    }

    pub fn open(&self) {
        open_bar(&self.ctx);
    }

    pub fn close(&self) {
        close_bar(&self.ctx);
    }

    pub fn toggle(&self) {
        if self.is_open() && self.entry_has_focus() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn sync_active_vte(&self, vte: *mut GtkWidget) {
        if self.is_open() && !vte.is_null() {
            refresh_on_vte(&self.ctx, vte);
        }
    }

    #[cfg(test)]
    pub fn set_query(&self, query: &str) {
        if let Ok(c) = CString::new(query) {
            unsafe { gtk_entry_set_text(self.ctx.w.entry, c.as_ptr()) };
        }
    }

    #[cfg(test)]
    pub fn set_case_sensitive(&self, active: bool) {
        unsafe { gtk_toggle_button_set_active(self.ctx.w.case_btn, i32::from(active)) };
    }

    #[cfg(test)]
    pub fn set_regex_mode(&self, active: bool) {
        unsafe { gtk_toggle_button_set_active(self.ctx.w.regex_btn, i32::from(active)) };
    }

    #[cfg(test)]
    pub fn handle_key(&self, keyval: c_uint, state: c_uint, keycode: u16) -> bool {
        handle_entry_key(&self.ctx, keyval, state, keycode)
    }

    #[cfg(test)]
    pub fn status(&self) -> SearchStatus {
        self.ctx.state.try_borrow().map_or(SearchStatus::Empty, |st| st.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split_pane::{wrap_vte_scroll, TabPanes};
    use std::cell::Cell;
    use std::ffi::c_char;
    use std::fs;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn escapes_all_pcre2_metacharacters_and_preserves_cyrillic() {
        let raw = r"[ERR] (a+b)* $1 ^start end$ {2,3}? foo|bar.\E ошибка";
        let escaped = escape_pcre2_literal(raw);
        assert_eq!(
            escaped,
            r"\[ERR\] \(a\+b\)\* \$1 \^start end\$ \{2,3\}\? foo\|bar\.\\E ошибка"
        );
        assert_eq!(build_search_pattern("a+b", false), r"a\+b");
        assert_eq!(build_search_pattern("a+b", true), "a+b");
        assert_eq!(escape_pcre2_literal("a\0b"), r"a\x00b");
    }

    #[test]
    fn computes_pcre2_search_flags_and_compiles_vte_search_regex() {
        assert_eq!(search_pcre2_flags(false), 0x0008_0408);
        assert_eq!(search_pcre2_flags(true), 0x0008_0400);
        assert_eq!(compile_search_regex("", false, false), Err(SearchStatus::Empty));
        assert_eq!(compile_search_regex("[unclosed", false, true), Err(SearchStatus::RegexError));
        let lit_reg = compile_search_regex("[unclosed", false, false).expect("escaped literal compiles");
        unsafe { vte_regex_unref(lit_reg) };
        let rx_reg = compile_search_regex(r"ERR_[0-9]+", true, true).expect("valid regex compiles");
        unsafe { vte_regex_unref(rx_reg) };
    }

    #[test]
    fn matches_ctrl_shift_f_across_latin_cyrillic_and_keycode_41() {
        let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
        assert!(is_search_shortcut(u32::from(b'F'), cs, 0));
        assert!(is_search_shortcut(u32::from(b'f'), cs, 0));
        assert!(is_search_shortcut(0x06c1, cs, 0));
        assert!(is_search_shortcut(0x06e1, cs, 0));
        assert!(is_search_shortcut(0x0410, cs, 0));
        assert!(is_search_shortcut(0x0100_0430, cs, 0));
        assert!(is_search_shortcut(0x06e1, cs, KEYCODE_F));
        assert!(!is_search_shortcut(u32::from(b'f'), GDK_CONTROL_MASK, KEYCODE_F));
        assert!(!is_search_shortcut(u32::from(b'F'), GDK_SHIFT_MASK, KEYCODE_F));
        assert!(!is_search_shortcut(u32::from(b'F'), cs | GDK_MOD1_MASK, KEYCODE_F));
        assert!(!is_search_shortcut(u32::from(b'P'), cs, 33));
    }

    #[test]
    fn search_bar_module_maintains_grade_b_mi_score() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("search_bar.rs"));
        assert!(mi >= 58.0, "expected search_bar.rs MI >= 58.0, got {mi:.2}");
    }

    fn pump_events() {
        for _ in 0..80 {
            if unsafe { gtk_events_pending() } == 0 {
                break;
            }
            unsafe { gtk_main_iteration_do(0) };
        }
    }

    fn feed_vte(vte: *mut GtkWidget, payload: &str) {
        unsafe {
            vte_terminal_feed(vte, payload.as_ptr() as *const c_char, payload.len() as isize);
        }
        thread::sleep(Duration::from_millis(20));
        pump_events();
    }

    fn read_selected_vte_text(vte: *mut GtkWidget) -> String {
        unsafe {
            vte_terminal_copy_clipboard_format(vte, VTE_FORMAT_TEXT);
            pump_events();
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            let raw = gtk_clipboard_wait_for_text(clip);
            if raw.is_null() {
                return String::new();
            }
            let text = CStr::from_ptr(raw).to_string_lossy().into_owned();
            g_free(raw as *mut c_void);
            text
        }
    }

    fn read_status_label(bar: &SearchBar) -> String {
        unsafe {
            let raw = gtk_label_get_text(bar.ctx.w.status_lbl);
            if raw.is_null() {
                String::new()
            } else {
                CStr::from_ptr(raw).to_string_lossy().into_owned()
            }
        }
    }

    fn spawn_isolated_broadway(dir: &Path) -> Option<Child> {
        let http_sock = dir.join("http.sock");
        let child = Command::new("broadwayd")
            .args(["-u", &http_sock.to_string_lossy(), ":19"])
            .env("XDG_RUNTIME_DIR", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let gdk_sock = dir.join("broadway20.socket");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !gdk_sock.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        std::env::set_var("XDG_RUNTIME_DIR", dir);
        std::env::set_var("GDK_BACKEND", "broadway");
        std::env::set_var("BROADWAY_DISPLAY", ":19");
        (unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } != 0).then_some(child)
    }

    fn build_scrollback_fixture() -> String {
        let mut buf = String::new();
        for idx in 0..110 {
            match idx {
                4 => buf.push_str("row_004 MARKER_ONE сбой [a+b]\r\n"),
                52 => buf.push_str("row_052 MARKER_TWO промежуточный\r\n"),
                105 => buf.push_str("row_105 MARKER_THREE финал\r\n"),
                _ => buf.push_str(&format!("row_{idx:03} обычный лог сборки\r\n")),
            }
        }
        buf
    }

    fn verify_literal_and_case_toggle(bar: &SearchBar, vte1: *mut GtkWidget) {
        bar.open();
        pump_events();
        assert!(bar.is_open());
        assert_eq!(unsafe { gtk_widget_get_visible(bar.widget()) }, 1);

        bar.set_query("СБОЙ [a+b]");
        pump_events();
        assert_eq!(bar.status(), SearchStatus::Found);
        assert_eq!(read_status_label(bar), "Найдено");
        assert_ne!(unsafe { vte_terminal_get_has_selection(vte1) }, 0);
        assert_eq!(read_selected_vte_text(vte1), "сбой [a+b]");

        bar.set_case_sensitive(true);
        pump_events();
        assert_eq!(bar.status(), SearchStatus::NotFound);
        assert_eq!(read_status_label(bar), "Нет совпадений");
        assert_eq!(unsafe { vte_terminal_get_has_selection(vte1) }, 0);

        bar.set_case_sensitive(false);
        pump_events();
        assert_eq!(bar.status(), SearchStatus::Found);
    }

    fn verify_regex_stepping_and_error(bar: &SearchBar, vte1: *mut GtkWidget) {
        bar.set_regex_mode(true);
        bar.set_query("MARKER_[A-Z]+");
        pump_events();
        assert_eq!(bar.status(), SearchStatus::Found);
        assert_eq!(read_selected_vte_text(vte1), "MARKER_ONE");

        assert!(bar.handle_key(0xff0d, 0, 0));
        pump_events();
        assert_eq!(read_selected_vte_text(vte1), "MARKER_TWO");

        assert!(bar.handle_key(0xff0d, 0, 0));
        pump_events();
        assert_eq!(read_selected_vte_text(vte1), "MARKER_THREE");

        assert!(bar.handle_key(0xff0d, 0, 0));
        pump_events();
        assert_eq!(read_selected_vte_text(vte1), "MARKER_ONE");

        assert!(bar.handle_key(0xff0d, GDK_SHIFT_MASK, 0));
        pump_events();
        assert_eq!(read_selected_vte_text(vte1), "MARKER_THREE");

        assert!(bar.handle_key(0xff0d, GDK_SHIFT_MASK, 0));
        pump_events();
        assert_eq!(read_selected_vte_text(vte1), "MARKER_TWO");

        let ctx_ptr = Rc::as_ptr(&bar.ctx) as *mut c_void;
        on_prev_clicked(bar.ctx.w.prev_btn, ctx_ptr);
        pump_events();
        assert_eq!(read_selected_vte_text(vte1), "MARKER_ONE");
        on_next_clicked(bar.ctx.w.next_btn, ctx_ptr);
        pump_events();
        assert_eq!(read_selected_vte_text(vte1), "MARKER_TWO");

        bar.set_query("[unclosed");
        pump_events();
        assert_eq!(bar.status(), SearchStatus::RegexError);
        assert_eq!(read_status_label(bar), "Ошибка regex");
        assert_eq!(unsafe { vte_terminal_get_has_selection(vte1) }, 0);
        bar.set_regex_mode(false);
    }

    fn verify_split_pane_and_escape_focus(
        bar: &SearchBar,
        win: *mut GtkWidget,
        vte1: *mut GtkWidget,
        vte2: *mut GtkWidget,
        focused: &Rc<Cell<*mut GtkWidget>>,
    ) {
        focused.set(vte2);
        unsafe { gtk_widget_grab_focus(vte2) };
        pump_events();
        bar.toggle();
        pump_events();
        assert!(bar.is_open());
        assert_eq!(unsafe { gtk_window_get_focus(win) }, bar.ctx.w.entry);

        bar.set_query("ВТОРАЯ_ПАНЕЛЬ");
        pump_events();
        assert_eq!(bar.status(), SearchStatus::Found);
        assert_eq!(read_selected_vte_text(vte2), "ВТОРАЯ_ПАНЕЛЬ");

        assert!(bar.handle_key(0xff1b, 0, 0));
        pump_events();
        assert!(!bar.is_open());
        assert_eq!(unsafe { gtk_widget_get_visible(bar.widget()) }, 0);
        assert_eq!(unsafe { vte_terminal_get_has_selection(vte1) }, 0);
        assert_eq!(unsafe { vte_terminal_get_has_selection(vte2) }, 0);
        assert_eq!(unsafe { gtk_window_get_focus(win) }, vte2);

        bar.toggle();
        pump_events();
        assert!(bar.is_open());
        bar.toggle();
        pump_events();
        assert!(!bar.is_open());
        assert_eq!(unsafe { gtk_window_get_focus(win) }, vte2);

        bar.open();
        pump_events();
        on_close_clicked(bar.ctx.w.close_btn, Rc::as_ptr(&bar.ctx) as *mut c_void);
        pump_events();
        assert!(!bar.is_open());
        assert_eq!(unsafe { gtk_window_get_focus(win) }, vte2);
    }

    fn setup_broadway_window() -> (*mut GtkWidget, *mut GtkWidget, TabPanes, *mut GtkWidget, *mut GtkWidget) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            gtk_window_set_default_size(win, 900, 520);
            let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
            let vte1 = vte_terminal_new();
            vte_terminal_set_scrollback_lines(vte1, 5000);
            let scroll1 = wrap_vte_scroll(vte1);
            let mut panes = TabPanes::new(scroll1, vte1);
            let vte2 = vte_terminal_new();
            panes.split(scroll1, vte1, vte2, 0, "/tmp".into(), GTK_ORIENTATION_HORIZONTAL);
            gtk_container_add(win, vbox);
            (win, vbox, panes, vte1, vte2)
        }
    }

    fn run_broadway_search_checks() {
        let dir = std::env::temp_dir().join(format!("akhterm_search_bway_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let Some(mut child) = spawn_isolated_broadway(&dir) else {
            let _ = fs::remove_dir_all(&dir);
            return;
        };
        unsafe { crate::theme::apply_gtk_theme() };
        let (win, vbox, panes, vte1, vte2) = setup_broadway_window();
        let focused = Rc::new(Cell::new(vte1));
        let f_act = Rc::clone(&focused);
        let f_cls = Rc::clone(&focused);
        let bar = SearchBar::new(
            move || f_act.get(),
            move || (vec![vte1, vte2], f_cls.get()),
        );
        unsafe {
            gtk_box_pack_start(vbox, bar.widget(), 0, 1, 0);
            gtk_box_pack_start(vbox, panes.root, 1, 1, 0);
            gtk_widget_show_all(win);
            gtk_widget_grab_focus(vte1);
        }
        pump_events();
        assert_eq!(unsafe { gtk_widget_get_visible(bar.widget()) }, 0);
        feed_vte(vte1, &build_scrollback_fixture());
        feed_vte(vte2, "лог второй панели: ВТОРАЯ_ПАНЕЛЬ готова\r\n");
        verify_literal_and_case_toggle(&bar, vte1);
        verify_regex_stepping_and_error(&bar, vte1);
        verify_split_pane_and_escape_focus(&bar, win, vte1, vte2, &focused);
        unsafe { gtk_widget_destroy(win) };
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn headless_broadway_vte_scrollback_search_toggles_and_escape_focus() {
        if std::env::var_os("AKHTERM_SEARCH_BWAY_CHILD").is_some() {
            run_broadway_search_checks();
            return;
        }
        let exe = std::env::current_exe().expect("test binary path");
        let status = Command::new(exe)
            .args([
                "--exact",
                "search_bar::tests::headless_broadway_vte_scrollback_search_toggles_and_escape_focus",
            ])
            .env("AKHTERM_SEARCH_BWAY_CHILD", "1")
            .status()
            .expect("spawn broadway search bar test");
        assert!(status.success());
    }
}
