//! Sidebar navigation for open tabs and discoverable tmux sessions.

use std::ffi::{c_void, CStr};
use std::ptr;

use crate::ffi::*;
use crate::paths::shorten_home;
use crate::session::{SessionCategory, TermSession, WorkState};
use crate::theme::set_agent_accent;
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

    pub fn set_visible(&self, visible: bool) {
        unsafe { gtk_widget_set_visible(self.panel, i32::from(visible)) };
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

    pub fn reveal_tmux_results(&self) {
        unsafe { gtk_expander_set_expanded(self.tmux, 1) };
    }

    pub fn filter<'a>(
        &self,
        query: &str,
        sessions: &[TermSession],
        tmux_rows: impl Iterator<Item = (&'a str, *mut GtkWidget)>,
    ) {
        let mut visible_groups = [false; 3];
        for session in sessions {
            let visible = query.is_empty() || search_text(session).contains(query);
            visible_groups[group_index(session.category)] |= visible;
            unsafe { gtk_widget_set_visible(session.sidebar_row.widget, i32::from(visible)) };
        }
        let mut tmux_count = 0;
        for (key, widget) in tmux_rows {
            let visible = key.contains(query);
            tmux_count += usize::from(visible);
            unsafe { gtk_widget_set_visible(widget, i32::from(visible)) };
        }

        set_label_text(self.tmux_heading, &format!("Сессии tmux · {tmux_count}"));
        unsafe {
            for (group, visible) in self.groups.iter().zip(visible_groups) {
                gtk_widget_set_visible(group.widget, i32::from(visible));
            }
            gtk_widget_set_visible(self.tmux, i32::from(tmux_count > 0));
            gtk_widget_set_visible(
                self.empty,
                i32::from(tmux_count == 0 && !visible_groups.contains(&true)),
            );
        }
        let message = if query.is_empty() {
            "Нет открытых вкладок"
        } else {
            "Ничего не найдено"
        };
        set_label_text(self.empty, message);
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

pub struct SidebarRow {
    widget: *mut GtkWidget,
    icon: *mut GtkWidget,
    title: *mut GtkWidget,
    detail: *mut GtkWidget,
    status: *mut GtkWidget,
    metric: *mut GtkWidget,
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
            }
        }
    }

    pub fn update(
        &self,
        index: usize,
        session: &TermSession,
        mi: Option<(f64, &str)>,
        active: bool,
    ) {
        set_agent_accent(self.icon, &session.proc_info.agent);
        set_class(self.widget, "sidebar-selected", active);
        let (status, style) = status_for(session);
        for class in ["sidebar-ready", "sidebar-working", "sidebar-ended"] {
            set_class(self.status, class, style == class);
        }
        set_label_text(self.status, status);
        set_label_text(self.title, &session.title);
        let detail = tab_detail(session);
        set_label_text(self.detail, &detail);
        let metric = mi.map_or(String::new(), |(score, grade)| {
            format!("MI {score:.1} · {grade}")
        });
        set_label_text(self.metric, &metric);
        let shortcut = if index < 9 {
            format!("\nAlt+{}", index + 1)
        } else {
            String::new()
        };
        set_tooltip(
            self.widget,
            &format!("{}\n{detail}{shortcut}", session.title),
        );
    }

    pub fn destroy(self) {
        unsafe { gtk_widget_destroy(self.widget) };
    }
}

fn status_for(session: &TermSession) -> (&'static str, &'static str) {
    match session.category {
        SessionCategory::Remote if session.child_pid == 0 => ("Нет связи", "sidebar-ended"),
        _ if session.child_pid == 0 => ("Завершена", "sidebar-ended"),
        _ if session.unread => ("Готово", "sidebar-ready"),
        SessionCategory::Daemon => ("Онлайн", "sidebar-working"),
        _ if session.state == WorkState::Working => ("В работе", "sidebar-working"),
        _ => ("", ""),
    }
}

fn tab_detail(session: &TermSession) -> String {
    if let Some((name, window, _, directory)) = &session.remote_tmux {
        return format!("tmux:{name}/{window} · {}", shorten_home(directory));
    }
    let directory = if session.proc_info.cwd.is_empty() {
        &session.cwd
    } else {
        &session.proc_info.cwd
    };
    if session.proc_info.has_tmux.is_empty() {
        shorten_home(directory)
    } else {
        format!("tmux · {}", shorten_home(directory))
    }
}

fn search_text(session: &TermSession) -> String {
    format!(
        "{} {} {} {} {}",
        session.title,
        session.cwd,
        session.proc_info.cwd,
        session.proc_info.ssh_host,
        session.proc_info.agent
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
