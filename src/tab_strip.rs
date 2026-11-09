//! GTK widgets for the scrollable terminal tab strip.

use std::ffi::{c_void, CStr, CString};
use std::ptr;

use crate::connect_click;
use crate::ffi::*;
use crate::session::SessionCategory;
use crate::theme::set_agent_accent;
use crate::ui_text as text;

type ClickHandler = extern "C" fn(*mut GtkWidget, *mut c_void);

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

    pub fn append(
        &self,
        sid: usize,
        category: SessionCategory,
        on_select: ClickHandler,
        on_close: ClickHandler,
    ) -> TabView {
        let tab = TabView::new(sid, category, on_select, on_close);
        unsafe {
            gtk_box_pack_start(self.tabs, tab.widget, 0, 0, 0);
            gtk_widget_show_all(tab.widget);
        }
        tab
    }

    pub fn reveal(&self, tab: &TabView) {
        let mut x = 0;
        unsafe {
            if gtk_widget_translate_coordinates(
                tab.widget,
                self.tabs,
                0,
                0,
                &mut x,
                ptr::null_mut(),
            ) != 0
            {
                let width = gtk_widget_get_allocated_width(tab.widget);
                gtk_adjustment_clamp_page(self.adjustment, f64::from(x), f64::from(x + width));
            }
        }
    }
}

pub struct TabView {
    widget: *mut GtkWidget,
    select: *mut GtkWidget,
    icon: *mut GtkWidget,
    label: *mut GtkWidget,
}

impl TabView {
    fn new(
        sid: usize,
        category: SessionCategory,
        on_select: ClickHandler,
        on_close: ClickHandler,
    ) -> Self {
        let data = sid as *mut c_void;
        unsafe {
            let widget = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
            add_class(widget, "terminal-tab");

            let select = gtk_button_new();
            add_class(select, "tab-select");
            let content = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 7);
            let icon = gtk_image_new_from_icon_name(category.icon_name().as_ptr(), GTK_ICON_SIZE_MENU);
            add_class(icon, "tab-icon");
            gtk_box_pack_start(content, icon, 0, 0, 0);
            let label = gtk_label_new(ptr::null());
            gtk_label_set_xalign(label, 0.0);
            gtk_label_set_ellipsize(label, PANGO_ELLIPSIZE_END);
            gtk_label_set_max_width_chars(label, 22);
            gtk_box_pack_start(content, label, 1, 1, 0);
            gtk_container_add(select, content);
            connect_click(select, on_select, data);
            gtk_box_pack_start(widget, select, 1, 1, 0);

            let close = gtk_button_new_with_label(text::CLOSE_TAB.as_ptr());
            add_class(close, "tab-close");
            gtk_widget_set_tooltip_text(close, text::CLOSE_TAB_TIP.as_ptr());
            connect_click(close, on_close, data);
            gtk_box_pack_start(widget, close, 0, 0, 0);

            Self {
                widget,
                select,
                icon,
                label,
            }
        }
    }

    pub fn update(&self, title: &str, agent: &str, active: bool, unread: bool) {
        set_agent_accent(self.icon, agent);
        set_class(self.widget, "tab-active", active);
        set_class(self.widget, "tab-ready", unread && !active);
        if let Ok(title) = CString::new(title) {
            unsafe {
                if CStr::from_ptr(gtk_label_get_text(self.label)) != title.as_c_str() {
                    gtk_label_set_text(self.label, title.as_ptr());
                    gtk_widget_set_tooltip_text(self.select, title.as_ptr());
                }
            }
        }
    }

    pub fn destroy(self) {
        unsafe { gtk_widget_destroy(self.widget) };
    }
}
