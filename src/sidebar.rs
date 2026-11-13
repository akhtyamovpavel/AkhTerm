//! Sidebar navigation for open tabs and discoverable tmux sessions.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void, CStr};
use std::path::Path;
use std::ptr;
use std::rc::{Rc, Weak};
use std::time::Instant;

use crate::ffi::*;
use crate::git_wt::{detect_git_head, GitHeadInfo};
use crate::mi_watch::resolve_project_root;
use crate::paths::{home_dir, shorten_home};
use crate::platform::resolve_category_icon;
use crate::session::{SessionCategory, TermSession, WorkState};
use crate::sidebar_folders::{group_local_entries, resolve_project_folder};
use crate::tab_order::BoxReorder;
use crate::telemetry::{
    format_turn_duration, format_working_sidebar_status, ContextAlert,
};
use crate::theme::set_agent_accent;
use crate::ui_text as text;
use crate::{connect_click, connect_entry_changed};

pub type WidgetHandler = extern "C" fn(*mut GtkWidget, *mut c_void);
pub type EventHandler = extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int;
pub type FolderNewTabHandler = fn(&str);

fn noop_folder_action(_path: &str) {}

extern "C" fn noop_row_event(_w: *mut GtkWidget, _e: *const GdkEvent, _d: *mut c_void) -> c_int {
    0
}

#[derive(Clone, Copy)]
pub struct SidebarRowHandlers {
    pub on_select: WidgetHandler,
    pub on_press: EventHandler,
    pub on_motion: EventHandler,
    pub on_release: EventHandler,
}

#[derive(Clone)]
struct FolderSection {
    root: *mut GtkWidget,
    toggle_btn: *mut GtkWidget,
    add_btn: *mut GtkWidget,
    chevron: *mut GtkWidget,
    name_label: *mut GtkWidget,
    count_label: *mut GtkWidget,
    rows_box: *mut GtkWidget,
}

impl FolderSection {
    fn set_collapsed(&self, collapsed: bool) {
        let chevron = if collapsed { c"▸" } else { c"▾" };
        unsafe {
            gtk_label_set_text(self.chevron, chevron.as_ptr());
            gtk_widget_set_visible(self.rows_box, i32::from(!collapsed));
        }
    }

    fn set_header(&self, plan: &FolderPlan, count: usize) {
        set_label_text(self.name_label, &plan.label);
        set_label_text(self.count_label, &count.to_string());
        set_tooltip(self.toggle_btn, &plan.tooltip);
        unsafe {
            gtk_widget_set_tooltip_text(self.add_btn, text::sidebar_folder_new_tab_tip().as_ptr());
            gtk_widget_set_visible(self.root, i32::from(plan.visible));
        }
        self.set_collapsed(plan.collapsed);
    }
}

struct FolderUiState {
    local_list: *mut GtkWidget,
    on_new_tab: FolderNewTabHandler,
    collapsed: HashSet<String>,
    sections: HashMap<String, FolderSection>,
}

impl FolderUiState {
    fn new(local_list: *mut GtkWidget) -> Self {
        Self { local_list, on_new_tab: noop_folder_action, collapsed: HashSet::new(), sections: HashMap::new() }
    }

    fn snapshot_collapsed(&self) -> HashSet<String> {
        self.collapsed.clone()
    }
}

struct FolderPlan {
    key: String,
    label: String,
    tooltip: String,
    total_count: usize,
    visible_count: usize,
    row_widgets: Vec<*mut GtkWidget>,
    visible: bool,
    collapsed: bool,
}

impl FolderPlan {
    fn display_count(&self, query_empty: bool) -> usize {
        if query_empty { self.total_count } else { self.visible_count }
    }
}

struct FolderCallbackData {
    state: Weak<RefCell<FolderUiState>>,
    key: String,
}

impl FolderCallbackData {
    fn boxed(state: &Rc<RefCell<FolderUiState>>, key: &str) -> *mut Self {
        Box::into_raw(Box::new(Self { state: Rc::downgrade(state), key: key.to_owned() }))
    }
}

#[derive(Clone)]
pub struct Sidebar {
    panel: *mut GtkWidget,
    scroll: *mut GtkWidget,
    vadj: *mut GtkAdjustment,
    title_lbl: *mut GtkWidget,
    search: *mut GtkWidget,
    groups: [Group; 3],
    tmux: *mut GtkWidget,
    tmux_heading: *mut GtkWidget,
    tmux_list: *mut GtkWidget,
    empty: *mut GtkWidget,
    folders: Rc<RefCell<FolderUiState>>,
}

fn build_sidebar_header(panel: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let header = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        let title_lbl = label(text::sidebar_title(), "sidebar-title");
        gtk_box_pack_start(header, title_lbl, 1, 1, 0);
        gtk_box_pack_end(header, label(c"Alt 1–9", "sidebar-shortcut"), 0, 0, 0);
        gtk_box_pack_start(panel, header, 0, 1, 0);
        title_lbl
    }
}

fn build_sidebar_scroll(panel: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkAdjustment, *mut GtkWidget) {
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_kinetic_scrolling(scroll, 0);
        let vadj = gtk_scrolled_window_get_vadjustment(scroll);
        let list = gtk_box_new(GTK_ORIENTATION_VERTICAL, 14);
        gtk_container_add(scroll, list);
        gtk_box_pack_start(panel, scroll, 1, 1, 0);
        (scroll, vadj, list)
    }
}

fn build_sidebar_search(panel: *mut GtkWidget, on_search: WidgetHandler) -> *mut GtkWidget {
    unsafe {
        let search = gtk_search_entry_new();
        gtk_entry_set_placeholder_text(search, text::sidebar_search_placeholder().as_ptr());
        connect_entry_changed(search, on_search);
        gtk_box_pack_start(panel, search, 0, 1, 0);
        search
    }
}

fn build_sidebar_groups(list: *mut GtkWidget) -> [Group; 3] {
    [
        Group::new(list, text::sidebar_group_local()),
        Group::new(list, text::sidebar_group_ssh()),
        Group::new(list, text::sidebar_group_monitor()),
    ]
}

impl Sidebar {
    pub fn new(parent: *mut GtkWidget, on_search: WidgetHandler) -> Self {
        let panel = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 8) };
        unsafe { gtk_widget_set_size_request(panel, 320, -1) };
        add_class(panel, "sidebar");
        unsafe { gtk_box_pack_start(parent, panel, 0, 1, 0) };
        let title_lbl = build_sidebar_header(panel);
        let search = build_sidebar_search(panel, on_search);
        let (scroll, vadj, list) = build_sidebar_scroll(panel);
        let groups = build_sidebar_groups(list);
        let (tmux, tmux_heading, tmux_list) = build_tmux_disclosure(list);
        let empty = label(text::sidebar_empty_tabs(), "sidebar-empty");
        pack_hidden(list, empty);
        let folders = Rc::new(RefCell::new(FolderUiState::new(groups[0].list)));
        Self { panel, scroll, vadj, title_lbl, search, groups, tmux, tmux_heading, tmux_list, empty, folders }
    }

    pub fn set_new_folder_tab_handler(&self, on_new_tab: FolderNewTabHandler) {
        self.folders.borrow_mut().on_new_tab = on_new_tab;
    }

    pub fn panel_widget(&self) -> *mut GtkWidget {
        self.panel
    }

    #[cfg(test)]
    pub fn scroll_widget(&self) -> *mut GtkWidget {
        self.scroll
    }

    #[cfg(test)]
    pub fn folder_keys(&self) -> Vec<String> {
        let state = self.folders.borrow();
        let mut keys = Vec::new();
        for child in tests::box_children(state.local_list) {
            if let Some((k, _)) = state.sections.iter().find(|(_, sec)| sec.root == child) {
                keys.push(k.clone());
            }
        }
        keys
    }

    #[cfg(test)]
    pub fn folder_widgets(
        &self,
        key: &str,
    ) -> Option<(*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget)> {
        self.folders
            .borrow()
            .sections
            .get(key)
            .map(|s| (s.root, s.toggle_btn, s.add_btn, s.rows_box))
    }

    pub fn reveal_drag_y(&self, container: *mut GtkWidget, local_y: f64) {
        let list = unsafe { gtk_widget_get_parent(self.groups[0].widget) };
        let Some(y) = crate::platform::translate_widget_y_to_ancestor(container, list, local_y) else {
            return;
        };
        unsafe {
            let max_y = f64::from(gtk_widget_get_allocated_height(list)).max(1.0);
            let lo = (y - 36.0).clamp(0.0, max_y);
            let hi = (y + 36.0).clamp(lo, max_y);
            gtk_adjustment_clamp_page(self.vadj, lo, hi);
        }
    }

    fn row_container(&self, session: &TermSession) -> *mut GtkWidget {
        let widget = session.sidebar_row.widget;
        let parent = (!widget.is_null())
            .then(|| unsafe { gtk_widget_get_parent(widget) })
            .unwrap_or(ptr::null_mut());
        if parent.is_null() { self.groups[session.sidebar_row.group.get()].list } else { parent }
    }

    pub fn build_reorder_ops(&self, sessions: &[TermSession]) -> Vec<BoxReorder> {
        let mut counts: Vec<(*mut GtkWidget, c_int)> = Vec::new();
        sessions
            .iter()
            .map(|s| {
                let container = self.row_container(s);
                let position = next_container_pos(&mut counts, container);
                BoxReorder { container, child: s.sidebar_row.widget, position }
            })
            .collect()
    }

    #[cfg(test)]
    pub fn group_list_widget(&self, category: SessionCategory) -> *mut GtkWidget {
        self.groups[group_index(category)].list
    }

    pub fn append_tab(
        &self,
        category: SessionCategory,
        sid: usize,
        on_select: WidgetHandler,
        on_context: EventHandler,
    ) -> SidebarRow {
        let handlers = SidebarRowHandlers {
            on_select,
            on_press: on_context,
            on_motion: noop_row_event,
            on_release: noop_row_event,
        };
        self.append_tab_with_handlers(category, sid, handlers)
    }

    pub fn append_tab_with_handlers(
        &self,
        category: SessionCategory,
        sid: usize,
        handlers: SidebarRowHandlers,
    ) -> SidebarRow {
        let row = SidebarRow::new(category);
        connect_sidebar_row_signals(row.widget, sid, handlers);
        unsafe {
            gtk_box_pack_start(self.groups[group_index(category)].list, row.widget, 0, 1, 0);
            gtk_widget_show_all(row.widget);
        }
        row
    }

    pub fn append_tmux(
        &self,
        title: &str,
        tooltip: &str,
        index: usize,
        on_jump: WidgetHandler,
    ) -> *mut GtkWidget {
        let button = build_tmux_button(title, tooltip, index, on_jump);
        unsafe {
            gtk_box_pack_start(self.tmux_list, button, 0, 1, 0);
            gtk_widget_show_all(button);
        }
        button
    }

    pub fn tmux_widget(&self) -> *mut GtkWidget {
        self.tmux
    }

    fn sync_nonlocal_session(&self, session: &TermSession, dirty: &mut [bool; 3], moves: &mut Vec<(*mut GtkWidget, *mut GtkWidget)>) {
        let target = group_index(session.effective_category());
        let current = session.sidebar_row.group.get();
        let row = session.sidebar_row.widget;
        if target != 0 && !row.is_null() && unsafe { gtk_widget_get_parent(row) } != self.groups[target].list {
            moves.push((row, self.groups[target].list));
            dirty[target] = true;
        }
        if current != target {
            session.sidebar_row.group.set(target);
            dirty[current] = true;
            dirty[target] = true;
        }
    }

    fn mark_dirty_nonlocal_moves(&self, sessions: &[TermSession]) -> ([bool; 3], Vec<(*mut GtkWidget, *mut GtkWidget)>) {
        let mut dirty = [false; 3];
        let mut moves = Vec::new();
        for session in sessions {
            self.sync_nonlocal_session(session, &mut dirty, &mut moves);
        }
        (dirty, moves)
    }

    fn collect_group_ops(
        &self,
        sessions: &[TermSession],
    ) -> (Vec<(*mut GtkWidget, *mut GtkWidget)>, Vec<(*mut GtkWidget, Vec<*mut GtkWidget>)>) {
        let (dirty, moves) = self.mark_dirty_nonlocal_moves(sessions);
        let reorders = (1..=2)
            .filter(|&idx| dirty[idx] && !self.groups[idx].list.is_null())
            .map(|idx| {
                let order = sessions
                    .iter()
                    .filter(|s| s.sidebar_row.group.get() == idx && !s.sidebar_row.widget.is_null())
                    .map(|s| s.sidebar_row.widget)
                    .collect();
                (self.groups[idx].list, order)
            })
            .collect();
        (moves, reorders)
    }

    fn collect_folder_plans(
        &self,
        sessions: &[TermSession],
        query: &str,
        row_matches: &[bool],
    ) -> (Vec<FolderPlan>, bool) {
        let home = home_dir();
        let entries = collect_local_dir_entries(sessions, row_matches);
        let grouped = group_local_entries(
            entries.iter().map(|(idx, dir, vis)| (*idx, dir.as_str(), *vis)),
            &home,
        );
        let collapsed_set = self.folders.borrow().snapshot_collapsed();
        let query_empty = query.is_empty();
        let plans: Vec<FolderPlan> = grouped
            .into_iter()
            .map(|g| build_folder_plan(g, sessions, &collapsed_set, query_empty))
            .collect();
        let any_visible = plans.iter().any(|p| p.visible);
        (plans, any_visible)
    }

    pub fn prepare_filter<'a>(
        &self,
        query: &str,
        sessions: &[TermSession],
        tmux_rows: impl Iterator<Item = (&'a str, *mut GtkWidget)>,
    ) -> SidebarUpdate {
        let (moves, reorders) = self.collect_group_ops(sessions);
        let (row_matches, row_vis, mut visible_groups) = collect_row_visibility(query, sessions);
        let (folder_plans, local_visible) = self.collect_folder_plans(sessions, query, &row_matches);
        visible_groups[0] = local_visible;
        let (tmux_count, tmux_vis) = collect_tmux_visibility(query, tmux_rows);
        SidebarUpdate {
            moves, reorders, folder_plans, folders: self.folders.clone(),
            query_empty: query.is_empty(), row_vis, tmux_vis,
            chrome: self.chrome_targets(visible_groups),
            tmux_count, show_empty: tmux_count == 0 && !visible_groups.contains(&true),
            empty_msg: empty_state_label(query.is_empty()),
        }
    }

    fn chrome_targets(&self, vis: [bool; 3]) -> SidebarChromeTargets {
        SidebarChromeTargets {
            title_lbl: self.title_lbl, search: self.search,
            tmux: self.tmux, tmux_heading: self.tmux_heading, empty: self.empty,
            group_headings: [(self.groups[0].heading, text::sidebar_group_local()), (self.groups[1].heading, text::sidebar_group_ssh()), (self.groups[2].heading, text::sidebar_group_monitor())],
            group_vis: [(self.groups[0].widget, vis[0]), (self.groups[1].widget, vis[1]), (self.groups[2].widget, vis[2])],
        }
    }

    #[cfg(test)]
    pub fn filter<'a>(
        &self,
        query: &str,
        sessions: &[TermSession],
        tmux_rows: impl Iterator<Item = (&'a str, *mut GtkWidget)>,
    ) {
        self.prepare_filter(query, sessions, tmux_rows).apply();
    }
}

fn empty_state_label(query_empty: bool) -> &'static CStr {
    if query_empty { text::sidebar_empty_tabs() } else { text::sidebar_empty_search() }
}

struct SidebarChromeTargets {
    title_lbl: *mut GtkWidget, search: *mut GtkWidget,
    tmux: *mut GtkWidget, tmux_heading: *mut GtkWidget, empty: *mut GtkWidget,
    group_headings: [(*mut GtkWidget, &'static CStr); 3],
    group_vis: [(*mut GtkWidget, bool); 3],
}

impl SidebarChromeTargets {
    fn apply_texts(&self, tmux_count: usize, empty_msg: &'static CStr) {
        unsafe {
            for &(h, title) in &self.group_headings {
                gtk_label_set_text(h, title.as_ptr());
            }
            gtk_label_set_text(self.title_lbl, text::sidebar_title().as_ptr());
            gtk_entry_set_placeholder_text(self.search, text::sidebar_search_placeholder().as_ptr());
            gtk_label_set_text(self.empty, empty_msg.as_ptr());
        }
        set_label_text(self.tmux_heading, &format!("{} · {tmux_count}", text::sidebar_tmux_prefix()));
    }
}

pub struct SidebarUpdate {
    moves: Vec<(*mut GtkWidget, *mut GtkWidget)>,
    reorders: Vec<(*mut GtkWidget, Vec<*mut GtkWidget>)>,
    folder_plans: Vec<FolderPlan>,
    folders: Rc<RefCell<FolderUiState>>,
    query_empty: bool,
    row_vis: Vec<(*mut GtkWidget, bool)>,
    tmux_vis: Vec<(*mut GtkWidget, bool)>,
    chrome: SidebarChromeTargets,
    tmux_count: usize,
    show_empty: bool,
    empty_msg: &'static CStr,
}

impl SidebarUpdate {
    pub fn apply(self) {
        for (widget, to_list) in self.moves {
            reparent_row(widget, to_list);
        }
        apply_folder_plans(&self.folders, &self.folder_plans, self.query_empty);
        apply_box_reorders(self.reorders);
        set_widgets_visible(self.row_vis);
        set_widgets_visible(self.tmux_vis);
        set_widgets_visible(self.chrome.group_vis.to_vec());
        self.chrome.apply_texts(self.tmux_count, self.empty_msg);
        unsafe {
            gtk_widget_set_visible(self.chrome.tmux, i32::from(self.tmux_count > 0));
            gtk_widget_set_visible(self.chrome.empty, i32::from(self.show_empty));
        }
    }
}

fn collect_local_dir_entries(
    sessions: &[TermSession],
    row_matches: &[bool],
) -> Vec<(usize, String, bool)> {
    sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| s.effective_category() == SessionCategory::Project)
        .map(|(idx, s)| (idx, fallback_session_dir(s), row_matches[idx]))
        .collect()
}

fn build_folder_plan(
    g: crate::sidebar_folders::FolderGroup,
    sessions: &[TermSession],
    collapsed_set: &HashSet<String>,
    query_empty: bool,
) -> FolderPlan {
    let total_count = g.item_indices.len();
    let visible_count = g.visible_indices.len();
    let visible = !g.visible_indices.is_empty();
    let collapsed = query_empty && collapsed_set.contains(&g.key);
    let row_widgets = g
        .item_indices
        .into_iter()
        .map(|i| sessions[i].sidebar_row.widget)
        .filter(|w| !w.is_null())
        .collect();
    FolderPlan { label: g.label, tooltip: g.tooltip, total_count, visible_count, collapsed, visible, row_widgets, key: g.key }
}

fn row_matches_query(session: &TermSession, query: &str) -> bool {
    query.is_empty() || search_text(session).contains(query)
}

fn collect_row_visibility(
    query: &str,
    sessions: &[TermSession],
) -> (Vec<bool>, Vec<(*mut GtkWidget, bool)>, [bool; 3]) {
    let mut matches = Vec::with_capacity(sessions.len());
    let mut vis = Vec::with_capacity(sessions.len());
    let mut groups = [false; 3];
    for session in sessions {
        let m = row_matches_query(session, query);
        matches.push(m);
        groups[group_index(session.effective_category())] |= m;
        if !session.sidebar_row.widget.is_null() {
            vis.push((session.sidebar_row.widget, m));
        }
    }
    (matches, vis, groups)
}

fn collect_tmux_visibility<'a>(
    query: &str,
    rows: impl Iterator<Item = (&'a str, *mut GtkWidget)>,
) -> (usize, Vec<(*mut GtkWidget, bool)>) {
    let mut count = 0;
    let mut vis = Vec::new();
    for (key, widget) in rows {
        let m = key.contains(query);
        count += usize::from(m);
        vis.push((widget, m));
    }
    (count, vis)
}

fn set_widgets_visible(pairs: Vec<(*mut GtkWidget, bool)>) {
    for (widget, vis) in pairs {
        unsafe { gtk_widget_set_visible(widget, i32::from(vis)) };
    }
}

fn apply_box_reorders(reorders: Vec<(*mut GtkWidget, Vec<*mut GtkWidget>)>) {
    for (list, order) in reorders {
        for (pos, widget) in order.into_iter().enumerate() {
            unsafe { gtk_box_reorder_child(list, widget, pos as c_int) };
        }
    }
}

#[derive(Clone, Copy)]
struct Group {
    widget: *mut GtkWidget,
    heading: *mut GtkWidget,
    list: *mut GtkWidget,
}

impl Group {
    fn new(parent: *mut GtkWidget, title: &CStr) -> Self {
        let widget = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 4) };
        let heading = label(title, "sidebar-group-label");
        let list = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 2) };
        pack_group_widgets(parent, widget, heading, list);
        Self { widget, heading, list }
    }
}

fn pack_group_widgets(
    parent: *mut GtkWidget,
    widget: *mut GtkWidget,
    heading: *mut GtkWidget,
    list: *mut GtkWidget,
) {
    unsafe {
        gtk_box_pack_start(widget, heading, 0, 1, 0);
        gtk_box_pack_start(widget, list, 0, 1, 0);
        gtk_widget_show_all(widget);
        pack_hidden(parent, widget);
    }
}

fn group_index(category: SessionCategory) -> usize {
    match category {
        SessionCategory::Project => 0,
        SessionCategory::Remote => 1,
        SessionCategory::Daemon => 2,
    }
}

fn pack_tmux_widgets(list: *mut GtkWidget, tmux: *mut GtkWidget, heading: *mut GtkWidget, tmux_list: *mut GtkWidget) {
    unsafe {
        gtk_expander_set_label_widget(tmux, heading);
        gtk_widget_set_visible(heading, 1);
        gtk_container_add(tmux, tmux_list);
        gtk_widget_set_visible(tmux_list, 1);
        pack_hidden(list, tmux);
    }
}

fn build_tmux_disclosure(list: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let tmux = unsafe { gtk_expander_new(ptr::null()) };
    add_class(tmux, "sidebar-disclosure");
    let tmux_heading = label(c"Сессии tmux", "sidebar-tmux-label");
    let tmux_list = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 2) };
    pack_tmux_widgets(list, tmux, tmux_heading, tmux_list);
    (tmux, tmux_heading, tmux_list)
}

fn next_container_pos(counts: &mut Vec<(*mut GtkWidget, c_int)>, container: *mut GtkWidget) -> c_int {
    if let Some((_, count)) = counts.iter_mut().find(|(w, _)| *w == container) {
        let pos = *count;
        *count += 1;
        return pos;
    }
    counts.push((container, 1));
    0
}

fn connect_row_event(widget: *mut GtkWidget, signal: &CStr, handler: EventHandler, data: *mut c_void) {
    unsafe { g_signal_connect_data(widget, signal.as_ptr(), handler as *const c_void, data, ptr::null(), 0) };
}

fn connect_sidebar_row_signals(widget: *mut GtkWidget, sid: usize, handlers: SidebarRowHandlers) {
    let data = sid as *mut c_void;
    connect_click(widget, handlers.on_select, data);
    unsafe { gtk_widget_add_events(widget, (GDK_BUTTON_PRESS_MASK | GDK_BUTTON_RELEASE_MASK | GDK_BUTTON1_MOTION_MASK) as c_int) };
    connect_row_event(widget, c"button-press-event", handlers.on_press, data);
    connect_row_event(widget, c"motion-notify-event", handlers.on_motion, data);
    connect_row_event(widget, c"button-release-event", handlers.on_release, data);
}

extern "C" fn drop_folder_cb_data(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data as *mut FolderCallbackData)) };
    }
}

fn flip_collapsed_state(state: &mut FolderUiState, key: &str) -> bool {
    if state.collapsed.remove(key) {
        false
    } else {
        state.collapsed.insert(key.to_owned());
        true
    }
}

extern "C" fn toggle_folder_cb(_btn: *mut GtkWidget, data: *mut c_void) {
    let Some(cb) = (!data.is_null()).then(|| unsafe { &*(data as *const FolderCallbackData) }) else { return };
    let Some(rc) = cb.state.upgrade() else { return };
    let mut state = rc.borrow_mut();
    let collapsed = flip_collapsed_state(&mut state, &cb.key);
    if let Some(sec) = state.sections.get(&cb.key) {
        sec.set_collapsed(collapsed);
    }
}

extern "C" fn add_folder_tab_cb(_btn: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let cb = unsafe { &*(data as *const FolderCallbackData) };
    let handler = {
        let Some(rc) = cb.state.upgrade() else {
            return;
        };
        let mut state = rc.borrow_mut();
        state.collapsed.remove(&cb.key);
        state.on_new_tab
    };
    handler(&cb.key);
}

fn connect_folder_button(
    btn: *mut GtkWidget,
    handler: WidgetHandler,
    state: &Rc<RefCell<FolderUiState>>,
    key: &str,
) {
    let payload = FolderCallbackData::boxed(state, key);
    unsafe {
        g_signal_connect_data(
            btn,
            c"clicked".as_ptr(),
            handler as *const c_void,
            payload as *mut c_void,
            drop_folder_cb_data as *const c_void,
            0,
        );
    }
}

fn build_tmux_button(title: &str, tooltip: &str, index: usize, on_jump: WidgetHandler) -> *mut GtkWidget {
    unsafe {
        let button = gtk_button_new();
        add_class(button, "sidebar-action");
        let title_label = label(c"", "sidebar-action-label");
        set_label_text(title_label, title);
        gtk_container_add(button, title_label);
        set_tooltip(button, tooltip);
        connect_click(button, on_jump, index as *mut c_void);
        button
    }
}

fn build_folder_toggle_row() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let toggle_btn = gtk_button_new();
        add_class(toggle_btn, "sidebar-folder-toggle");
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let chevron = label(c"▾", "sidebar-folder-chevron");
        let icon = gtk_image_new_from_icon_name(c"folder-symbolic".as_ptr(), GTK_ICON_SIZE_MENU);
        add_class(icon, "sidebar-folder-icon");
        let name_label = label(c"", "sidebar-folder-name");
        let count_label = label(c"", "sidebar-folder-count");
        pack_folder_toggle_children(row, chevron, icon, name_label, count_label);
        gtk_container_add(toggle_btn, row);
        (toggle_btn, chevron, name_label, count_label)
    }
}

fn pack_folder_toggle_children(
    row: *mut GtkWidget,
    chevron: *mut GtkWidget,
    icon: *mut GtkWidget,
    name_label: *mut GtkWidget,
    count_label: *mut GtkWidget,
) {
    unsafe {
        gtk_box_pack_start(row, chevron, 0, 0, 0);
        gtk_box_pack_start(row, icon, 0, 0, 0);
        gtk_box_pack_start(row, name_label, 1, 1, 0);
        gtk_box_pack_end(row, count_label, 0, 0, 0);
    }
}

fn pack_folder_section_widgets(local_list: *mut GtkWidget, sec: &FolderSection, header: *mut GtkWidget) {
    unsafe {
        gtk_box_pack_start(header, sec.toggle_btn, 1, 1, 0);
        gtk_box_pack_end(header, sec.add_btn, 0, 0, 0);
        gtk_box_pack_start(sec.root, header, 0, 1, 0);
        gtk_box_pack_start(sec.root, sec.rows_box, 0, 1, 0);
        gtk_box_pack_start(local_list, sec.root, 0, 1, 0);
        gtk_widget_show_all(sec.root);
    }
}

fn classed_box(orientation: c_int, spacing: c_int, class: &str) -> *mut GtkWidget {
    let w = unsafe { gtk_box_new(orientation, spacing) };
    add_class(w, class);
    w
}

fn create_folder_section(state: &Rc<RefCell<FolderUiState>>, local_list: *mut GtkWidget, key: &str) -> FolderSection {
    let root = classed_box(GTK_ORIENTATION_VERTICAL, 2, "sidebar-folder");
    let header = classed_box(GTK_ORIENTATION_HORIZONTAL, 2, "sidebar-folder-header");
    let (toggle_btn, chevron, name_label, count_label) = build_folder_toggle_row();
    let add_btn = unsafe { gtk_button_new_with_label(c"+".as_ptr()) };
    add_class(add_btn, "sidebar-folder-add");
    let rows_box = classed_box(GTK_ORIENTATION_VERTICAL, 2, "sidebar-folder-rows");
    let sec = FolderSection { root, toggle_btn, add_btn, chevron, name_label, count_label, rows_box };
    pack_folder_section_widgets(local_list, &sec, header);
    connect_folder_button(toggle_btn, toggle_folder_cb, state, key);
    connect_folder_button(add_btn, add_folder_tab_cb, state, key);
    sec
}

fn apply_folder_plan(
    sec: &FolderSection,
    local_list: *mut GtkWidget,
    plan: &FolderPlan,
    pos: usize,
    query_empty: bool,
) {
    sec.set_header(plan, plan.display_count(query_empty));
    for (rpos, &row) in plan.row_widgets.iter().enumerate() {
        reparent_row(row, sec.rows_box);
        unsafe { gtk_box_reorder_child(sec.rows_box, row, rpos as c_int) };
    }
    unsafe { gtk_box_reorder_child(local_list, sec.root, pos as c_int) };
}

fn ensure_folder_section(
    folders: &Rc<RefCell<FolderUiState>>,
    state: &mut FolderUiState,
    key: &str,
) -> FolderSection {
    if let Some(existing) = state.sections.get(key) {
        return existing.clone();
    }
    let created = create_folder_section(folders, state.local_list, key);
    state.sections.insert(key.to_owned(), created.clone());
    created
}

fn sync_folder_section_order(
    folders: &Rc<RefCell<FolderUiState>>,
    state: &mut FolderUiState,
    plans: &[FolderPlan],
    query_empty: bool,
) {
    let local_list = state.local_list;
    for (pos, plan) in plans.iter().enumerate() {
        let sec = ensure_folder_section(folders, state, &plan.key);
        apply_folder_plan(&sec, local_list, plan, pos, query_empty);
    }
}

fn remove_stale_folder_sections(state: &mut FolderUiState, plans: &[FolderPlan]) {
    let stale: Vec<String> = state
        .sections
        .keys()
        .filter(|k| !plans.iter().any(|p| &p.key == *k))
        .cloned()
        .collect();
    for key in stale {
        if let Some(sec) = state.sections.remove(&key) {
            unsafe { gtk_widget_destroy(sec.root) };
        }
    }
}

fn apply_folder_plans(
    folders: &Rc<RefCell<FolderUiState>>,
    plans: &[FolderPlan],
    query_empty: bool,
) {
    let mut state = folders.borrow_mut();
    if state.local_list.is_null() {
        return;
    }
    sync_folder_section_order(folders, &mut state, plans, query_empty);
    remove_stale_folder_sections(&mut state, plans);
}

fn move_child_box(parent: *mut GtkWidget, widget: *mut GtkWidget, to_list: *mut GtkWidget) {
    unsafe {
        g_object_ref(widget);
        if !parent.is_null() {
            gtk_container_remove(parent, widget);
        }
        gtk_box_pack_start(to_list, widget, 0, 1, 0);
        g_object_unref(widget);
    }
}

fn reparent_row(widget: *mut GtkWidget, to_list: *mut GtkWidget) {
    if widget.is_null() || to_list.is_null() {
        return;
    }
    let parent = unsafe { gtk_widget_get_parent(widget) };
    if parent != to_list {
        move_child_box(parent, widget, to_list);
    }
}

pub struct SidebarRow {
    widget: *mut GtkWidget,
    icon: *mut GtkWidget,
    title: *mut GtkWidget,
    detail: *mut GtkWidget,
    status: *mut GtkWidget,
    metric: *mut GtkWidget,
    group: Cell<usize>,
    icon_cat: Cell<SessionCategory>,
}

fn build_row_labels(content: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let title = label(c"", "sidebar-row-title");
        let detail = label(c"", "sidebar-detail");
        gtk_label_set_ellipsize(detail, PANGO_ELLIPSIZE_MIDDLE);
        let status = label(c"", "sidebar-status");
        let metric = label(c"", "sidebar-metric");
        let lines = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
        pack_line(lines, title, status);
        pack_line(lines, detail, metric);
        gtk_box_pack_start(content, lines, 1, 1, 0);
        (title, detail, status, metric)
    }
}

fn build_row_icon(category: SessionCategory, content: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let icon = gtk_image_new_from_icon_name(resolve_category_icon(category).as_ptr(), GTK_ICON_SIZE_MENU);
        add_class(icon, "sidebar-icon");
        gtk_widget_set_valign(icon, GTK_ALIGN_START);
        gtk_box_pack_start(content, icon, 0, 0, 0);
        icon
    }
}

impl SidebarRow {
    fn new(category: SessionCategory) -> Self {
        let widget = unsafe { gtk_button_new() };
        add_class(widget, "sidebar-row");
        let content = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 9) };
        unsafe { gtk_container_add(widget, content) };
        let icon = build_row_icon(category, content);
        let (title, detail, status, metric) = build_row_labels(content);
        Self {
            widget, icon, title, detail, status, metric,
            group: Cell::new(group_index(category)),
            icon_cat: Cell::new(category),
        }
    }

    fn take_icon_change(&self, category: SessionCategory) -> Option<SessionCategory> {
        (self.icon_cat.get() != category).then(|| {
            self.icon_cat.set(category);
            category
        })
    }

    pub fn prepare_update(
        &self,
        index: usize,
        session: &TermSession,
        mi: Option<(f64, &str)>,
        git_dirty: &str,
        active: bool,
    ) -> RowUpdate {
        let eff_cat = session.effective_category();
        let (status, style, live_status, tip_status) = compute_row_status(session);
        let detail = tab_detail(session, git_dirty);
        let (metric, metric_tip, alert) = compute_row_metrics(session, eff_cat, mi);
        let tip = format_row_tooltip(&session.title, &tip_status, &detail, &metric_tip, index);
        RowUpdate {
            widget: self.widget, icon: self.icon, title_lbl: self.title,
            detail_lbl: self.detail, status_lbl: self.status, metric_lbl: self.metric,
            new_icon: self.take_icon_change(eff_cat), agent: session.effective_agent(),
            active, status, live_status, style, title: session.title.clone(),
            detail, metric, alert, tip,
        }
    }

    #[cfg(test)]
    pub fn update(
        &self,
        index: usize,
        session: &TermSession,
        mi: Option<(f64, &str)>,
        git_dirty: &str,
        active: bool,
    ) {
        self.prepare_update(index, session, mi, git_dirty, active).apply();
    }

    pub fn row_widget(&self) -> *mut GtkWidget {
        self.widget
    }

    #[cfg(test)]
    pub fn metric_widget(&self) -> *mut GtkWidget {
        self.metric
    }

    #[cfg(test)]
    pub fn icon_widget(&self) -> *mut GtkWidget {
        self.icon
    }

    #[cfg(test)]
    pub fn detail_text_for_test(&self) -> String {
        tests::widget_label_str(self.detail)
    }

    pub fn destroy(self) {
        if !self.widget.is_null() {
            unsafe { gtk_widget_destroy(self.widget) };
        }
    }

    #[cfg(test)]
    pub fn empty_for_test(category: SessionCategory) -> Self {
        Self {
            widget: ptr::null_mut(),
            icon: ptr::null_mut(),
            title: ptr::null_mut(),
            detail: ptr::null_mut(),
            status: ptr::null_mut(),
            metric: ptr::null_mut(),
            group: Cell::new(group_index(category)),
            icon_cat: Cell::new(category),
        }
    }
}

fn compute_row_status(
    session: &TermSession,
) -> (&'static CStr, &'static str, Option<String>, String) {
    let (status, style) = status_for(session);
    let live_status = (style == "sidebar-working" && session.category != SessionCategory::Daemon)
        .then(|| session.live_turn_secs(Instant::now()))
        .flatten()
        .map(format_working_sidebar_status);
    let text = live_status
        .as_deref()
        .map_or_else(|| status.to_string_lossy(), std::borrow::Cow::Borrowed);
    let tip = format_settled_status(&text, session.settled_turn_secs());
    (status, style, live_status, tip)
}

fn format_mi_badge(eff_cat: SessionCategory, mi: Option<(f64, &str)>) -> String {
    let eff_mi = (eff_cat != SessionCategory::Remote).then_some(mi).flatten();
    eff_mi.map_or(String::new(), |(score, grade)| format!("MI {score:.1} · {grade}"))
}

fn compute_row_metrics(
    session: &TermSession,
    eff_cat: SessionCategory,
    mi: Option<(f64, &str)>,
) -> (String, String, ContextAlert) {
    let snap = session.telemetry.snapshot();
    let ctx_badge = snap.compact_context_badge();
    let mi_str = format_mi_badge(eff_cat, mi);
    let agents = crate::pane_radar::format_agents_badge(session.pane_registry.active_agent_count());
    let metric = combine_metric_parts(ctx_badge.as_deref(), &combine_metric_parts(agents.as_deref(), &mi_str));
    let mi_tip = format_metric_tooltip(&mi_str, local_project_root(session).as_deref());
    (metric, combine_metric_parts(ctx_badge.as_deref(), &mi_tip), snap.alert_level())
}

fn format_row_tooltip(title: &str, status: &str, detail: &str, metric: &str, index: usize) -> String {
    let head = if status.is_empty() { title.to_owned() } else { format!("{title} · {status}") };
    let mut out = format!("{head}\n{detail}");
    if !metric.is_empty() {
        out.push_str(&format!("\n{metric}"));
    }
    if index < 9 {
        out.push_str(&format!("\nAlt+{}", index + 1));
    }
    out
}

fn combine_metric_parts(ctx_badge: Option<&str>, mi_text: &str) -> String {
    match (ctx_badge, mi_text.is_empty()) {
        (Some(ctx), false) => format!("{ctx} · {mi_text}"),
        (Some(ctx), true) => ctx.to_owned(),
        (None, false) => mi_text.to_owned(),
        (None, true) => String::new(),
    }
}

fn format_settled_status(status: &str, settled_secs: Option<u64>) -> String {
    match (status.is_empty(), settled_secs) {
        (false, Some(secs)) => format!("{status} · ⏱ {}", format_turn_duration(secs)),
        (true, Some(secs)) => format!("⏱ {}", format_turn_duration(secs)),
        _ => status.to_owned(),
    }
}

fn set_status_classes(status_label: *mut GtkWidget, style: &'static str) {
    for class in ["sidebar-approval", "sidebar-ready", "sidebar-working", "sidebar-ended"] {
        set_class(status_label, class, style == class);
    }
}

pub struct RowUpdate {
    widget: *mut GtkWidget,
    icon: *mut GtkWidget,
    title_lbl: *mut GtkWidget,
    detail_lbl: *mut GtkWidget,
    status_lbl: *mut GtkWidget,
    metric_lbl: *mut GtkWidget,
    new_icon: Option<SessionCategory>,
    agent: String,
    active: bool,
    status: &'static CStr,
    live_status: Option<String>,
    style: &'static str,
    title: String,
    detail: String,
    metric: String,
    alert: ContextAlert,
    tip: String,
}

impl RowUpdate {
    fn apply_header(&self) {
        if let Some(cat) = self.new_icon.filter(|_| !self.icon.is_null()) {
            set_image_icon(self.icon, resolve_category_icon(cat));
        }
        set_agent_accent(self.icon, &self.agent);
        set_class(self.widget, "sidebar-selected", self.active);
    }

    fn apply_metrics(&self) {
        set_class(self.metric_lbl, "sidebar-metric-warn", self.alert == ContextAlert::Warn);
        set_class(self.metric_lbl, "sidebar-metric-high", self.alert == ContextAlert::High);
        set_label_text(self.metric_lbl, &self.metric);
    }

    pub fn apply(self) {
        self.apply_header();
        apply_row_status(self.status_lbl, self.status, self.live_status.as_deref(), self.style);
        set_label_text(self.title_lbl, &self.title);
        set_label_text(self.detail_lbl, &self.detail);
        self.apply_metrics();
        set_tooltip(self.widget, &self.tip);
    }
}

fn apply_row_status(
    status_label: *mut GtkWidget,
    status: &'static CStr,
    live_status: Option<&str>,
    style: &'static str,
) {
    set_status_classes(status_label, style);
    if let Some(live) = live_status {
        set_label_text(status_label, live);
    } else {
        unsafe { gtk_label_set_text(status_label, status.as_ptr()) };
    }
}

fn status_for(session: &TermSession) -> (&'static CStr, &'static str) {
    match session.effective_category() {
        SessionCategory::Remote if session.remote().is_some() && session.child_pid == 0 => {
            (c"Нет связи", "sidebar-ended")
        }
        _ if session.active_pid() == 0 => (c"Завершена", "sidebar-ended"),
        _ if session.state == WorkState::NeedsApproval => (text::STATUS_APPROVAL, "sidebar-approval"),
        _ if session.unread => (c"Готово", "sidebar-ready"),
        SessionCategory::Daemon => (c"Онлайн", "sidebar-working"),
        _ if session.state == WorkState::Working => (c"В работе", "sidebar-working"),
        _ => (c"", ""),
    }
}

fn local_git_head(session: &TermSession) -> Option<GitHeadInfo> {
    let dir = session.active_local_cwd()?;
    detect_git_head(Path::new(&dir))
}

fn local_project_root(session: &TermSession) -> Option<String> {
    let dir = session.active_local_cwd()?;
    resolve_project_root(&dir, &home_dir())
}

fn format_metric_tooltip(metric: &str, project_root: Option<&str>) -> String {
    match (metric.is_empty(), project_root) {
        (true, _) => String::new(),
        (false, Some(root)) => format!("{metric} ({})", shorten_home(root)),
        (false, None) => metric.to_owned(),
    }
}

fn format_git_suffix(git: Option<&GitHeadInfo>, git_dirty: &str) -> String {
    let Some(info) = git else { return String::new() };
    if git_dirty.is_empty() {
        format!(" · {}", info.badge())
    } else {
        format!(" · {} · {git_dirty}", info.badge())
    }
}

fn format_local_detail(
    directory: &str,
    has_tmux: bool,
    git: Option<&GitHeadInfo>,
    git_dirty: &str,
) -> String {
    let base = if has_tmux {
        format!("tmux · {}", shorten_home(directory))
    } else {
        shorten_home(directory)
    };
    format!("{base}{}", format_git_suffix(git, git_dirty))
}

fn fallback_session_dir(session: &TermSession) -> String {
    session.active_local_cwd().unwrap_or_else(|| {
        if session.proc_info.cwd.is_empty() {
            session.cwd.clone()
        } else {
            session.proc_info.cwd.clone()
        }
    })
}

fn format_remote_detail(host: &str, osc_title: &str) -> String {
    if osc_title.is_empty() || osc_title == host {
        host.to_owned()
    } else {
        format!("{host} · {osc_title}")
    }
}

fn session_remote_host(session: &TermSession) -> &str {
    session
        .remote()
        .map(|r| r.host.as_str())
        .filter(|h| !h.is_empty())
        .unwrap_or(session.proc_info.ssh_host.as_str())
}

pub fn tab_detail(session: &TermSession, git_dirty: &str) -> String {
    if let Some((name, window, _, directory)) = &session.remote_tmux {
        return format!("tmux:{name}/{window} · {}", shorten_home(directory));
    }
    if session.effective_category() == SessionCategory::Remote {
        return format_remote_detail(session_remote_host(session), &session.osc_title);
    }
    let directory = fallback_session_dir(session);
    let git = local_git_head(session);
    format_local_detail(&directory, !session.proc_info.has_tmux.is_empty(), git.as_ref(), git_dirty)
}

fn search_folder_name(session: &TermSession) -> String {
    (session.effective_category() == SessionCategory::Project)
        .then(|| resolve_project_folder(&fallback_session_dir(session), &home_dir()))
        .unwrap_or_default()
}

fn search_text(session: &TermSession) -> String {
    let live_cwd = session.active_local_cwd().unwrap_or_default();
    let branch = local_git_head(session).map_or_else(String::new, |git| git.badge());
    let root = local_project_root(session).unwrap_or_default();
    let folder = search_folder_name(session);
    let status = status_for(session).0.to_string_lossy();
    format!(
        "{} {} {} {} {} {} {} {} {} {} {}",
        session.title, session.osc_title, session.cwd, session.proc_info.cwd,
        live_cwd, session.proc_info.ssh_host, session.proc_info.agent,
        branch, root, folder, status
    )
    .to_lowercase()
}

fn label(title: &CStr, class: &str) -> *mut GtkWidget {
    unsafe {
        let widget = gtk_label_new(title.as_ptr());
        gtk_label_set_xalign(widget, 0.0);
        gtk_label_set_ellipsize(widget, PANGO_ELLIPSIZE_END);
        add_class(widget, class);
        widget
    }
}

fn pack_line(parent: *mut GtkWidget, leading: *mut GtkWidget, trailing: *mut GtkWidget) {
    unsafe {
        let line = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        gtk_box_pack_start(line, leading, 1, 1, 0);
        gtk_box_pack_end(line, trailing, 0, 0, 0);
        gtk_box_pack_start(parent, line, 0, 1, 0);
    }
}

fn pack_hidden(parent: *mut GtkWidget, child: *mut GtkWidget) {
    unsafe {
        gtk_widget_set_no_show_all(child, 1);
        gtk_widget_set_visible(child, 0);
        gtk_box_pack_start(parent, child, 0, 1, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::proc_radar::ProcInfo;
    use crate::session::refresh_smart_title;
    use crate::split_pane::TabPanes;
    use crate::tab_strip::TabStrip;
    use crate::tabs::Tab;
    use std::fs;
    use std::time::Instant;

    extern "C" fn noop_handler(_w: *mut GtkWidget, _d: *mut c_void) {}
    extern "C" fn noop_event(_w: *mut GtkWidget, _e: *const GdkEvent, _d: *mut c_void) -> c_int {
        0
    }
    fn noop_folder_tab(_path: &str) {}

    pub(super) fn box_children(container: *mut GtkWidget) -> Vec<*mut GtkWidget> {
        let mut out = Vec::new();
        unsafe {
            let head = gtk_container_get_children(container);
            let mut cur = head;
            while !cur.is_null() {
                out.push((*cur).data as *mut GtkWidget);
                cur = (*cur).next;
            }
            if !head.is_null() {
                g_list_free(head);
            }
        }
        out
    }

    #[repr(C)]
    struct GList {
        data: *mut c_void,
        next: *mut GList,
        prev: *mut GList,
    }

    extern "C" {
        fn g_list_free(list: *mut GList);
        fn gtk_container_get_children(container: *mut GtkContainer) -> *mut GList;
        fn gtk_image_get_icon_name(
            image: *mut GtkWidget,
            icon_name: *mut *const std::ffi::c_char,
            size: *mut c_int,
        );
    }

    fn widget_icon_name(image: *mut GtkWidget) -> String {
        unsafe {
            let mut name_ptr = ptr::null();
            let mut size: c_int = 0;
            gtk_image_get_icon_name(image, &mut name_ptr, &mut size);
            if name_ptr.is_null() {
                String::new()
            } else {
                CStr::from_ptr(name_ptr).to_string_lossy().into_owned()
            }
        }
    }

    pub(super) fn widget_label_str(lbl: *mut GtkWidget) -> String {
        unsafe {
            let ptr = gtk_label_get_text(lbl);
            if ptr.is_null() { String::new() } else { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
        }
    }

    fn attach_test_widgets(
        sidebar: &Sidebar,
        strip: &TabStrip,
        sid: usize,
        category: SessionCategory,
    ) -> (SidebarRow, crate::tab_strip::TabView) {
        let row_h = SidebarRowHandlers { on_select: noop_handler, on_press: noop_event, on_motion: noop_event, on_release: noop_event };
        let tab_h = crate::tab_strip::TabHandlers { on_select: noop_handler, on_close: noop_handler, on_press: noop_event, on_motion: noop_event, on_release: noop_event };
        (sidebar.append_tab_with_handlers(category, sid, row_h), strip.append(sid, category, tab_h))
    }

    fn build_test_session(
        sid: usize,
        title: &str,
        cwd: &str,
        category: SessionCategory,
        tab: Tab,
        widgets: (SidebarRow, crate::tab_strip::TabView),
    ) -> TermSession {
        let now = Instant::now();
        let mut proc_info = ProcInfo::default();
        if let Tab::Remote(ref r) = tab {
            proc_info.ssh_host.clone_from(&r.host);
        }
        TermSession {
            id: sid, base_title: title.to_owned(), profile_cwd: cwd.to_owned(), title: title.to_owned(),
            osc_title: String::new(), pinned_title: None, cwd: cwd.to_owned(), category,
            vte: ptr::null_mut(), scroll_wrap: ptr::null_mut(), panes: TabPanes::empty_for_test(),
            sidebar_row: widgets.0, top_tab: widgets.1, child_pid: 1234, state: WorkState::Idle, unread: false,
            last_output: now, work_started: now, work_started_at: None, last_turn_secs: None,
            last_notified: None, proc_info, tab, remote_tmux: None, tmux_title_from_vte: false,
            telemetry: crate::telemetry::SessionTelemetry::default(),
            pane_registry: crate::pane_registry::PaneRegistry::default(),
        }
    }

    fn make_gtk_session(
        sidebar: &Sidebar,
        strip: &TabStrip,
        sid: usize,
        title: &str,
        cwd: &str,
        category: SessionCategory,
        tab: Tab,
    ) -> TermSession {
        let widgets = attach_test_widgets(sidebar, strip, sid, category);
        build_test_session(sid, title, cwd, category, tab, widgets)
    }

    fn sync_all_views(sidebar: &Sidebar, sessions: &[TermSession]) {
        for (idx, s) in sessions.iter().enumerate() {
            s.sidebar_row.update(idx, s, Some((61.4, "B")), "+3 -1", idx == 1);
            s.top_tab.update(s, idx == 1);
        }
        sidebar.filter("", sessions, std::iter::empty());
        pump_gtk_events();
    }

    #[test]
    fn formats_local_detail_with_branch_and_worktree_marker() {
        let main = GitHeadInfo { head: "main".to_owned(), is_linked_worktree: false };
        let wt = GitHeadInfo { head: "wt/claude-1".to_owned(), is_linked_worktree: true };
        assert_eq!(format_local_detail("~/proj", false, Some(&main), ""), "~/proj · ⎇ main");
        assert_eq!(format_local_detail("~/proj-wt", false, Some(&wt), ""), "~/proj-wt · ⎇ wt/claude-1 [wt]");
        assert_eq!(format_local_detail("~/proj", true, Some(&main), ""), "tmux · ~/proj · ⎇ main");
        assert_eq!(format_local_detail("~/Downloads", false, None, ""), "~/Downloads");
    }

    #[test]
    fn formats_local_detail_with_dirty_stat_badge() {
        let main = GitHeadInfo { head: "main".to_owned(), is_linked_worktree: false };
        let wt = GitHeadInfo { head: "wt/claude-1".to_owned(), is_linked_worktree: true };
        assert_eq!(format_local_detail("~/proj", false, Some(&main), "+14 -3"), "~/proj · ⎇ main · +14 -3");
        assert_eq!(format_local_detail("~/proj-wt", false, Some(&wt), "±2"), "~/proj-wt · ⎇ wt/claude-1 [wt] · ±2");
        assert_eq!(format_local_detail("~/Downloads", false, None, "+5 -1"), "~/Downloads");
    }

    fn assert_tip_case(name: &str, status: &str, metric: &str, idx: usize, expected: &str) {
        assert_eq!(format_row_tooltip(name, status, "~/proj · ⎇ main", metric, idx), expected);
    }

    #[test]
    fn formats_sidebar_tooltip_with_status_branch_and_mi_root() {
        let m = format_metric_tooltip("MI 61.2 · B", Some("~/proj"));
        assert_eq!(
            format_row_tooltip("AkhTerm", "Ждёт апрува", "~/proj/src · ⎇ main", &m, 0),
            "AkhTerm · Ждёт апрува\n~/proj/src · ⎇ main\nMI 61.2 · B (~/proj)\nAlt+1"
        );
        let settled = format_settled_status("Готово", Some(134));
        let full_m = combine_metric_parts(Some("ctx 84k"), &m);
        assert_tip_case(
            "AkhTerm",
            &settled,
            &full_m,
            1,
            "AkhTerm · Готово · ⏱ 2m 14s\n~/proj · ⎇ main\nctx 84k · MI 61.2 · B (~/proj)\nAlt+2",
        );
    }

    fn assert_initial_folder_boxes(sidebar: &Sidebar, rows: &[*mut GtkWidget], repo_s: &str) {
        assert_eq!(sidebar.folder_keys(), vec!["/tmp".to_owned(), repo_s.to_owned()]);
        let tmp_rows = sidebar.folder_widgets("/tmp").unwrap().3;
        let repo_rows = sidebar.folder_widgets(repo_s).unwrap().3;
        assert_eq!(box_children(tmp_rows), vec![rows[0], rows[2]]);
        assert_eq!(box_children(repo_rows), vec![rows[1]]);
        assert_eq!(box_children(sidebar.groups[1].list), vec![rows[3]]);
    }

    fn assert_initial_reparent_layout(sidebar: &Sidebar, sessions: &[TermSession], rows: &[*mut GtkWidget], repo_s: &str) {
        assert_initial_folder_boxes(sidebar, rows, repo_s);
        assert_eq!(widget_icon_name(sessions[1].sidebar_row.icon), "utilities-terminal-symbolic");
        assert_eq!(widget_icon_name(sessions[1].top_tab.icon_widget()), "utilities-terminal-symbolic");
        assert!(widget_label_str(sessions[1].sidebar_row.detail).contains("⎇ main · +3 -1"));
        assert_eq!(widget_label_str(sessions[1].sidebar_row.metric), "MI 61.4 · B");
    }

    fn assert_mid_ssh_reparent(sidebar: &Sidebar, sessions: &mut [TermSession], rows: &[*mut GtkWidget]) {
        sessions[1].apply_proc_snapshot(ProcInfo { ssh_host: "ubuntu@prod.example.com".to_owned(), ..ProcInfo::default() });
        sessions[1].apply_vte_title(ptr::null_mut(), "ubuntu@prod.example.com: /srv/api");
        refresh_smart_title(&mut sessions[1]);
        sync_all_views(sidebar, sessions);
        assert_eq!(sidebar.folder_keys(), vec!["/tmp".to_owned()]);
        assert_eq!(box_children(sidebar.folder_widgets("/tmp").unwrap().3), vec![rows[0], rows[2]]);
        assert_eq!(box_children(sidebar.groups[1].list), vec![rows[1], rows[3]]);
        assert_eq!(unsafe { gtk_widget_get_parent(rows[1]) }, sidebar.groups[1].list);
        assert_eq!(widget_icon_name(sessions[1].sidebar_row.icon), "network-server-symbolic");
        assert_eq!(widget_label_str(sessions[1].sidebar_row.title), "ssh ubuntu@prod.example.com · /srv/api");
        assert_eq!(widget_label_str(sessions[1].sidebar_row.metric), "");
    }

    fn assert_tmux_split_detail(sidebar: &Sidebar, sessions: &mut [TermSession], repo_s: &str) {
        let split_vte = unsafe { gtk_label_new(ptr::null()) };
        sessions[1].panes.extra.push(crate::split_pane::ExtraPane { scroll: ptr::null_mut(), vte: split_vte, child_pid: 5678, cwd: repo_s.to_owned() });
        sessions[1].panes.record_focus(split_vte);
        sessions[1].apply_vte_title(split_vte, "AKHTERM_TMUX:dev|1|nvim|/srv/api");
        refresh_smart_title(&mut sessions[1]);
        sync_all_views(sidebar, sessions);
        assert_eq!(widget_label_str(sessions[1].sidebar_row.detail), "tmux:dev/1 · /srv/api");
        sessions[1].panes.extra.clear();
        sessions[1].panes.record_focus(ptr::null_mut());
        unsafe { gtk_widget_destroy(split_vte) };
    }

    fn assert_tmux_split_and_return(sidebar: &Sidebar, sessions: &mut [TermSession], rows: &[*mut GtkWidget], repo_s: &str) {
        assert_tmux_split_detail(sidebar, sessions, repo_s);
        sessions[1].apply_proc_snapshot(ProcInfo { cwd: repo_s.to_owned(), ..ProcInfo::default() });
        refresh_smart_title(&mut sessions[1]);
        sync_all_views(sidebar, sessions);
        assert_eq!(sidebar.folder_keys(), vec!["/tmp".to_owned(), repo_s.to_owned()]);
        let repo_rows = sidebar.folder_widgets(repo_s).unwrap().3;
        assert_eq!(box_children(repo_rows), vec![rows[1]]);
        assert_eq!(unsafe { gtk_widget_get_parent(rows[1]) }, repo_rows);
        assert_eq!(widget_icon_name(sessions[1].sidebar_row.icon), "utilities-terminal-symbolic");
        assert_eq!(widget_label_str(sessions[1].sidebar_row.title), "Проект · Новая");
        assert!(matches!(sessions[1].snapshot_tab(), Tab::Local { .. }));
    }

    fn setup_reparent_repo_and_ui() -> (std::path::PathBuf, String, *mut GtkWidget, TabStrip, Sidebar) {
        let repo = std::env::temp_dir().join(format!("akhterm_dynssh_{}", std::process::id()));
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let repo_s = repo.to_string_lossy().into_owned();
        let (win, root) = unsafe { (gtk_window_new(GTK_WINDOW_TOPLEVEL), gtk_box_new(GTK_ORIENTATION_VERTICAL, 0)) };
        unsafe { gtk_container_add(win, root) };
        let (strip, sidebar) = (TabStrip::new(root, noop_handler), Sidebar::new(root, noop_handler));
        sidebar.set_new_folder_tab_handler(noop_folder_tab);
        unsafe { gtk_widget_show_all(win) };
        (repo, repo_s, win, strip, sidebar)
    }

    #[test]
    fn headless_broadway_reparents_local_row_to_ssh_and_back_in_tab_order() {
        with_headless_gtk(|| {
            let (repo, repo_s, win, strip, sidebar) = setup_reparent_repo_and_ui();
            let mut sessions = vec![
                make_gtk_session(&sidebar, &strip, 1, "Первая", "/tmp", SessionCategory::Project, Tab::local("a")),
                make_gtk_session(&sidebar, &strip, 2, "Проект · Новая", &repo_s, SessionCategory::Project, Tab::local("b")),
                make_gtk_session(&sidebar, &strip, 3, "Третья", "/tmp", SessionCategory::Project, Tab::local("c")),
                make_gtk_session(&sidebar, &strip, 4, "ssh gpu-box", "/tmp", SessionCategory::Remote, Tab::remote("gpu-box", None)),
            ];
            let rows: Vec<*mut GtkWidget> = sessions.iter().map(|s| s.sidebar_row.widget).collect();
            sync_all_views(&sidebar, &sessions);
            assert_initial_reparent_layout(&sidebar, &sessions, &rows, &repo_s);
            assert_mid_ssh_reparent(&sidebar, &mut sessions, &rows);
            assert_tmux_split_and_return(&sidebar, &mut sessions, &rows, &repo_s);
            for s in sessions { s.sidebar_row.destroy(); s.top_tab.destroy(); }
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
            let _ = fs::remove_dir_all(&repo);
        });
    }
}
