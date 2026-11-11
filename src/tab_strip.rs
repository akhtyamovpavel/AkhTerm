//! GTK widgets for the scrollable terminal tab strip.

use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_void, CStr, CString};
use std::ptr;
use std::time::Instant;

use crate::connect_click;
use crate::ffi::*;
use crate::session::{SessionCategory, TermSession};
use crate::tab_order::BoxReorder;
use crate::telemetry::{format_turn_duration, format_working_badge};
use crate::theme::set_agent_accent;
use crate::ui_text as text;

pub type ClickHandler = extern "C" fn(*mut GtkWidget, *mut c_void);
pub type EventHandler = extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int;

#[derive(Clone, Copy)]
pub struct TabHandlers {
    pub on_select: ClickHandler,
    pub on_close: ClickHandler,
    pub on_press: EventHandler,
    pub on_motion: EventHandler,
    pub on_release: EventHandler,
}

#[derive(Clone, Copy)]
pub struct TabStrip {
    tabs: *mut GtkWidget,
    adjustment: *mut GtkAdjustment,
}

impl TabStrip {
    pub fn new(parent: *mut GtkWidget, on_new_tab: ClickHandler) -> Self {
        unsafe {
            let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
            add_class(row, "tab-strip");
            gtk_box_pack_start(parent, row, 0, 1, 0);

            let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            gtk_scrolled_window_set_policy(scroll, GTK_POLICY_AUTOMATIC, GTK_POLICY_NEVER);
            let viewport = gtk_viewport_new(ptr::null_mut(), ptr::null_mut());
            gtk_scrollable_set_hscroll_policy(viewport, GTK_SCROLL_NATURAL);
            let tabs = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
            gtk_container_add(viewport, tabs);
            gtk_container_add(scroll, viewport);
            gtk_box_pack_start(row, scroll, 1, 1, 0);
            let adjustment = gtk_scrolled_window_get_hadjustment(scroll);
            gtk_container_set_focus_hadjustment(tabs, adjustment);

            let new_tab = gtk_button_new_with_label(text::NEW_TAB.as_ptr());
            add_class(new_tab, "new-tab");
            gtk_widget_set_valign(new_tab, GTK_ALIGN_CENTER);
            gtk_widget_set_tooltip_text(new_tab, text::NEW_TAB_TIP.as_ptr());
            connect_click(new_tab, on_new_tab, ptr::null_mut());
            gtk_box_pack_start(row, new_tab, 0, 0, 0);

            Self { tabs, adjustment }
        }
    }

    pub fn append(&self, sid: usize, category: SessionCategory, handlers: TabHandlers) -> TabView {
        let tab = TabView::new(sid, category, handlers);
        unsafe {
            gtk_box_pack_start(self.tabs, tab.widget, 0, 0, 0);
            gtk_widget_show_all(tab.widget);
        }
        tab
    }

    pub fn reorder_op(&self, tab: &TabView, position: usize) -> BoxReorder {
        BoxReorder {
            container: self.tabs,
            child: tab.widget,
            position: position as c_int,
        }
    }

    pub fn tabs_box(&self) -> *mut GtkWidget {
        self.tabs
    }

    pub fn reveal_widget(&self, tab_widget: *mut GtkWidget) {
        let Some(x) = crate::platform::translate_widget_x_to_ancestor(tab_widget, self.tabs, 0.0) else {
            return;
        };
        unsafe {
            let width = f64::from(gtk_widget_get_allocated_width(tab_widget));
            gtk_adjustment_clamp_page(self.adjustment, x, x + width);
        }
    }
}

pub struct TabView {
    widget: *mut GtkWidget,
    select: *mut GtkWidget,
    icon: *mut GtkWidget,
    label: *mut GtkWidget,
    status: *mut GtkWidget,
    icon_cat: Cell<SessionCategory>,
    last_tip: RefCell<String>,
}

fn wire_tab_drag_events(select: *mut GtkWidget, data: *mut c_void, handlers: TabHandlers) {
    let mask = GDK_BUTTON_PRESS_MASK | GDK_BUTTON_RELEASE_MASK | GDK_BUTTON1_MOTION_MASK;
    unsafe {
        gtk_widget_add_events(select, mask);
        g_signal_connect_data(
            select,
            c"button-press-event".as_ptr(),
            handlers.on_press as *const c_void,
            data,
            ptr::null(),
            0,
        );
        g_signal_connect_data(
            select,
            c"motion-notify-event".as_ptr(),
            handlers.on_motion as *const c_void,
            data,
            ptr::null(),
            0,
        );
        g_signal_connect_data(
            select,
            c"button-release-event".as_ptr(),
            handlers.on_release as *const c_void,
            data,
            ptr::null(),
            0,
        );
    }
}

impl TabView {
    fn new(sid: usize, category: SessionCategory, handlers: TabHandlers) -> Self {
        let data = sid as *mut c_void;
        unsafe {
            let widget = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
            add_class(widget, "terminal-tab");

            let select = gtk_button_new();
            add_class(select, "tab-select");
            let content = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 7);
            let icon_name = crate::platform::resolve_category_icon(category);
            let icon = gtk_image_new_from_icon_name(icon_name.as_ptr(), GTK_ICON_SIZE_MENU);
            add_class(icon, "tab-icon");
            gtk_widget_set_valign(icon, GTK_ALIGN_CENTER);
            gtk_box_pack_start(content, icon, 0, 0, 0);
            let label = create_title_label();
            gtk_box_pack_start(content, label, 1, 1, 0);
            let status = create_status_badge();
            gtk_box_pack_start(content, status, 0, 0, 0);
            gtk_container_add(select, content);
            connect_click(select, handlers.on_select, data);
            wire_tab_drag_events(select, data, handlers);
            gtk_box_pack_start(widget, select, 1, 1, 0);

            let close = create_close_button(data, handlers.on_close);
            gtk_box_pack_start(widget, close, 0, 0, 0);

            Self {
                widget,
                select,
                icon,
                label,
                status,
                icon_cat: Cell::new(category),
                last_tip: RefCell::new(String::new()),
            }
        }
    }

    pub fn select_button(&self) -> *mut GtkWidget {
        self.select
    }

    pub fn tab_widget(&self) -> *mut GtkWidget {
        self.widget
    }

    fn take_icon_change(&self, category: SessionCategory) -> Option<SessionCategory> {
        (self.icon_cat.get() != category).then(|| {
            self.icon_cat.set(category);
            category
        })
    }

    fn take_tooltip_change(&self, tip: &str) -> Option<String> {
        let mut prev = self.last_tip.try_borrow_mut().ok()?;
        if prev.as_str() == tip {
            None
        } else {
            prev.clear();
            prev.push_str(tip);
            Some(tip.to_owned())
        }
    }

    pub fn prepare_update(&self, session: &TermSession, active: bool) -> TabUpdate {
        let now = Instant::now();
        let needs_approval = session.state.needs_approval();
        let working_secs = session.live_turn_secs(now);
        let tip = tooltip_for_session(session, needs_approval, working_secs);
        TabUpdate {
            widget: self.widget,
            select: self.select,
            icon: self.icon,
            label: self.label,
            status: self.status,
            new_icon: self.take_icon_change(session.effective_category()),
            new_tip: self.take_tooltip_change(&tip),
            title: session.title.clone(),
            agent: session.proc_info.agent.clone(),
            active,
            unread: session.unread,
            needs_approval,
            working_secs,
        }
    }

    #[cfg(test)]
    pub fn update(&self, session: &TermSession, active: bool) {
        self.prepare_update(session, active).apply();
    }

    pub fn destroy(self) {
        unsafe { gtk_widget_destroy(self.widget) };
    }

    #[cfg(test)]
    pub fn empty_for_test(category: SessionCategory) -> Self {
        Self {
            widget: ptr::null_mut(),
            select: ptr::null_mut(),
            icon: ptr::null_mut(),
            label: ptr::null_mut(),
            status: ptr::null_mut(),
            icon_cat: Cell::new(category),
            last_tip: RefCell::new(String::new()),
        }
    }

    #[cfg(test)]
    pub fn icon_widget(&self) -> *mut GtkWidget {
        self.icon
    }
}

pub struct TabUpdate {
    widget: *mut GtkWidget,
    select: *mut GtkWidget,
    icon: *mut GtkWidget,
    label: *mut GtkWidget,
    status: *mut GtkWidget,
    new_icon: Option<SessionCategory>,
    new_tip: Option<String>,
    title: String,
    agent: String,
    active: bool,
    unread: bool,
    needs_approval: bool,
    working_secs: Option<u64>,
}

impl TabUpdate {
    pub fn apply(self) {
        if let Some(cat) = self.new_icon.filter(|_| !self.icon.is_null()) {
            set_image_icon(self.icon, crate::platform::resolve_category_icon(cat));
        }
        set_agent_accent(self.icon, &self.agent);
        set_class(self.widget, "tab-active", self.active);
        set_class(self.widget, "tab-approval", self.needs_approval);
        set_class(self.widget, "tab-ready", self.unread && !self.active && !self.needs_approval);
        let _ = sync_status_badge(self.status, self.needs_approval, self.working_secs);
        sync_tab_title(self.label, &self.title);
        if let Some(tip) = &self.new_tip {
            set_tooltip(self.select, tip);
        }
    }
}

fn create_title_label() -> *mut GtkWidget {
    unsafe {
        let label = gtk_label_new(ptr::null());
        gtk_label_set_xalign(label, 0.0);
        gtk_label_set_ellipsize(label, PANGO_ELLIPSIZE_END);
        gtk_label_set_max_width_chars(label, 22);
        label
    }
}

fn create_status_badge() -> *mut GtkWidget {
    unsafe {
        let status = gtk_label_new(text::STATUS_APPROVAL.as_ptr());
        add_class(status, "tab-status");
        gtk_widget_set_no_show_all(status, 1);
        gtk_widget_set_visible(status, 0);
        status
    }
}

fn create_close_button(data: *mut c_void, on_close: ClickHandler) -> *mut GtkWidget {
    unsafe {
        let close = gtk_button_new_with_label(text::CLOSE_TAB.as_ptr());
        add_class(close, "tab-close");
        gtk_widget_set_tooltip_text(close, text::CLOSE_TAB_TIP.as_ptr());
        connect_click(close, on_close, data);
        close
    }
}

fn sync_status_badge(
    status: *mut GtkWidget,
    needs_approval: bool,
    working_secs: Option<u64>,
) -> bool {
    let show = needs_approval || working_secs.is_some();
    set_class(status, "tab-timer", !needs_approval && working_secs.is_some());
    let text_changed = if needs_approval {
        sync_label_cstr(status, text::STATUS_APPROVAL)
    } else if let Some(secs) = working_secs {
        let badge = format_working_badge(secs);
        CString::new(badge).ok().is_some_and(|c| sync_label_cstr(status, &c))
    } else {
        false
    };
    let vis_now = i32::from(show);
    let vis_changed = unsafe { gtk_widget_get_visible(status) } != vis_now;
    unsafe { gtk_widget_set_visible(status, vis_now) };
    vis_changed || text_changed
}

fn sync_label_cstr(label: *mut GtkWidget, next: &CStr) -> bool {
    let cur_ptr = unsafe { gtk_label_get_text(label) };
    let changed = cur_ptr.is_null() || unsafe { CStr::from_ptr(cur_ptr) != next };
    if changed {
        unsafe { gtk_label_set_text(label, next.as_ptr()) };
    }
    changed
}

fn tooltip_for_session(
    session: &TermSession,
    needs_approval: bool,
    working_secs: Option<u64>,
) -> String {
    let ctx_badge = session.telemetry.snapshot().compact_context_badge();
    format_tab_tooltip(
        &session.title,
        needs_approval,
        working_secs.or_else(|| session.settled_turn_secs()),
        ctx_badge.as_deref(),
    )
}

fn format_tab_tooltip(
    title: &str,
    needs_approval: bool,
    turn_secs: Option<u64>,
    ctx_badge: Option<&str>,
) -> String {
    let mut out = if needs_approval {
        format!("{title} — {}", text::APPROVAL_TAB_TIP.to_string_lossy())
    } else {
        title.to_owned()
    };
    if let Some(secs) = turn_secs {
        out.push_str(&format!(" · ⏱ {}", format_turn_duration(secs)));
    }
    if let Some(ctx) = ctx_badge {
        out.push_str(&format!(" · {ctx}"));
    }
    out
}

fn sync_tab_title(label: *mut GtkWidget, title: &str) {
    if let Ok(title_c) = CString::new(title) {
        let _ = sync_label_cstr(label, &title_c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_tab_tooltip_with_turn_timer_and_context() {
        assert_eq!(format_tab_tooltip("AkhTerm", false, None, None), "AkhTerm");
        assert_eq!(
            format_tab_tooltip("AkhTerm · AGY", false, Some(134), Some("ctx 84k")),
            "AkhTerm · AGY · ⏱ 2m 14s · ctx 84k"
        );
        let tip = format_tab_tooltip("AkhTerm", true, Some(42), Some("ctx 142k"));
        assert!(tip.contains("Ждёт апрува") && tip.ends_with("· ⏱ 42s · ctx 142k"));
    }
}
