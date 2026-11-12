//! Swarm Dashboard (`Ctrl+Shift+S`), Topbar `Рой ▾` popover, Quick Approve & Broadcast.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::ptr;
use std::rc::Rc;
use std::time::Instant;

use crate::app_state::{focus_active_vte, set_footer_message, with_app};
use crate::approval::read_vte_visible_text;
use crate::ffi::*;
use crate::git_wt::detect_git_repo;
use crate::i18n::{UiLocale, UiText};
use crate::session::{TermSession, WorkState};
use crate::session_ops::select_session;
use crate::task_dispatch::open_for_app;

pub const SWARM_STACK_NAME: &str = "swarm_dashboard";
const SWARM_STACK_CSTR: &CStr = c"swarm_dashboard";

const TITLE_SWARM: UiText = UiText::new(c"Рой агентов", c"Agent Swarm");
const BTN_NEW_TASK: UiText = UiText::new(c"+ Новая задача", c"+ Task Dispatch");
const BTN_CLOSE_ESC: UiText = UiText::new(c"Назад (Esc)", c"Back (Esc)");
const BTN_OPEN_DASHBOARD: UiText = UiText::new(
    c"Открыть доску (Ctrl+Shift+S)",
    c"Open Dashboard (Ctrl+Shift+S)",
);
const BTN_SELECT_ALL: UiText = UiText::new(c"Выбрать все", c"Select All");
const BTN_SELECT_IDLE: UiText = UiText::new(c"Только свободные", c"Idle only");
const BTN_SELECT_CLEAR: UiText = UiText::new(c"Снять", c"Clear");
const BTN_BROADCAST: UiText = UiText::new(c"Отправить выбранным", c"Broadcast");
const BTN_FOCUS: UiText = UiText::new(c"Перейти", c"Focus");
const BTN_APPROVE_YN: UiText = UiText::new(c"✓ y / Enter", c"✓ y / Enter");
const BTN_APPROVE_CR: UiText = UiText::new(c"✓ Enter", c"✓ Enter");
const PLACEHOLDER_BROADCAST: UiText = UiText::new(
    c"Сообщение для отмеченных агентов...",
    c"Prompt for checked agent panes...",
);
const EMPTY_SWARM_TEXT: UiText = UiText::new(
    c"Нет активных вкладок агентов",
    c"No active agent panes",
);
const STATE_IDLE: UiText = UiText::new(c"Свободен", c"Idle");
const STATE_WORKING: UiText = UiText::new(c"Работает", c"Working");
const STATE_READY: UiText = UiText::new(c"Готов", c"Ready");
const STATE_APPROVAL: UiText = UiText::new(c"Ждёт подтверждения", c"Needs Approval");

extern "C" {
    fn gtk_check_button_new_with_label(label: *const c_char) -> *mut GtkWidget;
    fn gtk_stack_get_visible_child_name(stack: *mut GtkStack) -> *const c_char;
    fn gtk_label_set_line_wrap(label: *mut GtkLabel, wrap: c_int);
    fn gtk_widget_set_margin_start(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_margin_end(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_margin_top(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_margin_bottom(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_vexpand(widget: *mut GtkWidget, expand: c_int);
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwarmCardSpec {
    pub sid: usize,
    pub vte: *mut GtkWidget,
    pub agent: String,
    pub repo_name: String,
    pub branch_badge: String,
    pub is_worktree: bool,
    pub state: WorkState,
    pub elapsed_secs: Option<u64>,
    pub context_badge: Option<String>,
    pub cost_usd: Option<f64>,
    pub preview_lines: Vec<String>,
    pub approval_line: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BroadcastFilter {
    All,
    IdleOnly,
    Clear,
}

#[derive(Clone)]
pub struct SwarmCallbacks {
    pub on_focus: Rc<dyn Fn(usize, *mut GtkWidget)>,
    pub on_approve: Rc<dyn Fn(usize, *mut GtkWidget, &[u8])>,
    pub on_broadcast: Rc<dyn Fn(&[(usize, *mut GtkWidget)], &str)>,
    pub on_close: Rc<dyn Fn()>,
    pub on_open_dispatch: Rc<dyn Fn()>,
    pub on_open_dashboard: Rc<dyn Fn()>,
}

impl Default for SwarmCallbacks {
    fn default() -> Self {
        Self {
            on_focus: Rc::new(|_, _| {}),
            on_approve: Rc::new(|_, _, _| {}),
            on_broadcast: Rc::new(|_, _| {}),
            on_close: Rc::new(|| {}),
            on_open_dispatch: Rc::new(|| {}),
            on_open_dashboard: Rc::new(|| {}),
        }
    }
}

pub fn is_swarm_shortcut(keyval: u32, state: u32, hardware_keycode: u16) -> bool {
    let ctrl_shift = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
    let mods_ok = (state & ctrl_shift) == ctrl_shift && (state & GDK_MOD1_MASK) == 0;
    let is_s_key = matches!(keyval, 0x053 | 0x073 | 0x6d9 | 0x6f9) || hardware_keycode == 39;
    mods_ok && is_s_key
}

pub fn format_swarm_badge(total: usize, approvals: usize, locale: UiLocale) -> String {
    let prefix = if locale == UiLocale::En { "Swarm" } else { "Рой" };
    if total == 0 {
        return format!("{prefix} ▾");
    }
    if approvals > 0 {
        return format!("{prefix} · {total} (!{approvals})");
    }
    format!("{prefix} · {total}")
}

pub fn approval_response_bytes(approval_line: Option<&str>) -> &'static [u8] {
    let text = approval_line.unwrap_or("").to_ascii_lowercase();
    let asks_yes_no = text.contains("y/n")
        || text.contains("yes/no")
        || text.contains("[y]")
        || text.contains("(y/");
    if asks_yes_no {
        b"y\r"
    } else {
        b"\r"
    }
}

pub fn extract_preview_lines(visible_text: &str, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = visible_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| truncate_chars(line, 96))
        .collect();
    if lines.len() > max_lines {
        lines.drain(0..(lines.len() - max_lines));
    }
    lines
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let prefix: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{prefix}…")
}

pub fn extract_approval_line(visible_text: &str) -> Option<String> {
    let line = visible_text
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| is_approval_prompt_line(l))?;
    Some(truncate_chars(line, 110))
}

fn is_approval_prompt_line(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    lower.contains("y/n")
        || lower.contains("yes/no")
        || lower.contains("allow")
        || lower.contains("approve")
        || lower.contains("confirm")
        || lower.contains("подтверд")
}

pub fn collect_swarm_cards_from_sessions(sessions: &[TermSession], now: Instant) -> Vec<SwarmCardSpec> {
    let mut cards = Vec::new();
    for session in sessions {
        collect_pane_cards(session, now, &mut cards);
    }
    cards
}

fn collect_pane_cards(session: &TermSession, now: Instant, out: &mut Vec<SwarmCardSpec>) {
    let mut vtes = Vec::new();
    session.panes.collect_vtes(session.vte, &mut vtes);
    for vte in vtes {
        out.push(build_card_spec(session, vte, now));
    }
}

fn build_card_spec(session: &TermSession, vte: *mut GtkWidget, now: Instant) -> SwarmCardSpec {
    let reg_pane = session.pane_registry.by_vte(vte);
    let cwd = reg_pane.filter(|p| !p.cwd.is_empty()).map(|p| p.cwd.clone()).unwrap_or_else(|| {
        session.panes.live_cwd_for_vte(vte, session.vte, session.child_pid, &session.cwd).unwrap_or_else(|| session.cwd.clone())
    });
    let (repo_name, branch_badge, is_worktree) = resolve_repo_meta(&cwd, &session.title);
    let visible = read_vte_visible_text(vte);
    let state = reg_pane.map_or(session.state, |p| p.state);
    let elapsed_secs = reg_pane
        .and_then(|p| p.live_turn_secs(now).or(p.last_turn_secs))
        .or_else(|| session.live_turn_secs(now).or_else(|| session.settled_turn_secs()));
    let snap = reg_pane.map_or_else(|| session.telemetry.snapshot(), |p| p.telemetry.snapshot());
    let agent = reg_pane.map(crate::pane_registry::PaneRecord::effective_agent).filter(|a| !a.is_empty())
        .unwrap_or_else(|| card_agent_label(session));
    SwarmCardSpec {
        sid: session.id, vte, agent, repo_name, branch_badge, is_worktree, state, elapsed_secs,
        context_badge: snap.compact_context_badge(), cost_usd: snap.has_data().then_some(snap.cost_usd).flatten(),
        preview_lines: extract_preview_lines(&visible, 3),
        approval_line: (state == WorkState::NeedsApproval).then(|| extract_approval_line(&visible)).flatten(),
    }
}

fn card_agent_label(session: &TermSession) -> String {
    let agent = session.effective_agent();
    if agent.is_empty() {
        "shell".to_owned()
    } else {
        agent
    }
}

fn resolve_repo_meta(cwd: &str, fallback_title: &str) -> (String, String, bool) {
    let path = Path::new(cwd);
    if let Some((root, info)) = detect_git_repo(path) {
        let name = root.file_name().and_then(|n| n.to_str()).unwrap_or(fallback_title);
        return (name.to_owned(), info.badge(), info.is_linked_worktree);
    }
    let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or(fallback_title);
    (dir_name.to_owned(), String::new(), false)
}

pub fn select_broadcast_indices(cards: &[SwarmCardSpec], filter: BroadcastFilter) -> Vec<usize> {
    match filter {
        BroadcastFilter::All => (0..cards.len()).collect(),
        BroadcastFilter::IdleOnly => cards
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.state, WorkState::Idle | WorkState::Ready))
            .map(|(idx, _)| idx)
            .collect(),
        BroadcastFilter::Clear => Vec::new(),
    }
}

pub fn build_app_swarm_callbacks() -> SwarmCallbacks {
    SwarmCallbacks {
        on_focus: Rc::new(focus_app_pane),
        on_approve: Rc::new(approve_app_pane),
        on_broadcast: Rc::new(broadcast_app_panes),
        on_close: Rc::new(close_app_swarm_view),
        on_open_dispatch: Rc::new(open_for_app),
        on_open_dashboard: Rc::new(open_app_swarm_view),
    }
}

fn open_app_swarm_view() {
    let snap = with_app(|app| {
        let cards = collect_swarm_cards_from_sessions(&app.sessions, Instant::now());
        (app.stack, app.swarm_view.clone(), cards)
    });
    if let Some((stack, view, cards)) = snap.filter(|(s, _, _)| !s.is_null()) {
        view.update_cards(&cards, crate::i18n::current_locale());
        unsafe { gtk_stack_set_visible_child_name(stack, SWARM_STACK_CSTR.as_ptr()) };
    }
}

fn focus_app_pane(sid: usize, vte: *mut GtkWidget) {
    select_session(sid);
    if !vte.is_null() {
        unsafe { gtk_widget_grab_focus(vte) };
    } else {
        focus_active_vte();
    }
}

fn approve_app_pane(sid: usize, vte: *mut GtkWidget, bytes: &[u8]) {
    feed_pane_bytes(sid, vte, bytes);
    set_footer_message(&format!("Approved #{sid}"));
}

fn broadcast_app_panes(targets: &[(usize, *mut GtkWidget)], prompt: &str) {
    let trimmed = prompt.trim();
    if trimmed.is_empty() || targets.is_empty() {
        return;
    }
    let payload = format!("{trimmed}\r");
    for &(sid, vte) in targets {
        feed_pane_bytes(sid, vte, payload.as_bytes());
    }
    set_footer_message(&format!("Broadcast sent to {} panes", targets.len()));
}

fn feed_pane_bytes(sid: usize, vte: *mut GtkWidget, bytes: &[u8]) {
    let target = if !vte.is_null() {
        vte
    } else {
        with_app(|app| app.sessions.iter().find(|s| s.id == sid).map(|s| s.active_vte()))
            .flatten()
            .unwrap_or(ptr::null_mut())
    };
    if !target.is_null() {
        unsafe { vte_terminal_feed_child(target, bytes.as_ptr().cast::<c_char>(), bytes.len() as isize) };
    }
}

fn close_app_swarm_view() {
    let target = with_app(|app| (app.stack, app.active_id));
    if let Some((stack, active_id)) = target {
        show_terminal_in_stack(stack, Some(active_id));
        focus_active_vte();
    }
}

pub fn show_terminal_in_stack(stack: *mut GtkStack, active_sid: Option<usize>) {
    if stack.is_null() {
        return;
    }
    if let Some(sid) = active_sid {
        let name = format!("term_{sid}");
        if let Ok(c_name) = CString::new(name) {
            unsafe { gtk_stack_set_visible_child_name(stack, c_name.as_ptr()) };
        }
    }
}

pub fn is_swarm_active_in_stack(stack: *mut GtkStack) -> bool {
    if stack.is_null() {
        return false;
    }
    let ptr = unsafe { gtk_stack_get_visible_child_name(stack) };
    if ptr.is_null() {
        return false;
    }
    unsafe { CStr::from_ptr(ptr).to_str() == Ok(SWARM_STACK_NAME) }
}

#[derive(Clone)]
pub struct SwarmCardWidgets {
    pub root: *mut GtkWidget,
    pub check: *mut GtkWidget,
    #[cfg(test)]
    pub approve_btn: Option<*mut GtkWidget>,
    #[cfg(test)]
    pub focus_btn: *mut GtkWidget,
    pub sid: usize,
    pub vte: *mut GtkWidget,
    pub state: WorkState,
}

struct SwarmViewInner {
    root: *mut GtkWidget,
    title_lbl: *mut GtkWidget,
    summary_lbl: *mut GtkWidget,
    cards_box: *mut GtkWidget,
    entry: *mut GtkWidget,
    select_all_btn: *mut GtkWidget,
    select_idle_btn: *mut GtkWidget,
    clear_btn: *mut GtkWidget,
    broadcast_btn: *mut GtkWidget,
    dispatch_btn: *mut GtkWidget,
    close_btn: *mut GtkWidget,
    cards: Vec<SwarmCardWidgets>,
    callbacks: SwarmCallbacks,
}

#[derive(Clone)]
pub struct SwarmView {
    inner: Rc<RefCell<SwarmViewInner>>,
}

struct SwarmPopoverInner {
    popover: *mut GtkWidget,
    list_box: *mut GtkWidget,
    row_widgets: Vec<*mut GtkWidget>,
    approve_btns: Vec<*mut GtkWidget>,
    focus_btns: Vec<*mut GtkWidget>,
    open_board_btn: *mut GtkWidget,
    open_dispatch_btn: *mut GtkWidget,
    callbacks: SwarmCallbacks,
}

#[derive(Clone)]
pub struct SwarmPopover {
    inner: Rc<RefCell<SwarmPopoverInner>>,
}

impl SwarmView {
    pub fn new(callbacks: SwarmCallbacks, locale: UiLocale) -> Self {
        let root = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 10) };
        add_class(root, "main-bg");
        add_class(root, "swarm-dashboard");
        set_widget_margins(root, 16);
        let (title_lbl, summary_lbl, dispatch_btn, close_btn) = build_header_row(root, locale);
        let cards_box = build_cards_scroller(root);
        let (select_all_btn, select_idle_btn, clear_btn, entry, broadcast_btn) =
            build_broadcast_bar(root, locale);
        let inner = SwarmViewInner {
            root, title_lbl, summary_lbl, cards_box, entry,
            select_all_btn, select_idle_btn, clear_btn, broadcast_btn,
            dispatch_btn, close_btn, cards: Vec::new(), callbacks,
        };
        let view = Self { inner: Rc::new(RefCell::new(inner)) };
        view.wire_static_signals();
        view
    }

    fn wire_static_signals(&self) {
        let inner = self.inner.borrow();
        let v1 = self.clone();
        connect_clicked(inner.select_all_btn, move || v1.apply_filter(BroadcastFilter::All));
        let v2 = self.clone();
        connect_clicked(inner.select_idle_btn, move || v2.apply_filter(BroadcastFilter::IdleOnly));
        let v3 = self.clone();
        connect_clicked(inner.clear_btn, move || v3.apply_filter(BroadcastFilter::Clear));
        let v4 = self.clone();
        connect_clicked(inner.broadcast_btn, move || v4.trigger_broadcast());
        let v5 = self.clone();
        connect_activate(inner.entry, move || v5.trigger_broadcast());
        wire_header_buttons(&inner);
    }

    pub fn root(&self) -> *mut GtkWidget {
        self.inner.borrow().root
    }

    #[cfg(test)]
    pub fn entry_widget(&self) -> *mut GtkWidget {
        self.inner.borrow().entry
    }

    pub fn mount_in_stack(&self, stack: *mut GtkStack) {
        if stack.is_null() {
            return;
        }
        let root = self.root();
        unsafe {
            gtk_stack_add_named(stack, root, SWARM_STACK_CSTR.as_ptr());
            gtk_widget_show_all(root);
        }
    }

    pub fn toggle_in_stack(&self, stack: *mut GtkStack, active_sid: Option<usize>) -> bool {
        if stack.is_null() {
            return false;
        }
        if is_swarm_active_in_stack(stack) {
            show_terminal_in_stack(stack, active_sid);
            return false;
        }
        unsafe { gtk_stack_set_visible_child_name(stack, SWARM_STACK_CSTR.as_ptr()) };
        unsafe { gtk_widget_grab_focus(self.inner.borrow().entry) };
        true
    }

    pub fn handle_key_press(&self, stack: *mut GtkStack, active_sid: Option<usize>, keyval: u32) -> bool {
        if keyval != 0xff1b || !is_swarm_active_in_stack(stack) {
            return false;
        }
        show_terminal_in_stack(stack, active_sid);
        (self.inner.borrow().callbacks.on_close)();
        true
    }

    pub fn update_cards(&self, specs: &[SwarmCardSpec], locale: UiLocale) {
        let mut inner = self.inner.borrow_mut();
        clear_swarm_cards(&mut inner);
        update_header_labels(&inner, specs, locale);
        if specs.is_empty() {
            append_empty_label(inner.cards_box, locale, &mut inner.cards);
            return;
        }
        for spec in specs {
            let row = build_card_row(spec, &inner.callbacks, locale);
            unsafe { gtk_box_pack_start(inner.cards_box.cast(), row.root, 0, 0, 0) };
            inner.cards.push(row);
        }
        unsafe { gtk_widget_show_all(inner.cards_box) };
    }

    pub fn apply_filter(&self, filter: BroadcastFilter) {
        let inner = self.inner.borrow();
        let specs: Vec<SwarmCardSpec> = inner.cards.iter().map(card_stub_for_filter).collect();
        let selected = select_broadcast_indices(&specs, filter);
        for (idx, card) in inner.cards.iter().enumerate() {
            if !card.check.is_null() {
                let on = selected.contains(&idx);
                unsafe { gtk_toggle_button_set_active(card.check.cast(), c_int::from(on)) };
            }
        }
    }

    pub fn selected_targets(&self) -> Vec<(usize, *mut GtkWidget)> {
        let inner = self.inner.borrow();
        inner
            .cards
            .iter()
            .filter(|c| is_card_checked(c))
            .map(|c| (c.sid, c.vte))
            .collect()
    }

    pub fn trigger_broadcast(&self) {
        let targets = self.selected_targets();
        let (prompt, cb, entry) = {
            let inner = self.inner.borrow();
            (read_entry_text(inner.entry), inner.callbacks.on_broadcast.clone(), inner.entry)
        };
        if !prompt.trim().is_empty() && !targets.is_empty() {
            cb(&targets, &prompt);
            unsafe { gtk_entry_set_text(entry, c"".as_ptr()) };
        }
    }

    #[cfg(test)]
    pub fn card_count(&self) -> usize {
        self.inner.borrow().cards.iter().filter(|c| !c.check.is_null()).count()
    }

    #[cfg(test)]
    pub fn card_approve_button(&self, idx: usize) -> Option<*mut GtkWidget> {
        self.inner.borrow().cards.get(idx).and_then(|c| c.approve_btn)
    }

    #[cfg(test)]
    pub fn card_focus_button(&self, idx: usize) -> Option<*mut GtkWidget> {
        self.inner.borrow().cards.get(idx).map(|c| c.focus_btn)
    }

    #[cfg(test)]
    pub fn preset_buttons(&self) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
        let inner = self.inner.borrow();
        (inner.select_all_btn, inner.select_idle_btn, inner.clear_btn, inner.broadcast_btn)
    }
}

impl SwarmPopover {
    pub fn new(relative_to: *mut GtkWidget, callbacks: SwarmCallbacks, locale: UiLocale) -> Self {
        let popover = unsafe { gtk_popover_new(relative_to) };
        add_class(popover, "popover-box");
        let body = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 6) };
        set_widget_margins(body, 10);
        unsafe { gtk_container_add(popover.cast(), body) };
        let list_box = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 4) };
        unsafe { gtk_box_pack_start(body.cast(), list_box, 0, 0, 0) };
        let (open_board_btn, open_dispatch_btn) = build_popover_footer(body, popover, &callbacks, locale);
        let inner = SwarmPopoverInner {
            popover, list_box, row_widgets: Vec::new(), approve_btns: Vec::new(),
            focus_btns: Vec::new(), open_board_btn, open_dispatch_btn, callbacks,
        };
        Self { inner: Rc::new(RefCell::new(inner)) }
    }

    pub fn popup(&self) {
        unsafe {
            gtk_widget_show_all(self.inner.borrow().popover);
            gtk_popover_popup(self.inner.borrow().popover.cast());
        }
    }

    pub fn update_cards(&self, specs: &[SwarmCardSpec], locale: UiLocale) {
        let mut inner = self.inner.borrow_mut();
        clear_popover_rows(&mut inner);
        unsafe {
            gtk_button_set_label(inner.open_board_btn.cast(), BTN_OPEN_DASHBOARD.for_locale(locale).as_ptr());
            gtk_button_set_label(inner.open_dispatch_btn.cast(), BTN_NEW_TASK.for_locale(locale).as_ptr());
        }
        if specs.is_empty() {
            let lbl = make_label(EMPTY_SWARM_TEXT.for_locale(locale), 0.0);
            unsafe { gtk_box_pack_start(inner.list_box.cast(), lbl, 0, 0, 0) };
            inner.row_widgets.push(lbl);
            return;
        }
        for spec in specs.iter().take(6) {
            append_popover_row(&mut inner, spec, locale);
        }
        unsafe { gtk_widget_show_all(inner.list_box) };
    }

    #[cfg(test)]
    pub fn row_count(&self) -> usize {
        self.inner.borrow().row_widgets.len()
    }

    #[cfg(test)]
    pub fn approve_buttons(&self) -> Vec<*mut GtkWidget> {
        self.inner.borrow().approve_btns.clone()
    }

    pub fn sync_topbar_button(button: *mut GtkWidget, specs: &[SwarmCardSpec], locale: UiLocale) {
        if button.is_null() {
            return;
        }
        let approvals = specs.iter().filter(|s| s.state == WorkState::NeedsApproval).count();
        let label = format_swarm_badge(specs.len(), approvals, locale);
        if let Ok(c_label) = CString::new(label) {
            unsafe { gtk_button_set_label(button.cast(), c_label.as_ptr()) };
        }
    }
}

fn wire_header_buttons(inner: &SwarmViewInner) {
    let cb_dispatch = inner.callbacks.on_open_dispatch.clone();
    connect_clicked(inner.dispatch_btn, move || cb_dispatch());
    let cb_close = inner.callbacks.on_close.clone();
    connect_clicked(inner.close_btn, move || cb_close());
}

fn build_header_row(
    root: *mut GtkWidget,
    locale: UiLocale,
) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let title = make_label(TITLE_SWARM.for_locale(locale), 0.0);
    add_class(title, "section-header");
    let summary = make_label(c"", 0.0);
    let dispatch_btn = make_button(BTN_NEW_TASK.for_locale(locale));
    let close_btn = make_button(BTN_CLOSE_ESC.for_locale(locale));
    unsafe {
        gtk_box_pack_start(row.cast(), title, 0, 0, 0);
        gtk_box_pack_start(row.cast(), summary, 1, 1, 8);
        gtk_box_pack_start(row.cast(), dispatch_btn, 0, 0, 0);
        gtk_box_pack_start(row.cast(), close_btn, 0, 0, 0);
        gtk_box_pack_start(root.cast(), row, 0, 0, 0);
    }
    (title, summary, dispatch_btn, close_btn)
}

fn build_cards_scroller(root: *mut GtkWidget) -> *mut GtkWidget {
    let sw = unsafe { gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut()) };
    let cards_box = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 8) };
    unsafe {
        gtk_scrolled_window_set_policy(sw, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_widget_set_vexpand(sw, 1);
        gtk_container_add(sw.cast(), cards_box);
        gtk_box_pack_start(root.cast(), sw, 1, 1, 0);
    }
    cards_box
}

fn build_broadcast_bar(
    root: *mut GtkWidget,
    locale: UiLocale,
) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6) };
    let all_btn = make_button(BTN_SELECT_ALL.for_locale(locale));
    let idle_btn = make_button(BTN_SELECT_IDLE.for_locale(locale));
    let clear_btn = make_button(BTN_SELECT_CLEAR.for_locale(locale));
    let entry = unsafe { gtk_entry_new() };
    add_class(entry, "search-entry");
    unsafe { gtk_entry_set_placeholder_text(entry, PLACEHOLDER_BROADCAST.for_locale(locale).as_ptr()) };
    let send_btn = make_button(BTN_BROADCAST.for_locale(locale));
    pack_broadcast_widgets(row, all_btn, idle_btn, clear_btn, entry, send_btn);
    unsafe { gtk_box_pack_start(root.cast(), row, 0, 0, 0) };
    (all_btn, idle_btn, clear_btn, entry, send_btn)
}

fn pack_broadcast_widgets(
    row: *mut GtkWidget,
    all_btn: *mut GtkWidget,
    idle_btn: *mut GtkWidget,
    clear_btn: *mut GtkWidget,
    entry: *mut GtkWidget,
    send_btn: *mut GtkWidget,
) {
    unsafe {
        gtk_box_pack_start(row.cast(), all_btn, 0, 0, 0);
        gtk_box_pack_start(row.cast(), idle_btn, 0, 0, 0);
        gtk_box_pack_start(row.cast(), clear_btn, 0, 0, 0);
        gtk_box_pack_start(row.cast(), entry, 1, 1, 4);
        gtk_box_pack_start(row.cast(), send_btn, 0, 0, 0);
    }
}

fn clear_swarm_cards(inner: &mut SwarmViewInner) {
    for card in inner.cards.drain(..) {
        if !card.root.is_null() {
            unsafe { gtk_widget_destroy(card.root) };
        }
    }
}

fn update_header_labels(inner: &SwarmViewInner, specs: &[SwarmCardSpec], locale: UiLocale) {
    set_label_text(inner.title_lbl, TITLE_SWARM.for_locale(locale).to_str().unwrap_or(""));
    let approvals = specs.iter().filter(|s| s.state == WorkState::NeedsApproval).count();
    let text = format_swarm_badge(specs.len(), approvals, locale);
    set_label_text(inner.summary_lbl, &text);
}

fn append_empty_label(cards_box: *mut GtkWidget, locale: UiLocale, cards: &mut Vec<SwarmCardWidgets>) {
    let lbl = make_label(EMPTY_SWARM_TEXT.for_locale(locale), 0.0);
    unsafe {
        gtk_box_pack_start(cards_box.cast(), lbl, 0, 0, 8);
        gtk_widget_show_all(cards_box);
    }
    #[cfg(test)]
    cards.push(SwarmCardWidgets {
        root: lbl, check: ptr::null_mut(), approve_btn: None,
        focus_btn: ptr::null_mut(), sid: 0, vte: ptr::null_mut(), state: WorkState::Idle,
    });
    #[cfg(not(test))]
    cards.push(SwarmCardWidgets {
        root: lbl, check: ptr::null_mut(), sid: 0, vte: ptr::null_mut(), state: WorkState::Idle,
    });
}

fn build_card_row(spec: &SwarmCardSpec, cb: &SwarmCallbacks, locale: UiLocale) -> SwarmCardWidgets {
    let card = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 4) };
    add_class(card, "popover-box");
    add_class(card, "swarm-card");
    set_widget_margins(card, 10);
    let (check, focus_btn) = build_card_top_bar(card, spec, cb, locale);
    append_card_meta_line(card, spec, locale);
    append_card_preview(card, spec);
    let approve_btn = maybe_append_approval_bar(card, spec, cb, locale);
    #[cfg(test)]
    return SwarmCardWidgets {
        root: card, check, approve_btn, focus_btn,
        sid: spec.sid, vte: spec.vte, state: spec.state,
    };
    #[cfg(not(test))]
    {
        let _ = (approve_btn, focus_btn);
        SwarmCardWidgets { root: card, check, sid: spec.sid, vte: spec.vte, state: spec.state }
    }
}

fn build_card_top_bar(
    card: *mut GtkWidget,
    spec: &SwarmCardSpec,
    cb: &SwarmCallbacks,
    locale: UiLocale,
) -> (*mut GtkWidget, *mut GtkWidget) {
    let top = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6) };
    let check = unsafe { gtk_check_button_new_with_label(c"".as_ptr()) };
    let title = format_card_title(spec);
    let title_lbl = make_text_label(&title, 0.0);
    add_class(title_lbl, "section-header");
    let focus_btn = make_button(BTN_FOCUS.for_locale(locale));
    wire_focus_click(focus_btn, spec.sid, spec.vte, cb.on_focus.clone());
    unsafe {
        gtk_box_pack_start(top.cast(), check, 0, 0, 0);
        gtk_box_pack_start(top.cast(), title_lbl, 1, 1, 0);
        gtk_box_pack_start(top.cast(), focus_btn, 0, 0, 0);
        gtk_box_pack_start(card.cast(), top, 0, 0, 0);
    }
    (check, focus_btn)
}

fn format_card_title(spec: &SwarmCardSpec) -> String {
    let wt_tag = if spec.is_worktree { " [wt]" } else { "" };
    if spec.branch_badge.is_empty() {
        return format!("{} · {}{}", spec.agent, spec.repo_name, wt_tag);
    }
    format!("{} · {} ({}){}", spec.agent, spec.repo_name, spec.branch_badge, wt_tag)
}

fn append_card_meta_line(card: *mut GtkWidget, spec: &SwarmCardSpec, locale: UiLocale) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let state_lbl = make_label(state_text(spec.state).for_locale(locale), 0.0);
    add_class(state_lbl, "palette-badge");
    let meta = format_card_metrics(spec);
    let meta_lbl = make_text_label(&meta, 0.0);
    unsafe {
        gtk_box_pack_start(row.cast(), state_lbl, 0, 0, 0);
        gtk_box_pack_start(row.cast(), meta_lbl, 1, 1, 0);
        gtk_box_pack_start(card.cast(), row, 0, 0, 0);
    }
}

fn format_card_metrics(spec: &SwarmCardSpec) -> String {
    let mut parts = Vec::new();
    if let Some(secs) = spec.elapsed_secs {
        parts.push(format!("⏱ {secs}s"));
    }
    if let Some(ctx) = &spec.context_badge {
        parts.push(format!("ctx {ctx}"));
    }
    if let Some(cost) = spec.cost_usd {
        parts.push(format!("${cost:.2}"));
    }
    parts.join(" · ")
}

fn append_card_preview(card: *mut GtkWidget, spec: &SwarmCardSpec) {
    if spec.preview_lines.is_empty() {
        return;
    }
    let joined = spec.preview_lines.join("\n");
    let lbl = make_text_label(&joined, 0.0);
    unsafe {
        gtk_label_set_line_wrap(lbl.cast(), 1);
        gtk_label_set_ellipsize(lbl.cast(), PANGO_ELLIPSIZE_END);
        gtk_label_set_max_width_chars(lbl.cast(), 96);
        gtk_box_pack_start(card.cast(), lbl, 0, 0, 0);
    }
}

fn maybe_append_approval_bar(
    card: *mut GtkWidget,
    spec: &SwarmCardSpec,
    cb: &SwarmCallbacks,
    locale: UiLocale,
) -> Option<*mut GtkWidget> {
    if spec.state != WorkState::NeedsApproval {
        return None;
    }
    let bar = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let prompt_text = spec.approval_line.as_deref().unwrap_or("Approval requested");
    let prompt_lbl = make_text_label(prompt_text, 0.0);
    add_class(prompt_lbl, "term-search-status-err");
    let btn = build_quick_approve_btn(spec, cb, locale);
    unsafe {
        gtk_box_pack_start(bar.cast(), prompt_lbl, 1, 1, 0);
        gtk_box_pack_start(bar.cast(), btn, 0, 0, 0);
        gtk_box_pack_start(card.cast(), bar, 0, 0, 0);
    }
    Some(btn)
}

fn build_quick_approve_btn(
    spec: &SwarmCardSpec,
    cb: &SwarmCallbacks,
    locale: UiLocale,
) -> *mut GtkWidget {
    let bytes = approval_response_bytes(spec.approval_line.as_deref());
    let label = if bytes == b"y\r" {
        BTN_APPROVE_YN.for_locale(locale)
    } else {
        BTN_APPROVE_CR.for_locale(locale)
    };
    let btn = make_button(label);
    let sid = spec.sid;
    let vte = spec.vte;
    let on_approve = cb.on_approve.clone();
    connect_clicked(btn, move || on_approve(sid, vte, bytes));
    btn
}

fn build_popover_footer(
    body: *mut GtkWidget,
    popover: *mut GtkWidget,
    callbacks: &SwarmCallbacks,
    locale: UiLocale,
) -> (*mut GtkWidget, *mut GtkWidget) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6) };
    let board_btn = make_button(BTN_OPEN_DASHBOARD.for_locale(locale));
    let cb_b = callbacks.on_open_dashboard.clone();
    connect_clicked(board_btn, move || {
        unsafe { gtk_popover_popdown(popover.cast()) };
        cb_b();
    });
    let dispatch_btn = make_button(BTN_NEW_TASK.for_locale(locale));
    let cb_d = callbacks.on_open_dispatch.clone();
    connect_clicked(dispatch_btn, move || {
        unsafe { gtk_popover_popdown(popover.cast()) };
        cb_d();
    });
    unsafe {
        gtk_box_pack_start(row.cast(), board_btn, 0, 0, 0);
        gtk_box_pack_start(row.cast(), dispatch_btn, 0, 0, 0);
        gtk_box_pack_start(body.cast(), row, 0, 0, 0);
    }
    (board_btn, dispatch_btn)
}

fn clear_popover_rows(inner: &mut SwarmPopoverInner) {
    for row in inner.row_widgets.drain(..) {
        if !row.is_null() {
            unsafe { gtk_widget_destroy(row) };
        }
    }
    inner.approve_btns.clear();
    inner.focus_btns.clear();
}

fn append_popover_row(inner: &mut SwarmPopoverInner, spec: &SwarmCardSpec, locale: UiLocale) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6) };
    let title = format_card_title(spec);
    let lbl = make_text_label(&title, 0.0);
    let focus_btn = make_button(BTN_FOCUS.for_locale(locale));
    wire_popover_focus(focus_btn, inner.popover, spec.sid, spec.vte, inner.callbacks.on_focus.clone());
    unsafe {
        gtk_box_pack_start(row.cast(), lbl, 1, 1, 0);
        maybe_pack_popover_approve(inner, row, spec, locale);
        gtk_box_pack_start(row.cast(), focus_btn, 0, 0, 0);
        gtk_box_pack_start(inner.list_box.cast(), row, 0, 0, 0);
    }
    inner.focus_btns.push(focus_btn);
    inner.row_widgets.push(row);
}

fn maybe_pack_popover_approve(
    inner: &mut SwarmPopoverInner,
    row: *mut GtkWidget,
    spec: &SwarmCardSpec,
    locale: UiLocale,
) {
    if spec.state == WorkState::NeedsApproval {
        let btn = build_quick_approve_btn(spec, &inner.callbacks, locale);
        unsafe { gtk_box_pack_start(row.cast(), btn, 0, 0, 0) };
        inner.approve_btns.push(btn);
    }
}

fn wire_popover_focus(
    btn: *mut GtkWidget,
    popover: *mut GtkWidget,
    sid: usize,
    vte: *mut GtkWidget,
    cb: Rc<dyn Fn(usize, *mut GtkWidget)>,
) {
    connect_clicked(btn, move || {
        unsafe { gtk_popover_popdown(popover.cast()) };
        cb(sid, vte);
    });
}

fn wire_focus_click(
    btn: *mut GtkWidget,
    sid: usize,
    vte: *mut GtkWidget,
    cb: Rc<dyn Fn(usize, *mut GtkWidget)>,
) {
    connect_clicked(btn, move || cb(sid, vte));
}

fn card_stub_for_filter(card: &SwarmCardWidgets) -> SwarmCardSpec {
    SwarmCardSpec {
        sid: card.sid, vte: card.vte, agent: String::new(), repo_name: String::new(),
        branch_badge: String::new(), is_worktree: false, state: card.state,
        elapsed_secs: None, context_badge: None, cost_usd: None,
        preview_lines: Vec::new(), approval_line: None,
    }
}

fn is_card_checked(card: &SwarmCardWidgets) -> bool {
    !card.check.is_null() && unsafe { gtk_toggle_button_get_active(card.check.cast()) != 0 }
}

fn read_entry_text(entry: *mut GtkWidget) -> String {
    if entry.is_null() {
        return String::new();
    }
    let ptr = unsafe { gtk_entry_get_text(entry) };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
}

fn state_text(state: WorkState) -> UiText {
    match state {
        WorkState::Idle => STATE_IDLE,
        WorkState::Working => STATE_WORKING,
        WorkState::Ready => STATE_READY,
        WorkState::NeedsApproval => STATE_APPROVAL,
    }
}

fn set_widget_margins(widget: *mut GtkWidget, px: c_int) {
    unsafe {
        gtk_widget_set_margin_start(widget, px);
        gtk_widget_set_margin_end(widget, px);
        gtk_widget_set_margin_top(widget, px);
        gtk_widget_set_margin_bottom(widget, px);
    }
}

fn make_label(text: &CStr, xalign: f32) -> *mut GtkWidget {
    let lbl = unsafe { gtk_label_new(text.as_ptr()) };
    unsafe { gtk_label_set_xalign(lbl.cast(), xalign) };
    lbl
}

fn make_text_label(text: &str, xalign: f32) -> *mut GtkWidget {
    let c_str = CString::new(text).unwrap_or_default();
    make_label(&c_str, xalign)
}

fn make_button(label: &CStr) -> *mut GtkWidget {
    unsafe { gtk_button_new_with_label(label.as_ptr()) }
}

fn connect_clicked<F: Fn() + 'static>(widget: *mut GtkWidget, cb: F) {
    connect_zero_arg_signal(widget, c"clicked", cb);
}

fn connect_activate<F: Fn() + 'static>(widget: *mut GtkWidget, cb: F) {
    connect_zero_arg_signal(widget, c"activate", cb);
}

fn connect_zero_arg_signal<F: Fn() + 'static>(widget: *mut GtkWidget, signal: &CStr, cb: F) {
    if widget.is_null() {
        return;
    }
    let ptr = Box::into_raw(Box::new(cb)).cast::<c_void>();
    unsafe {
        g_signal_connect_data(
            widget.cast(),
            signal.as_ptr(),
            on_zero_arg_signal::<F> as *const c_void,
            ptr,
            drop_boxed::<F> as *const c_void,
            0,
        );
    }
}

extern "C" fn on_zero_arg_signal<F: Fn()>(_widget: *mut GtkWidget, data: *mut c_void) {
    let cb = unsafe { &*data.cast::<F>() };
    cb();
}

extern "C" fn drop_boxed<F>(data: *mut c_void, _closure: *mut c_void) {
    drop(unsafe { Box::from_raw(data.cast::<F>()) });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::process::{Child, Command, Stdio};
    use std::rc::Rc;

    const CHILD_ENV: &str = "AKHTERM_SWARM_GTK_CHILD";

    fn sample_cards() -> Vec<SwarmCardSpec> {
        vec![
            sample_card(1, "codex", WorkState::NeedsApproval, Some("Allow git push? [y/n]")),
            sample_card(2, "claude", WorkState::Idle, None),
            sample_card(3, "agy", WorkState::Working, None),
        ]
    }

    fn sample_card(sid: usize, agent: &str, state: WorkState, approval: Option<&str>) -> SwarmCardSpec {
        SwarmCardSpec {
            sid,
            vte: (sid * 100) as *mut GtkWidget,
            agent: agent.to_owned(),
            repo_name: "akhterm-rs".to_owned(),
            branch_badge: format!("wt/{agent}-fix"),
            is_worktree: true,
            state,
            elapsed_secs: Some(42),
            context_badge: Some("38% (12k)".to_owned()),
            cost_usd: Some(0.14),
            preview_lines: vec!["Running tests".to_owned(), "All green".to_owned()],
            approval_line: approval.map(str::to_owned),
        }
    }

    #[test]
    fn shortcut_matches_ctrl_shift_s_latin_and_cyrillic_only() {
        let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
        assert!(is_swarm_shortcut(0x053, cs, 39));
        assert!(is_swarm_shortcut(0x6d9, cs, 39));
        assert!(is_swarm_shortcut(0x6f9, cs, 0));
        assert!(!is_swarm_shortcut(0x053, GDK_CONTROL_MASK, 39));
        assert!(!is_swarm_shortcut(0x053, cs | GDK_MOD1_MASK, 39));
        assert!(!is_swarm_shortcut(0x04e, cs, 57));
    }

    #[test]
    fn badge_formats_ru_and_en_with_approval_count() {
        assert_eq!(format_swarm_badge(0, 0, UiLocale::Ru), "Рой ▾");
        assert_eq!(format_swarm_badge(0, 0, UiLocale::En), "Swarm ▾");
        assert_eq!(format_swarm_badge(3, 0, UiLocale::Ru), "Рой · 3");
        assert_eq!(format_swarm_badge(3, 1, UiLocale::En), "Swarm · 3 (!1)");
    }

    #[test]
    fn approval_response_bytes_detects_yes_no_vs_enter_menu() {
        assert_eq!(approval_response_bytes(Some("Continue? [y/n]")), b"y\r");
        assert_eq!(approval_response_bytes(Some("Confirm (yes/no)?")), b"y\r");
        assert_eq!(approval_response_bytes(Some("Press Enter to approve tool")), b"\r");
        assert_eq!(approval_response_bytes(None), b"\r");
    }

    #[test]
    fn preview_and_approval_line_extraction_from_vte_output() {
        let screen = "\n  line one\n\nline two\nAllow rm -rf /tmp/x? [y/n]\nline four\n";
        let preview = extract_preview_lines(screen, 3);
        assert_eq!(preview, vec!["line two", "Allow rm -rf /tmp/x? [y/n]", "line four"]);
        assert_eq!(extract_approval_line(screen).as_deref(), Some("Allow rm -rf /tmp/x? [y/n]"));
    }

    #[test]
    fn broadcast_filter_selects_all_idle_only_and_clear() {
        let cards = sample_cards();
        assert_eq!(select_broadcast_indices(&cards, BroadcastFilter::All), vec![0, 1, 2]);
        assert_eq!(select_broadcast_indices(&cards, BroadcastFilter::IdleOnly), vec![1]);
        assert!(select_broadcast_indices(&cards, BroadcastFilter::Clear).is_empty());
    }

    #[test]
    fn headless_broadway_swarm_dashboard_cards_approve_and_broadcast() {
        if std::env::var_os(CHILD_ENV).is_some() {
            run_broadway_swarm_child();
            return;
        }
        let exe = std::env::current_exe().unwrap();
        let out = Command::new(exe)
            .env(CHILD_ENV, "1")
            .args(["--exact", "swarm_view::tests::headless_broadway_swarm_dashboard_cards_approve_and_broadcast", "--nocapture"])
            .output()
            .expect("spawn child test");
        assert!(out.status.success(), "child failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[derive(Default)]
    struct RecordedCalls {
        focused: Vec<usize>,
        approved: Vec<(usize, Vec<u8>)>,
        broadcasts: Vec<(Vec<usize>, String)>,
        closed: usize,
    }

    fn run_broadway_swarm_child() {
        let mut broadway = spawn_broadway(":19");
        init_gtk_for_broadway();
        let calls = Rc::new(RefCell::new(RecordedCalls::default()));
        let cb = build_recording_callbacks(&calls);
        verify_dashboard_interactions(&cb, &calls);
        verify_popover_and_button(&cb);
        let _ = broadway.kill();
        let _ = broadway.wait();
    }

    fn spawn_broadway(display: &str) -> Child {
        unsafe { std::env::set_var("GDK_BACKEND", "broadway") };
        unsafe { std::env::set_var("BROADWAY_DISPLAY", display) };
        let child = Command::new("broadwayd")
            .arg(display)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("broadwayd must be installed");
        std::thread::sleep(std::time::Duration::from_millis(200));
        child
    }

    fn init_gtk_for_broadway() {
        let ok = unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) };
        assert_ne!(ok, 0, "gtk_init_check on Broadway :19 failed");
    }

    fn build_recording_callbacks(calls: &Rc<RefCell<RecordedCalls>>) -> SwarmCallbacks {
        let c1 = calls.clone();
        let c2 = calls.clone();
        let c3 = calls.clone();
        let c4 = calls.clone();
        SwarmCallbacks {
            on_focus: Rc::new(move |sid, _| c1.borrow_mut().focused.push(sid)),
            on_approve: Rc::new(move |sid, _, b| c2.borrow_mut().approved.push((sid, b.to_vec()))),
            on_broadcast: Rc::new(move |t, p| {
                let sids = t.iter().map(|(s, _)| *s).collect();
                c3.borrow_mut().broadcasts.push((sids, p.to_owned()));
            }),
            on_close: Rc::new(move || c4.borrow_mut().closed += 1),
            ..Default::default()
        }
    }

    fn verify_dashboard_interactions(cb: &SwarmCallbacks, calls: &Rc<RefCell<RecordedCalls>>) {
        let view = SwarmView::new(cb.clone(), UiLocale::Ru);
        let stack = mount_test_stack(&view);
        assert!(view.toggle_in_stack(stack, Some(1)));
        assert!(is_swarm_active_in_stack(stack));
        view.update_cards(&sample_cards(), UiLocale::Ru);
        assert_eq!(view.card_count(), 3);
        click_widget(view.card_approve_button(0).expect("approve btn"));
        click_widget(view.card_focus_button(1).expect("focus btn"));
        trigger_idle_broadcast(&view);
        assert!(view.handle_key_press(stack, Some(1), 0xff1b));
        assert!(!is_swarm_active_in_stack(stack));
        assert_broadcast_and_actions(calls);
    }

    fn mount_test_stack(view: &SwarmView) -> *mut GtkStack {
        let win = unsafe { gtk_window_new(0) };
        let stack = unsafe { gtk_stack_new() };
        let term = unsafe { gtk_label_new(c"term".as_ptr()) };
        unsafe {
            gtk_container_add(win.cast(), stack);
            gtk_stack_add_named(stack.cast(), term, c"term_1".as_ptr());
        }
        view.mount_in_stack(stack.cast());
        unsafe {
            gtk_widget_show_all(win);
            gtk_stack_set_visible_child_name(stack.cast(), c"term_1".as_ptr());
        }
        stack.cast()
    }

    fn trigger_idle_broadcast(view: &SwarmView) {
        let (_, idle_btn, _, send_btn) = view.preset_buttons();
        click_widget(idle_btn);
        unsafe { gtk_entry_set_text(view.entry_widget(), c"status update".as_ptr()) };
        click_widget(send_btn);
    }

    fn assert_broadcast_and_actions(calls: &Rc<RefCell<RecordedCalls>>) {
        let r = calls.borrow();
        assert_eq!(r.approved, vec![(1, b"y\r".to_vec())]);
        assert_eq!(r.focused, vec![2]);
        assert_eq!(r.broadcasts, vec![(vec![2], "status update".to_owned())]);
        assert_eq!(r.closed, 1);
    }

    fn verify_popover_and_button(cb: &SwarmCallbacks) {
        let btn = unsafe { gtk_button_new_with_label(c"Рой".as_ptr()) };
        let cards = sample_cards();
        SwarmPopover::sync_topbar_button(btn, &cards, UiLocale::Ru);
        let pop = SwarmPopover::new(btn, cb.clone(), UiLocale::Ru);
        pop.update_cards(&cards, UiLocale::Ru);
        assert_eq!(pop.row_count(), 3);
        assert_eq!(pop.approve_buttons().len(), 1);
        let lbl = unsafe { gtk_button_get_label(btn.cast()) };
        assert_eq!(unsafe { CStr::from_ptr(lbl).to_str() }, Ok("Рой · 3 (!1)"));
    }

    fn click_widget(btn: *mut GtkWidget) {
        unsafe { gtk_button_clicked(btn.cast()) };
        while unsafe { gtk_events_pending() } != 0 {
            unsafe { gtk_main_iteration_do(0) };
        }
    }
}
