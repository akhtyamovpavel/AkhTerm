//! Sidebar navigation for open tabs and discoverable tmux sessions.

use std::cell::Cell;
use std::ffi::{c_int, c_void, CStr};
use std::path::Path;
use std::ptr;
use std::time::Instant;

use crate::ffi::*;
use crate::git_wt::{detect_git_head, GitHeadInfo};
use crate::mi_watch::resolve_project_root;
use crate::paths::{home_dir, shorten_home};
use crate::session::{SessionCategory, TermSession, WorkState};
use crate::telemetry::{
    format_turn_duration, format_working_sidebar_status, ContextAlert,
};
use crate::theme::set_agent_accent;
use crate::ui_text as text;
use crate::{connect_click, connect_entry_changed};

type WidgetHandler = extern "C" fn(*mut GtkWidget, *mut c_void);

pub struct Sidebar {
    panel: *mut GtkWidget,
    groups: [Group; 3],
    tmux: *mut GtkWidget,
    tmux_heading: *mut GtkWidget,
    tmux_list: *mut GtkWidget,
    empty: *mut GtkWidget,
}

impl Sidebar {
    pub fn new(parent: *mut GtkWidget, on_search: WidgetHandler) -> Self {
        unsafe {
            let panel = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
            gtk_widget_set_size_request(panel, 320, -1);
            add_class(panel, "sidebar");
            gtk_box_pack_start(parent, panel, 0, 1, 0);

            let header = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
            gtk_box_pack_start(header, label(c"Вкладки", "sidebar-title"), 1, 1, 0);
            gtk_box_pack_end(header, label(c"Alt 1–9", "sidebar-shortcut"), 0, 0, 0);
            gtk_box_pack_start(panel, header, 0, 1, 0);

            let search = gtk_search_entry_new();
            gtk_entry_set_placeholder_text(search, c"Вкладка, хост или сессия".as_ptr());
            connect_entry_changed(search, on_search);
            gtk_box_pack_start(panel, search, 0, 1, 0);

            let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
            let list = gtk_box_new(GTK_ORIENTATION_VERTICAL, 14);
            gtk_container_add(scroll, list);
            gtk_box_pack_start(panel, scroll, 1, 1, 0);

            let groups = [c"Локальные", c"SSH", c"Мониторинг"].map(|title| Group::new(list, title));
            let tmux = gtk_expander_new(ptr::null());
            add_class(tmux, "sidebar-disclosure");
            let tmux_heading = label(c"Сессии tmux", "sidebar-tmux-label");
            gtk_expander_set_label_widget(tmux, tmux_heading);
            gtk_widget_set_visible(tmux_heading, 1);
            let tmux_list = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
            gtk_container_add(tmux, tmux_list);
            gtk_widget_set_visible(tmux_list, 1);
            pack_hidden(list, tmux);

            let empty = label(c"Нет открытых вкладок", "sidebar-empty");
            pack_hidden(list, empty);
            Self {
                panel,
                groups,
                tmux,
                tmux_heading,
                tmux_list,
                empty,
            }
        }
    }

    pub fn panel_widget(&self) -> *mut GtkWidget {
        self.panel
    }

    pub fn append_tab(
        &self,
        category: SessionCategory,
        sid: usize,
        on_select: WidgetHandler,
    ) -> SidebarRow {
        let row = SidebarRow::new(category);
        connect_click(row.widget, on_select, sid as *mut c_void);
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
        unsafe {
            let button = gtk_button_new();
            add_class(button, "sidebar-action");
            let title_label = label(c"", "sidebar-action-label");
            set_label_text(title_label, title);
            gtk_container_add(button, title_label);
            set_tooltip(button, tooltip);
            connect_click(button, on_jump, index as *mut c_void);
            gtk_box_pack_start(self.tmux_list, button, 0, 1, 0);
            gtk_widget_show_all(button);
            button
        }
    }

    pub fn tmux_widget(&self) -> *mut GtkWidget {
        self.tmux
    }

    fn collect_group_ops(
        &self,
        sessions: &[TermSession],
    ) -> (Vec<(*mut GtkWidget, *mut GtkWidget)>, Vec<(*mut GtkWidget, Vec<*mut GtkWidget>)>) {
        let mut dirty = [false; 3];
        let mut moves = Vec::new();
        for session in sessions {
            let target = group_index(session.effective_category());
            let current = session.sidebar_row.group.get();
            if current != target {
                moves.push((session.sidebar_row.widget, self.groups[target].list));
                session.sidebar_row.group.set(target);
                dirty[current] = true;
                dirty[target] = true;
            }
        }
        let mut reorders = Vec::new();
        for (idx, changed) in dirty.into_iter().enumerate() {
            if changed && !self.groups[idx].list.is_null() {
                let order = sessions
                    .iter()
                    .filter(|s| s.sidebar_row.group.get() == idx && !s.sidebar_row.widget.is_null())
                    .map(|s| s.sidebar_row.widget)
                    .collect();
                reorders.push((self.groups[idx].list, order));
            }
        }
        (moves, reorders)
    }

    pub fn prepare_filter<'a>(
        &self,
        query: &str,
        sessions: &[TermSession],
        tmux_rows: impl Iterator<Item = (&'a str, *mut GtkWidget)>,
    ) -> SidebarUpdate {
        let (moves, reorders) = self.collect_group_ops(sessions);
        let mut visible_groups = [false; 3];
        let mut row_vis = Vec::with_capacity(sessions.len());
        for session in sessions {
            let visible = query.is_empty() || search_text(session).contains(query);
            visible_groups[group_index(session.effective_category())] |= visible;
            if !session.sidebar_row.widget.is_null() {
                row_vis.push((session.sidebar_row.widget, visible));
            }
        }
        let mut tmux_count = 0;
        let mut tmux_vis = Vec::new();
        for (key, widget) in tmux_rows {
            let visible = key.contains(query);
            tmux_count += usize::from(visible);
            tmux_vis.push((widget, visible));
        }
        let empty_msg = if query.is_empty() { "Нет открытых вкладок" } else { "Ничего не найдено" };
        SidebarUpdate {
            moves,
            reorders,
            row_vis,
            tmux_vis,
            group_vis: [
                (self.groups[0].widget, visible_groups[0]),
                (self.groups[1].widget, visible_groups[1]),
                (self.groups[2].widget, visible_groups[2]),
            ],
            tmux: self.tmux,
            tmux_heading: self.tmux_heading,
            tmux_count,
            empty: self.empty,
            show_empty: tmux_count == 0 && !visible_groups.contains(&true),
            empty_msg,
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

pub struct SidebarUpdate {
    moves: Vec<(*mut GtkWidget, *mut GtkWidget)>,
    reorders: Vec<(*mut GtkWidget, Vec<*mut GtkWidget>)>,
    row_vis: Vec<(*mut GtkWidget, bool)>,
    tmux_vis: Vec<(*mut GtkWidget, bool)>,
    group_vis: [(*mut GtkWidget, bool); 3],
    tmux: *mut GtkWidget,
    tmux_heading: *mut GtkWidget,
    tmux_count: usize,
    empty: *mut GtkWidget,
    show_empty: bool,
    empty_msg: &'static str,
}

impl SidebarUpdate {
    pub fn apply(self) {
        for (widget, to_list) in self.moves {
            reparent_row(widget, to_list);
        }
        for (list, order) in self.reorders {
            for (pos, widget) in order.into_iter().enumerate() {
                unsafe { gtk_box_reorder_child(list, widget, pos as c_int) };
            }
        }
        unsafe {
            for (widget, vis) in self.row_vis {
                gtk_widget_set_visible(widget, i32::from(vis));
            }
            for (widget, vis) in self.tmux_vis {
                gtk_widget_set_visible(widget, i32::from(vis));
            }
            for (group, vis) in self.group_vis {
                gtk_widget_set_visible(group, i32::from(vis));
            }
            gtk_widget_set_visible(self.tmux, i32::from(self.tmux_count > 0));
            gtk_widget_set_visible(self.empty, i32::from(self.show_empty));
        }
        set_label_text(self.tmux_heading, &format!("Сессии tmux · {}", self.tmux_count));
        set_label_text(self.empty, self.empty_msg);
    }
}

struct Group {
    widget: *mut GtkWidget,
    list: *mut GtkWidget,
}

impl Group {
    fn new(parent: *mut GtkWidget, title: &CStr) -> Self {
        unsafe {
            let widget = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
            let list = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
            gtk_box_pack_start(widget, label(title, "sidebar-group-label"), 0, 1, 0);
            gtk_box_pack_start(widget, list, 0, 1, 0);
            gtk_widget_show_all(widget);
            pack_hidden(parent, widget);
            Self { widget, list }
        }
    }
}

fn group_index(category: SessionCategory) -> usize {
    match category {
        SessionCategory::Project => 0,
        SessionCategory::Remote => 1,
        SessionCategory::Daemon => 2,
    }
}

fn reparent_row(widget: *mut GtkWidget, to_list: *mut GtkWidget) {
    if widget.is_null() || to_list.is_null() {
        return;
    }
    unsafe {
        let parent = gtk_widget_get_parent(widget);
        if parent == to_list {
            return;
        }
        g_object_ref(widget);
        if !parent.is_null() {
            gtk_container_remove(parent, widget);
        }
        gtk_box_pack_start(to_list, widget, 0, 1, 0);
        g_object_unref(widget);
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

impl SidebarRow {
    fn new(category: SessionCategory) -> Self {
        unsafe {
            let widget = gtk_button_new();
            add_class(widget, "sidebar-row");
            let content = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 9);
            gtk_container_add(widget, content);
            let icon = gtk_image_new_from_icon_name(category.icon_name().as_ptr(), GTK_ICON_SIZE_MENU);
            add_class(icon, "sidebar-icon");
            gtk_widget_set_valign(icon, GTK_ALIGN_START);
            gtk_box_pack_start(content, icon, 0, 0, 0);

            let title = label(c"", "sidebar-row-title");
            let detail = label(c"", "sidebar-detail");
            gtk_label_set_ellipsize(detail, PANGO_ELLIPSIZE_MIDDLE);
            let status = label(c"", "sidebar-status");
            let metric = label(c"", "sidebar-metric");
            let lines = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
            pack_line(lines, title, status);
            pack_line(lines, detail, metric);
            gtk_box_pack_start(content, lines, 1, 1, 0);
            Self {
                widget,
                icon,
                title,
                detail,
                status,
                metric,
                group: Cell::new(group_index(category)),
                icon_cat: Cell::new(category),
            }
        }
    }

    fn take_icon_change(&self, category: SessionCategory) -> Option<&'static CStr> {
        (self.icon_cat.get() != category).then(|| {
            self.icon_cat.set(category);
            category.icon_name()
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
        let now = Instant::now();
        let eff_cat = session.effective_category();
        let (status, style) = status_for(session);
        let live_status = (style == "sidebar-working" && session.category != SessionCategory::Daemon)
            .then(|| session.live_turn_secs(now))
            .flatten()
            .map(format_working_sidebar_status);
        let status_text = live_status
            .as_deref()
            .map_or_else(|| status.to_string_lossy(), std::borrow::Cow::Borrowed);
        let tip_status = format_settled_status(&status_text, session.settled_turn_secs());
        let detail = tab_detail(session, git_dirty);
        let snap = session.telemetry.snapshot();
        let ctx_badge = snap.compact_context_badge();
        let eff_mi = (eff_cat != SessionCategory::Remote).then_some(mi).flatten();
        let mi_str = eff_mi.map_or(String::new(), |(score, grade)| format!("MI {score:.1} · {grade}"));
        let metric = combine_metric_parts(ctx_badge.as_deref(), &mi_str);
        let mi_tip = format_metric_tooltip(&mi_str, local_project_root(session).as_deref());
        let metric_tip = combine_metric_parts(ctx_badge.as_deref(), &mi_tip);
        let tip = format_row_tooltip(&session.title, &tip_status, &detail, &metric_tip, index);
        RowUpdate {
            widget: self.widget,
            icon: self.icon,
            title_lbl: self.title,
            detail_lbl: self.detail,
            status_lbl: self.status,
            metric_lbl: self.metric,
            new_icon: self.take_icon_change(eff_cat),
            agent: session.proc_info.agent.clone(),
            active,
            status,
            live_status,
            style,
            title: session.title.clone(),
            detail,
            metric,
            alert: snap.alert_level(),
            tip,
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

fn format_row_tooltip(
    title: &str,
    status: &str,
    detail: &str,
    metric: &str,
    index: usize,
) -> String {
    let head = if status.is_empty() { title.to_owned() } else { format!("{title} · {status}") };
    let mut out = format!("{head}\n{detail}");
    if !metric.is_empty() {
        out.push('\n');
        out.push_str(metric);
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

fn apply_row_status(
    status_label: *mut GtkWidget,
    status: &'static CStr,
    live_status: Option<&str>,
    style: &'static str,
) {
    for class in ["sidebar-approval", "sidebar-ready", "sidebar-working", "sidebar-ended"] {
        set_class(status_label, class, style == class);
    }
    if let Some(live) = live_status {
        set_label_text(status_label, live);
    } else {
        unsafe { gtk_label_set_text(status_label, status.as_ptr()) };
    }
}

pub struct RowUpdate {
    widget: *mut GtkWidget,
    icon: *mut GtkWidget,
    title_lbl: *mut GtkWidget,
    detail_lbl: *mut GtkWidget,
    status_lbl: *mut GtkWidget,
    metric_lbl: *mut GtkWidget,
    new_icon: Option<&'static CStr>,
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
    pub fn apply(self) {
        if let Some(icon_name) = self.new_icon {
            set_image_icon(self.icon, icon_name);
        }
        set_agent_accent(self.icon, &self.agent);
        set_class(self.widget, "sidebar-selected", self.active);
        apply_row_status(self.status_lbl, self.status, self.live_status.as_deref(), self.style);
        set_label_text(self.title_lbl, &self.title);
        set_label_text(self.detail_lbl, &self.detail);
        set_class(self.metric_lbl, "sidebar-metric-warn", self.alert == ContextAlert::Warn);
        set_class(self.metric_lbl, "sidebar-metric-high", self.alert == ContextAlert::High);
        set_label_text(self.metric_lbl, &self.metric);
        set_tooltip(self.widget, &self.tip);
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

pub fn tab_detail(session: &TermSession, git_dirty: &str) -> String {
    if let Some((name, window, _, directory)) = &session.remote_tmux {
        return format!("tmux:{name}/{window} · {}", shorten_home(directory));
    }
    if session.effective_category() == SessionCategory::Remote {
        let host = session
            .remote()
            .map(|r| r.host.as_str())
            .filter(|h| !h.is_empty())
            .unwrap_or(session.proc_info.ssh_host.as_str());
        return format_remote_detail(host, &session.osc_title);
    }
    let directory = fallback_session_dir(session);
    let git = local_git_head(session);
    format_local_detail(&directory, !session.proc_info.has_tmux.is_empty(), git.as_ref(), git_dirty)
}

fn search_text(session: &TermSession) -> String {
    let live_cwd = session.active_local_cwd().unwrap_or_default();
    let branch = local_git_head(session).map_or_else(String::new, |git| git.badge());
    let root = local_project_root(session).unwrap_or_default();
    let status = status_for(session).0.to_string_lossy();
    format!(
        "{} {} {} {} {} {} {} {} {} {}",
        session.title,
        session.osc_title,
        session.cwd,
        session.proc_info.cwd,
        live_cwd,
        session.proc_info.ssh_host,
        session.proc_info.agent,
        branch,
        root,
        status
    )
    .to_lowercase()
}

unsafe fn label(title: &CStr, class: &str) -> *mut GtkWidget {
    let widget = gtk_label_new(title.as_ptr());
    gtk_label_set_xalign(widget, 0.0);
    gtk_label_set_ellipsize(widget, PANGO_ELLIPSIZE_END);
    add_class(widget, class);
    widget
}

unsafe fn pack_line(parent: *mut GtkWidget, leading: *mut GtkWidget, trailing: *mut GtkWidget) {
    let line = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    gtk_box_pack_start(line, leading, 1, 1, 0);
    gtk_box_pack_end(line, trailing, 0, 0, 0);
    gtk_box_pack_start(parent, line, 0, 1, 0);
}

unsafe fn pack_hidden(parent: *mut GtkWidget, child: *mut GtkWidget) {
    gtk_widget_set_no_show_all(child, 1);
    gtk_widget_set_visible(child, 0);
    gtk_box_pack_start(parent, child, 0, 1, 0);
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

    fn box_children(container: *mut GtkWidget) -> Vec<*mut GtkWidget> {
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

    fn widget_label_str(lbl: *mut GtkWidget) -> String {
        unsafe {
            let ptr = gtk_label_get_text(lbl);
            if ptr.is_null() { String::new() } else { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
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
        let now = Instant::now();
        let sidebar_row = sidebar.append_tab(category, sid, noop_handler);
        let top_tab = strip.append(sid, category, noop_handler, noop_handler);
        let mut proc_info = ProcInfo::default();
        if let Tab::Remote(ref r) = tab {
            proc_info.ssh_host.clone_from(&r.host);
        }
        TermSession {
            id: sid,
            base_title: title.to_owned(),
            profile_cwd: cwd.to_owned(),
            title: title.to_owned(),
            osc_title: String::new(),
            custom_title: false,
            cwd: cwd.to_owned(),
            category,
            vte: ptr::null_mut(),
            scroll_wrap: ptr::null_mut(),
            panes: TabPanes { root: ptr::null_mut(), extra: Vec::new(), last_focus: ptr::null_mut() },
            sidebar_row,
            top_tab,
            child_pid: 1234,
            state: WorkState::Idle,
            unread: false,
            last_output: now,
            work_started: now,
            work_started_at: None,
            last_turn_secs: None,
            last_notified: None,
            proc_info,
            tab,
            remote_tmux: None,
            tmux_title_from_vte: false,
            telemetry: crate::telemetry::SessionTelemetry::default(),
        }
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

    #[test]
    fn formats_sidebar_tooltip_with_status_branch_and_mi_root() {
        let m = format_metric_tooltip("MI 61.2 · B", Some("~/proj"));
        let tip = format_row_tooltip("AkhTerm", "Ждёт апрува", "~/proj/src · ⎇ main", &m, 0);
        assert_eq!(
            tip,
            "AkhTerm · Ждёт апрува\n~/proj/src · ⎇ main\nMI 61.2 · B (~/proj)\nAlt+1"
        );
        let settled = format_settled_status("Готово", Some(134));
        let full_m = combine_metric_parts(Some("ctx 84k"), &m);
        let tip2 = format_row_tooltip("AkhTerm", &settled, "~/proj · ⎇ main", &full_m, 1);
        assert_eq!(
            tip2,
            "AkhTerm · Готово · ⏱ 2m 14s\n~/proj · ⎇ main\nctx 84k · MI 61.2 · B (~/proj)\nAlt+2"
        );
    }

    #[test]
    fn headless_broadway_reparents_local_row_to_ssh_and_back_in_tab_order() {
        with_headless_gtk(|| {
            let repo = std::env::temp_dir().join(format!("akhterm_dynssh_{}", std::process::id()));
            let _ = fs::remove_dir_all(&repo);
            fs::create_dir_all(repo.join(".git")).unwrap();
            fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
            let repo_s = repo.to_string_lossy().into_owned();

            let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
            let root = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 0) };
            unsafe { gtk_container_add(win, root) };
            let strip = TabStrip::new(root, noop_handler);
            let sidebar = Sidebar::new(root, noop_handler);
            unsafe { gtk_widget_show_all(win) };

            let mut sessions = vec![
                make_gtk_session(&sidebar, &strip, 1, "Первая", "/tmp", SessionCategory::Project, Tab::local("a")),
                make_gtk_session(&sidebar, &strip, 2, "Проект · Новая", &repo_s, SessionCategory::Project, Tab::local("b")),
                make_gtk_session(&sidebar, &strip, 3, "Третья", "/tmp", SessionCategory::Project, Tab::local("c")),
                make_gtk_session(&sidebar, &strip, 4, "ssh gpu-box", "/tmp", SessionCategory::Remote, Tab::remote("gpu-box", None)),
            ];
            let rows: Vec<*mut GtkWidget> = sessions.iter().map(|s| s.sidebar_row.widget).collect();

            sync_all_views(&sidebar, &sessions);
            assert_eq!(box_children(sidebar.groups[0].list), vec![rows[0], rows[1], rows[2]]);
            assert_eq!(box_children(sidebar.groups[1].list), vec![rows[3]]);
            assert_eq!(widget_icon_name(sessions[1].sidebar_row.icon), "utilities-terminal-symbolic");
            assert_eq!(widget_icon_name(sessions[1].top_tab.icon_widget()), "utilities-terminal-symbolic");
            assert!(widget_label_str(sessions[1].sidebar_row.detail).contains("⎇ main · +3 -1"));
            assert_eq!(widget_label_str(sessions[1].sidebar_row.metric), "MI 61.4 · B");

            sessions[1].apply_proc_snapshot(ProcInfo {
                ssh_host: "ubuntu@prod.example.com".to_owned(),
                ..ProcInfo::default()
            });
            sessions[1].apply_vte_title(ptr::null_mut(), "ubuntu@prod.example.com: /srv/api");
            refresh_smart_title(&mut sessions[1]);
            sync_all_views(&sidebar, &sessions);

            assert_eq!(box_children(sidebar.groups[0].list), vec![rows[0], rows[2]]);
            assert_eq!(box_children(sidebar.groups[1].list), vec![rows[1], rows[3]]);
            assert_eq!(unsafe { gtk_widget_get_parent(rows[1]) }, sidebar.groups[1].list);
            assert_eq!(widget_icon_name(sessions[1].sidebar_row.icon), "network-server-symbolic");
            assert_eq!(widget_icon_name(sessions[1].top_tab.icon_widget()), "network-server-symbolic");
            assert_eq!(widget_label_str(sessions[1].sidebar_row.title), "ssh ubuntu@prod.example.com · /srv/api");
            assert_eq!(widget_label_str(sessions[1].sidebar_row.detail), "ubuntu@prod.example.com · /srv/api");
            assert_eq!(widget_label_str(sessions[1].sidebar_row.metric), "");

            let split_vte = unsafe { gtk_label_new(ptr::null()) };
            sessions[1].panes.extra.push(crate::split_pane::ExtraPane {
                scroll: ptr::null_mut(),
                vte: split_vte,
                child_pid: 5678,
                cwd: repo_s.clone(),
            });
            sessions[1].panes.record_focus(split_vte);
            sessions[1].apply_vte_title(split_vte, "AKHTERM_TMUX:dev|1|nvim|/srv/api");
            refresh_smart_title(&mut sessions[1]);
            sync_all_views(&sidebar, &sessions);
            assert_eq!(widget_label_str(sessions[1].sidebar_row.detail), "tmux:dev/1 · /srv/api");
            sessions[1].panes.extra.clear();
            sessions[1].panes.record_focus(ptr::null_mut());
            unsafe { gtk_widget_destroy(split_vte) };

            sessions[1].apply_proc_snapshot(ProcInfo { cwd: repo_s.clone(), ..ProcInfo::default() });
            refresh_smart_title(&mut sessions[1]);
            sync_all_views(&sidebar, &sessions);

            assert_eq!(box_children(sidebar.groups[0].list), vec![rows[0], rows[1], rows[2]]);
            assert_eq!(box_children(sidebar.groups[1].list), vec![rows[3]]);
            assert_eq!(unsafe { gtk_widget_get_parent(rows[1]) }, sidebar.groups[0].list);
            assert_eq!(widget_icon_name(sessions[1].sidebar_row.icon), "utilities-terminal-symbolic");
            assert_eq!(widget_icon_name(sessions[1].top_tab.icon_widget()), "utilities-terminal-symbolic");
            assert_eq!(widget_label_str(sessions[1].sidebar_row.title), "Проект · Новая");
            assert!(widget_label_str(sessions[1].sidebar_row.detail).contains("⎇ main · +3 -1"));
            assert_eq!(widget_label_str(sessions[1].sidebar_row.metric), "MI 61.4 · B");
            assert!(matches!(sessions[1].snapshot_tab(), Tab::Local { .. }));

            for s in sessions {
                s.sidebar_row.destroy();
                s.top_tab.destroy();
            }
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
            let _ = fs::remove_dir_all(&repo);
        });
    }
}
