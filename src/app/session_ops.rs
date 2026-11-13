//! Session lifecycle, VTE callbacks, split panes, tab ordering, and persistence.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::{path::PathBuf, ptr, time::Instant};

use crate::app_state::{self, AppState, APP};
use crate::ffi::*;
use crate::session::{LinkCwd, SessionCategory, TermSession, WorkState};
use crate::tabs::{is_ssh_host, RemoteConnection, SplitLeaf, SplitTree, Tab, TabMeta, TabStore, TabsState};
use crate::{approval, config::WorkspaceConfig, context_menu, pane_actions, pane_radar, paths, platform};
use crate::{proc_radar, remote_telemetry, session, shortcuts, sidebar, split_pane, ssh_ui};
use crate::{tab_order, tab_strip, telemetry, topbar};

pub(crate) fn window_destroy_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_window_destroy
}

pub(crate) fn terminate_cb() -> extern "C" fn(*mut c_void) -> c_int {
    on_terminate
}

pub(crate) fn sidebar_search_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_sidebar_search_changed
}

extern "C" fn on_window_destroy(_w: *mut GtkWidget, _data: *mut c_void) {
    save_tabs();
    unsafe { gtk_main_quit() };
}

extern "C" fn on_terminate(_data: *mut c_void) -> c_int {
    on_window_destroy(ptr::null_mut(), ptr::null_mut());
    0
}

extern "C" fn on_sidebar_search_changed(entry: *mut GtkWidget, _data: *mut c_void) {
    let q = app_state::entry_text_lower(entry);
    let plan = app_state::with_app_mut(|app| {
        app.sidebar_filter = q;
        let reveal = (!app.sidebar_filter.is_empty()).then(|| app.sidebar.tmux_widget());
        (reveal, prepare_sidebar_filter(app))
    });
    if let Some((reveal, sidebar_plan)) = plan {
        if let Some(exp) = reveal {
            unsafe { gtk_expander_set_expanded(exp, 1) };
        }
        sidebar_plan.apply();
    }
}

pub(crate) fn prepare_sidebar_filter(app: &AppState) -> sidebar::SidebarUpdate {
    let tmux_rows = app.tmux_jump_btns.iter().map(|(_, _, key, button, _)| (key.as_str(), *button));
    app.sidebar.prepare_filter(&app.sidebar_filter, &app.sessions, tmux_rows)
}

fn clear_pane_unread_for(s: &mut TermSession, vte: *mut GtkWidget) {
    s.pane_registry.focus_pane(vte, Instant::now());
}

extern "C" fn on_vte_focus_in(
    vte: *mut GtkWidget,
    _event: *const GdkEvent,
    user_data: *mut c_void,
) -> c_int {
    let sid = user_data as usize;
    let updated = app_state::with_app_mut(|app| {
        let s = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(vte))?;
        s.panes.record_focus(vte);
        clear_pane_unread_for(s, vte);
        sync_session_proc(s, None);
        Some((s.id == app.active_id, app.search_bar.clone()))
    })
    .flatten();
    if let Some((is_active, bar)) = updated {
        if is_active {
            bar.sync_active_vte(vte);
        }
        topbar::refresh_ui();
    }
    0
}

pub(crate) fn lookup_vte_link_cwd(vte: *mut GtkWidget) -> LinkCwd {
    app_state::with_app(|app| {
        let Some(s) = app.sessions.iter().find(|s| s.has_vte(vte)) else {
            return LinkCwd::Unmanaged;
        };
        if s.remote().is_some() || (s.is_dynamic_ssh() && vte == s.active_vte()) {
            return LinkCwd::Remote;
        }
        s.panes
            .live_cwd_for_vte(vte, s.vte, s.child_pid, &s.cwd)
            .map_or(LinkCwd::Unmanaged, LinkCwd::Local)
    })
    .unwrap_or(LinkCwd::Unmanaged)
}

fn mark_pane_working(s: &mut TermSession, vte: *mut GtkWidget, is_active_tab: bool) {
    let is_daemon = s.category == SessionCategory::Daemon;
    let is_focused = is_active_tab && s.active_vte() == vte;
    let now = Instant::now();
    if let Some(pane) = s.pane_registry.by_vte_mut(vte) {
        pane.last_exit_code = None;
        if !is_daemon {
            pane.mark_output(now, is_focused);
        } else {
            pane.last_output = now;
        }
    }
}

extern "C" fn on_vte_contents_changed(vte: *mut GtkWidget, user_data: *mut c_void) {
    let sid = user_data as usize;
    app_state::with_app_mut(|app| {
        let active = app.active_id;
        let session = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(vte));
        if let Some(s) = session.filter(|s| s.pid_for_vte(vte) > 0 || s.active_pid() > 0) {
            mark_pane_working(s, vte, sid == active);
            mark_session_working(s, sid == active);
        }
    });
}

fn mark_session_working(s: &mut TermSession, is_active: bool) {
    let now = Instant::now();
    s.last_output = now;
    if s.category != SessionCategory::Daemon && s.state != WorkState::Working {
        s.state = WorkState::Working;
        s.work_started = now;
        s.work_started_at = Some(now);
    }
    if is_active {
        s.unread = false;
    }
}

fn prepare_split_reap(exited_vte: *mut GtkWidget) -> Option<(bool, split_pane::CloseOp)> {
    app_state::with_app_mut(|app| {
        let active_id = app.active_id;
        let s = app.sessions.iter_mut().find(|s| s.has_vte(exited_vte))?;
        let op = s.panes.prepare_close_exited(
            exited_vte, &mut s.scroll_wrap, &mut s.vte, &mut s.child_pid, &mut s.cwd,
        )?;
        s.pane_registry.remove_by_vte(exited_vte);
        sync_session_proc(s, None);
        Some((s.id == active_id, op))
    })
    .flatten()
}

extern "C" fn on_reap_split_pane(data: *mut c_void) -> c_int {
    if let Some((is_active, op)) = prepare_split_reap(data as *mut GtkWidget) {
        op.apply_focus(is_active);
        topbar::refresh_ui();
        save_tabs();
    }
    0
}

extern "C" fn on_vte_child_exited(vte: *mut GtkWidget, _status: c_int, user_data: *mut c_void) {
    let sid = user_data as usize;
    let reap_split = app_state::with_app_mut(|app| {
        let s = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(vte))?;
        if s.panes.has_splits() {
            return Some(true);
        }
        if s.vte == vte {
            reset_exited_primary_pane(s);
        }
        Some(false)
    })
    .flatten()
    .unwrap_or(false);
    if reap_split {
        unsafe { g_idle_add(on_reap_split_pane, vte as *mut c_void) };
    }
}

fn reset_exited_primary_pane(s: &mut TermSession) {
    s.child_pid = 0;
    s.state = WorkState::Idle;
    s.unread = false;
    s.work_started_at = None;
    if let Some(pane) = s.pane_registry.by_vte_mut(s.vte) {
        pane.child_pid = 0;
        pane.state = WorkState::Idle;
        pane.unread = false;
        pane.work_started_at = None;
    }
}

extern "C" fn on_vte_title_changed(vte: *mut GtkWidget, user_data: *mut c_void) {
    if vte.is_null() {
        return;
    }
    let sid = user_data as usize;
    let Some(title_str) = read_vte_window_title(vte) else {
        return;
    };
    let updated = app_state::with_app_mut(|app| {
        let s = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(vte))?;
        sync_session_proc(s, None);
        s.apply_vte_title(vte, &title_str);
        session::refresh_smart_title(s);
        Some(true)
    })
    .flatten();
    if updated == Some(true) {
        topbar::refresh_ui();
    }
}

fn read_vte_window_title(vte: *mut GtkWidget) -> Option<String> {
    unsafe {
        let ptr = vte_terminal_get_window_title(vte);
        (!ptr.is_null()).then(|| CStr::from_ptr(ptr).to_string_lossy().into_owned())
    }
}

fn select_session_from_click(sid: usize) {
    if context_menu::is_rename_open() {
        return;
    }
    let open_pin =
        app_state::with_app(|app| app.pin_pop.is_open().then(|| app.pin_pop.clone())).flatten();
    if let Some(pin_pop) = open_pin {
        pin_pop.focus_entry();
        return;
    }
    select_session(sid);
}

#[cfg(test)]
pub(crate) fn select_session_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_select_session
}

#[cfg(test)]
pub(crate) fn select_sidebar_row_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_select_sidebar_row
}

extern "C" fn on_select_session(_btn: *mut GtkWidget, user_data: *mut c_void) {
    let sid = user_data as usize;
    if tab_order::should_suppress_click(sid, tab_order::DragAxis::Horizontal) {
        return;
    }
    select_session_from_click(sid);
}

extern "C" fn on_select_sidebar_row(_btn: *mut GtkWidget, user_data: *mut c_void) {
    let sid = user_data as usize;
    if tab_order::should_suppress_click(sid, tab_order::DragAxis::Vertical) {
        return;
    }
    select_session_from_click(sid);
}

pub(crate) fn tab_strip_handlers() -> tab_strip::TabHandlers {
    tab_strip::TabHandlers {
        on_select: on_select_session,
        on_close: on_close_tab,
        on_press: on_tab_button_press,
        on_motion: on_tab_motion,
        on_release: on_tab_button_release,
    }
}

pub(crate) fn sidebar_row_handlers() -> sidebar::SidebarRowHandlers {
    sidebar::SidebarRowHandlers {
        on_select: on_select_sidebar_row,
        on_press: on_sidebar_button_press,
        on_motion: on_sidebar_motion,
        on_release: on_sidebar_button_release,
    }
}

fn reorder_session_to(sid: usize, target_idx: usize) -> bool {
    let op = app_state::with_app_mut(|app| {
        let from = app.sessions.iter().position(|s| s.id == sid)?;
        if !tab_order::move_item(&mut app.sessions, from, target_idx) {
            return None;
        }
        let strip = app.tab_strip.reorder_op(&app.sessions[target_idx].top_tab, target_idx);
        let sidebar = app.sidebar.build_reorder_ops(&app.sessions);
        Some((app.active_id, tab_order::TabReorderOp { strip, sidebar }))
    })
    .flatten();
    let Some((active_id, reorder_op)) = op else {
        return false;
    };
    finish_reorder(active_id, reorder_op);
    true
}

fn finish_reorder(active_id: usize, reorder_op: tab_order::TabReorderOp) {
    reorder_op.apply();
    topbar::refresh_ui();
    unsafe { g_idle_add(on_reveal_tab, active_id as *mut c_void) };
    save_tabs();
}

pub(crate) fn move_active_tab(delta: isize) {
    let step = app_state::with_app(|app| {
        let cur = app.sessions.iter().position(|s| s.id == app.active_id)?;
        let next = tab_order::step_tab_index(cur, app.sessions.len(), delta)?;
        Some((app.active_id, next))
    })
    .flatten();
    if let Some((sid, next)) = step {
        reorder_session_to(sid, next);
    }
}

fn open_tab_pin_popover(sid: usize) {
    context_menu::dismiss_rename_popover();
    pane_actions::dismiss_git_popover();
    let target = app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        let initial = s.pinned_title.as_deref().unwrap_or(&s.title).to_owned();
        let pal = app.palette.is_open().then(|| app.palette.clone());
        Some((pal, app.pin_pop.clone(), s.top_tab.select_button(), initial))
    })
    .flatten();
    if let Some((pal, pin_pop, anchor, initial)) = target {
        if let Some(pal) = pal {
            pal.close();
        }
        pin_pop.open(anchor, sid, &initial);
    }
}

pub(crate) fn apply_pinned_tab_title(sid: usize, pinned: Option<String>) {
    let changed = app_state::with_app_mut(|app| {
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        s.pinned_title = session::sanitize_pinned_title(pinned.as_deref());
        sync_session_proc(s, None);
        Some(true)
    })
    .flatten();
    if changed == Some(true) {
        topbar::refresh_ui();
        save_tabs();
    }
    app_state::focus_active_vte();
}

fn start_tab_mouse_drag(btn: *mut GtkWidget, sid: usize, event: *const GdkEvent) {
    let Some(tabs_box) = app_state::lookup_tabs_box() else {
        return;
    };
    if let Some(strip_x) = platform::translate_event_x_to_ancestor(btn, tabs_box, event) {
        tab_order::begin_tab_drag(sid, strip_x);
    }
}

fn compute_drag_reorder(sid: usize, strip_x: f64) -> Option<usize> {
    let (cur, widgets) = app_state::with_app(|app| {
        let cur = app.sessions.iter().position(|s| s.id == sid)?;
        let widgets: Vec<*mut GtkWidget> =
            app.sessions.iter().map(|s| s.top_tab.tab_widget()).collect();
        Some((cur, widgets))
    })
    .flatten()?;
    let widths = tab_order::allocated_widths(&widgets);
    let target = tab_order::drag_target_index(&widths, tab_order::TAB_STRIP_SPACING, cur, strip_x);
    (target != cur).then_some(target)
}

#[cfg(test)]
pub(crate) fn tab_button_press_cb() -> extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int {
    on_tab_button_press
}

extern "C" fn on_tab_button_press(
    btn: *mut GtkWidget,
    event: *const GdkEvent,
    data: *mut c_void,
) -> c_int {
    let mut button: c_uint = 0;
    let ev_type = unsafe { gdk_event_get_event_type(event) };
    if unsafe { gdk_event_get_button(event, &mut button) } == 0 {
        return 0;
    }
    dispatch_tab_press(btn, event, data, button, ev_type)
}

fn set_drag_class_for_sid(sid: usize, axis: tab_order::DragAxis, active: bool) {
    app_state::with_app(|app| {
        let Some(session) = app.sessions.iter().find(|s| s.id == sid) else {
            return;
        };
        let (widget, class) = match axis {
            tab_order::DragAxis::Horizontal => (session.top_tab.tab_widget(), "tab-dragging"),
            tab_order::DragAxis::Vertical => (session.sidebar_row.row_widget(), "sidebar-row-dragging"),
        };
        set_class(widget, class, active);
    });
}

fn finish_drag_interaction(axis: tab_order::DragAxis) {
    if let Some((sid, done_axis, true)) = tab_order::end_drag_details() {
        set_drag_class_for_sid(sid, done_axis, false);
    } else if tab_order::is_drag_pending(axis) {
        let _ = tab_order::end_drag_details();
    }
}

fn dispatch_tab_press(
    btn: *mut GtkWidget,
    event: *const GdkEvent,
    data: *mut c_void,
    button: c_uint,
    ev_type: c_int,
) -> c_int {
    if button == 3 && ev_type == GDK_BUTTON_PRESS {
        finish_drag_interaction(tab_order::DragAxis::Horizontal);
        return (pane_actions::tab_context_menu_cb())(btn, event, data);
    }
    if button == 1 && ev_type == GDK_2BUTTON_PRESS {
        finish_drag_interaction(tab_order::DragAxis::Horizontal);
        open_tab_pin_popover(data as usize);
        return 1;
    }
    if button == 1 && ev_type == GDK_BUTTON_PRESS {
        start_tab_mouse_drag(btn, data as usize, event);
    }
    0
}

#[cfg(test)]
pub(crate) fn tab_motion_cb() -> extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int {
    on_tab_motion
}

extern "C" fn on_tab_motion(
    btn: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    if !tab_order::is_tab_drag_pending() {
        return 0;
    }
    let Some((sid, strip_x, started_now, tabs_box, tab_widget)) = read_tab_drag_step(btn, event) else {
        return 0;
    };
    if started_now {
        set_drag_class_for_sid(sid, tab_order::DragAxis::Horizontal, true);
        if app_state::active_session_id() != sid {
            select_session(sid);
        }
    }
    app_state::with_app(|app| app.tab_strip.reveal_drag_x(strip_x));
    let cur_origin = platform::translate_widget_x_to_ancestor(tab_widget, tabs_box, 0.0);
    if !tab_order::is_drag_settling(cur_origin) {
        if let Some(target) = compute_drag_reorder(sid, strip_x) {
            tab_order::mark_drag_reordered(cur_origin);
            reorder_session_to(sid, target);
        }
    }
    1
}

fn read_tab_drag_step(
    btn: *mut GtkWidget,
    event: *const GdkEvent,
) -> Option<(usize, f64, bool, *mut GtkWidget, *mut GtkWidget)> {
    let tabs_box = app_state::lookup_tabs_box()?;
    let cur_x = platform::translate_event_x_to_ancestor(btn, tabs_box, event)?;
    let (sid, strip_x, started_now) = tab_order::update_tab_drag(cur_x)?;
    let tab_widget = app_state::with_app(|app| {
        app.sessions
            .iter()
            .find(|s| s.id == sid)
            .map(|s| s.top_tab.tab_widget())
    })
    .flatten()?;
    Some((sid, strip_x, started_now, tabs_box, tab_widget))
}

#[cfg(test)]
pub(crate) fn tab_button_release_cb() -> extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int {
    on_tab_button_release
}

extern "C" fn on_tab_button_release(
    _btn: *mut GtkWidget,
    _event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    finish_drag_interaction(tab_order::DragAxis::Horizontal);
    0
}

#[cfg(test)]
pub(crate) fn sidebar_button_press_cb() -> extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int {
    on_sidebar_button_press
}

#[cfg(test)]
pub(crate) fn sidebar_motion_cb() -> extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int {
    on_sidebar_motion
}

#[cfg(test)]
pub(crate) fn sidebar_button_release_cb() -> extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int {
    on_sidebar_button_release
}

extern "C" fn on_sidebar_button_press(
    btn: *mut GtkWidget,
    event: *const GdkEvent,
    data: *mut c_void,
) -> c_int {
    let mut button: c_uint = 0;
    let ev_type = unsafe { gdk_event_get_event_type(event) };
    if unsafe { gdk_event_get_button(event, &mut button) } == 0 {
        return 0;
    }
    if button == 3 && ev_type == GDK_BUTTON_PRESS {
        finish_drag_interaction(tab_order::DragAxis::Vertical);
        return (pane_actions::tab_context_menu_cb())(btn, event, data);
    }
    if button == 1 && ev_type == GDK_BUTTON_PRESS {
        let parent = unsafe { gtk_widget_get_parent(btn) };
        if let Some(y) = platform::translate_event_y_to_ancestor(btn, parent, event) {
            tab_order::begin_sidebar_drag(data as usize, y);
        }
    }
    0
}

fn compute_sidebar_drag_reorder(sid: usize, parent: *mut GtkWidget, y: f64) -> Option<usize> {
    let (cur_pos, siblings) = app_state::with_app(|app| {
        let siblings: Vec<(usize, *mut GtkWidget)> = app
            .sessions
            .iter()
            .enumerate()
            .filter_map(|(idx, s)| {
                let row = s.sidebar_row.row_widget();
                let same_parent = !row.is_null() && unsafe { gtk_widget_get_parent(row) } == parent;
                let visible = !row.is_null() && unsafe { gtk_widget_get_visible(row) } != 0;
                (same_parent && visible).then_some((idx, row))
            })
            .collect();
        let cur_pos = siblings.iter().position(|(idx, _)| app.sessions[*idx].id == sid)?;
        Some((cur_pos, siblings))
    })
    .flatten()?;
    let widgets: Vec<*mut GtkWidget> = siblings.iter().map(|(_, w)| *w).collect();
    let heights = tab_order::allocated_heights(&widgets);
    let target_pos =
        tab_order::vertical_drag_target_index(&heights, tab_order::SIDEBAR_ROW_SPACING, cur_pos, y);
    (target_pos != cur_pos).then_some(siblings[target_pos].0)
}

extern "C" fn on_sidebar_motion(
    btn: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    if !tab_order::is_drag_pending(tab_order::DragAxis::Vertical) {
        return 0;
    }
    let parent = unsafe { gtk_widget_get_parent(btn) };
    let Some(cur_y) = platform::translate_event_y_to_ancestor(btn, parent, event) else {
        return 0;
    };
    let Some((sid, drag_y, started_now)) = tab_order::update_sidebar_drag(cur_y) else {
        return 0;
    };
    if started_now {
        set_drag_class_for_sid(sid, tab_order::DragAxis::Vertical, true);
        if app_state::active_session_id() != sid {
            select_session(sid);
        }
    }
    app_state::with_app(|app| app.sidebar.reveal_drag_y(parent, drag_y));
    let cur_origin = platform::translate_widget_y_to_ancestor(btn, parent, 0.0);
    if !tab_order::is_drag_settling(cur_origin) {
        if let Some(target) = compute_sidebar_drag_reorder(sid, parent, drag_y) {
            tab_order::mark_drag_reordered(cur_origin);
            reorder_session_to(sid, target);
        }
    }
    1
}

extern "C" fn on_sidebar_button_release(
    _btn: *mut GtkWidget,
    _event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    finish_drag_interaction(tab_order::DragAxis::Vertical);
    0
}

fn prepare_session_selection(
    sid: usize,
) -> Option<(*mut GtkWidget, Vec<*mut GtkWidget>, *mut GtkWidget, crate::search_bar::SearchBar)> {
    app_state::with_app_mut(|app| {
        app.active_id = sid;
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        s.unread = false;
        if s.state == WorkState::Ready {
            s.state = WorkState::Idle;
        }
        let active_vte = s.active_vte();
        clear_pane_unread_for(s, active_vte);
        let mut vtes = Vec::new();
        s.panes.collect_vtes(s.vte, &mut vtes);
        Some((app.stack, vtes, s.panes.last_focus, app.search_bar.clone()))
    })
    .flatten()
}

pub fn select_session(sid: usize) {
    if let Some((stack, vtes, last_focus, bar)) = prepare_session_selection(sid) {
        apply_session_selection(sid, stack, &vtes, last_focus, &bar);
    }
    topbar::refresh_ui();
    unsafe { g_idle_add(on_reveal_tab, sid as *mut c_void) };
    save_tabs();
}

fn apply_session_selection(
    sid: usize,
    stack: *mut GtkWidget,
    vtes: &[*mut GtkWidget],
    last_focus: *mut GtkWidget,
    bar: &crate::search_bar::SearchBar,
) {
    let focus_vte = shortcuts::pick_focused_vte(vtes, last_focus);
    if !stack.is_null() {
        if let Ok(name_c) = CString::new(format!("term_{sid}")) {
            unsafe { gtk_stack_set_visible_child_name(stack, name_c.as_ptr()) };
        }
    }
    if !focus_vte.is_null() {
        unsafe { gtk_widget_grab_focus(focus_vte) };
    }
    bar.sync_active_vte(focus_vte);
}

extern "C" fn on_reveal_tab(data: *mut c_void) -> c_int {
    let sid = data as usize;
    let reveal = app_state::with_app(|app| {
        if app.active_id != sid {
            return None;
        }
        let session = app.sessions.iter().find(|s| s.id == sid)?;
        Some((app.tab_strip, session.top_tab.tab_widget()))
    })
    .flatten();
    if let Some((tab_strip, tab_widget)) = reveal {
        tab_strip.reveal_widget(tab_widget);
    }
    0
}

pub(crate) fn add_remote_session(host: &str, tmux_session: Option<&str>) -> usize {
    open_tab(Tab::remote(host, tmux_session))
}

pub(crate) fn open_tab(tab: Tab) -> usize {
    match tab {
        Tab::Local { profile, cwd } => open_local_tab(profile, &cwd),
        Tab::Remote(ref remote) => {
            let title = remote.title();
            let args = remote.argv();
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            add_tab(&title, "", &paths::home_dir(), SessionCategory::Remote, &argv, tab)
        }
    }
}

pub(crate) fn open_local_tab(profile: String, cwd: &str) -> usize {
    let (info, shell) = APP.with(|cell| {
        let state = cell.borrow();
        let config = &state.as_ref().expect("app is initialized").config;
        (config.profile(&profile), config.shell.clone())
    });
    let cwd = info.resolve_cwd(cwd);
    let tab = Tab::Local { profile, cwd: cwd.clone() };
    let (category, command) = if info.monitor {
        (SessionCategory::Daemon, "htop")
    } else {
        (SessionCategory::Project, shell.as_str())
    };
    add_tab(&info.title, &info.directory, &cwd, category, &[command], tab)
}

pub(crate) fn open_default_tab() {
    ssh_ui::dismiss_tab_menu();
    if let Some(tab) = app_state::with_app(|app| app.config.new_tab()) {
        open_tab(tab);
    }
}

pub(crate) fn open_local_tab_in_folder(dir: &str) -> usize {
    ssh_ui::dismiss_tab_menu();
    let profile = app_state::with_app(|app| match app.config.new_tab() {
        Tab::Local { profile, .. } => profile,
        Tab::Remote(_) => "shell".to_owned(),
    })
    .unwrap_or_else(|| "shell".to_owned());
    open_local_tab(profile, dir)
}

pub(crate) fn new_tab_in_folder_cb() -> sidebar::FolderNewTabHandler {
    open_local_tab_in_folder_action
}

fn open_local_tab_in_folder_action(dir: &str) {
    let _ = open_local_tab_in_folder(dir);
}

pub(crate) fn new_tab_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_new_tab
}

extern "C" fn on_new_tab(_btn: *mut GtkWidget, _data: *mut c_void) {
    open_default_tab();
}

fn apply_app_vte_prefs(vte: *mut GtkWidget) {
    if let Some((font, scrollback)) = app_state::with_app(|app| {
        (app.config.font_desc.clone(), app.config.scrollback_lines as isize)
    }) {
        session::apply_vte_preferences(vte, &font, scrollback);
    }
}

pub(crate) fn create_terminal(cwd: &str, argv: &[&str], sid: usize, scale: f64) -> (*mut GtkWidget, i32) {
    let (vte, pid) = session::create_vte_widget(cwd, argv);
    apply_app_vte_prefs(vte);
    let handlers = [
        (c"key-press-event", shortcuts::window_key_press_cb() as *const c_void),
        (c"focus-in-event", on_vte_focus_in as *const c_void),
        (c"contents-changed", on_vte_contents_changed as *const c_void),
        (c"window-title-changed", on_vte_title_changed as *const c_void),
        (c"child-exited", on_vte_child_exited as *const c_void),
    ];
    unsafe {
        vte_terminal_set_font_scale(vte, scale);
        for (signal, handler) in handlers {
            g_signal_connect_data(vte, signal.as_ptr(), handler, sid as *mut c_void, ptr::null(), 0);
        }
    }
    (vte, pid)
}

fn mount_tab_stack(stack: *mut GtkWidget, root: *mut GtkWidget, sid: usize) {
    unsafe {
        if !stack.is_null() {
            let stack_name = CString::new(format!("term_{sid}")).unwrap();
            gtk_stack_add_named(stack, root, stack_name.as_ptr());
        }
        gtk_widget_show_all(root);
    }
}

fn spawn_tab_widgets(
    sid: usize,
    cwd: &str,
    argv: &[&str],
    scale: f64,
    stack: *mut GtkWidget,
) -> (*mut GtkWidget, *mut GtkWidget, split_pane::TabPanes, i32) {
    let (vte, pid) = create_terminal(cwd, argv, sid, scale);
    let term_scroll = split_pane::wrap_vte_scroll(vte);
    let panes = split_pane::TabPanes::new(term_scroll, vte);
    mount_tab_stack(stack, panes.root, sid);
    (vte, term_scroll, panes, pid)
}

struct SessionInit {
    sid: usize,
    title: String,
    profile_cwd: String,
    cwd: String,
    category: SessionCategory,
    vte: *mut GtkWidget,
    scroll_wrap: *mut GtkWidget,
    panes: split_pane::TabPanes,
    sidebar_row: sidebar::SidebarRow,
    top_tab: tab_strip::TabView,
    child_pid: i32,
    tab: Tab,
}

fn init_primary_pane_registry(init: &SessionInit, proc_info: &proc_radar::ProcInfo) -> crate::pane_registry::PaneRegistry {
    let mut reg = crate::pane_registry::PaneRegistry::default();
    let pid = reg.register(init.vte, init.scroll_wrap, init.child_pid, init.cwd.clone());
    if let Some(p) = reg.get_mut(pid) {
        p.proc_info = proc_info.clone();
    }
    reg
}

fn build_term_session(init: SessionInit) -> TermSession {
    let now = Instant::now();
    let proc_info = proc_radar::inspect_proc_tree(init.child_pid);
    let pane_registry = init_primary_pane_registry(&init, &proc_info);
    let mut s = TermSession {
        id: init.sid, base_title: init.title.clone(), profile_cwd: init.profile_cwd,
        title: init.title, osc_title: String::new(), pinned_title: None,
        cwd: init.cwd, category: init.category, vte: init.vte, scroll_wrap: init.scroll_wrap,
        panes: init.panes, sidebar_row: init.sidebar_row, top_tab: init.top_tab,
        child_pid: init.child_pid, state: WorkState::Idle, unread: false, last_output: now,
        work_started: now, work_started_at: None, last_turn_secs: None, last_notified: None,
        proc_info, tab: init.tab, remote_tmux: None, tmux_title_from_vte: false,
        telemetry: telemetry::SessionTelemetry::default(), pane_registry,
    };
    session::refresh_smart_title(&mut s);
    s
}

fn reserve_tab_slot() -> Option<(usize, f64, *mut GtkWidget, sidebar::Sidebar, tab_strip::TabStrip)> {
    app_state::with_app_mut(|app| {
        let sid = app.next_id;
        app.next_id += 1;
        (sid, app.font_scale, app.stack, app.sidebar.clone(), app.tab_strip)
    })
}

pub(crate) fn add_tab(
    title: &str,
    profile_cwd: &str,
    cwd: &str,
    category: SessionCategory,
    argv: &[&str],
    tab: Tab,
) -> usize {
    let Some((sid, scale, stack, sidebar, tab_strip)) = reserve_tab_slot() else {
        return 0;
    };
    let (vte, scroll_wrap, panes, child_pid) = spawn_tab_widgets(sid, cwd, argv, scale, stack);
    let sidebar_row = sidebar.append_tab_with_handlers(category, sid, sidebar_row_handlers());
    let top_tab = tab_strip.append(sid, category, tab_strip_handlers());
    let session = build_term_session(SessionInit {
        sid, title: title.to_owned(), profile_cwd: profile_cwd.to_owned(), cwd: cwd.to_owned(),
        category, vte, scroll_wrap, panes, sidebar_row, top_tab, child_pid, tab,
    });
    app_state::with_app_mut(|app| {
        app.sessions.push(session);
        app.active_id = sid;
    });
    select_session(sid);
    sid
}

pub(crate) fn local_split_context(
    sid_opt: Option<usize>,
    target_vte: *mut GtkWidget,
) -> Option<(usize, *mut GtkWidget, String, String, f64)> {
    app_state::with_app(|app| {
        let id = sid_opt.unwrap_or(app.active_id);
        let s = app.sessions.iter().find(|s| s.id == id && s.remote().is_none())?;
        let vte = app_state::pick_session_vte(s, target_vte);
        Some((s.id, vte, app_state::pane_cwd(s, vte), app.config.shell.clone(), app.font_scale))
    })
    .flatten()
}

enum SplitTarget {
    Local { cwd: String, shell: String },
    Remote(RemoteConnection),
}

fn resolve_split_target(s: &TermSession, vte: *mut GtkWidget, shell: &str) -> Option<SplitTarget> {
    if let Some(conn) = s.panes.remote_for_vte(vte, s.scroll_wrap, s.vte).or_else(|| s.remote()) {
        return Some(SplitTarget::Remote(conn.clone()));
    }
    if s.is_dynamic_ssh() && is_ssh_host(&s.proc_info.ssh_host) {
        return Some(SplitTarget::Remote(RemoteConnection {
            host: s.proc_info.ssh_host.clone(),
            tmux_session: s.remote_tmux.as_ref().map(|(sess, ..)| sess.clone()).filter(|t| !t.is_empty()),
        }));
    }
    Some(SplitTarget::Local { cwd: app_state::pane_cwd(s, vte), shell: shell.to_owned() })
}

fn split_context_any(
    sid_opt: Option<usize>,
    target_vte: *mut GtkWidget,
) -> Option<(usize, *mut GtkWidget, SplitTarget, f64, usize)> {
    app_state::with_app_mut(|app| {
        let id = sid_opt.unwrap_or(app.active_id);
        let shell = app.config.shell.clone();
        let scale = app.font_scale;
        let s = app.sessions.iter_mut().find(|s| s.id == id)?;
        let vte = app_state::pick_session_vte(s, target_vte);
        let target = resolve_split_target(s, vte, &shell)?;
        let seq = s.panes.next_remote_seq();
        Some((s.id, vte, target, scale, seq))
    })
    .flatten()
}

fn spawn_split_terminal(
    target: &SplitTarget,
    sid: usize,
    scale: f64,
    seq: usize,
) -> (String, *mut GtkWidget, i32, Option<RemoteConnection>) {
    match target {
        SplitTarget::Local { cwd, shell } => {
            let (vte, pid) = create_terminal(cwd, &[shell.as_str()], sid, scale);
            (cwd.clone(), vte, pid, None)
        }
        SplitTarget::Remote(conn) => {
            let args = conn.split_argv(seq);
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            let (vte, pid) = create_terminal(&paths::home_dir(), &argv, sid, scale);
            (String::new(), vte, pid, Some(conn.clone()))
        }
    }
}

pub(crate) fn register_split_pane(
    s: &mut TermSession,
    new_vte: *mut GtkWidget,
    new_scroll: *mut GtkWidget,
    new_pid: i32,
    cwd: String,
    host: Option<&str>,
) {
    let id = s.pane_registry.register(new_vte, new_scroll, new_pid, cwd);
    if let (Some(h), Some(p)) = (host, s.pane_registry.get_mut(id)) {
        p.proc_info.ssh_host = h.to_owned();
    }
}

fn commit_split_to_session(
    sid: usize,
    pane_vte: *mut GtkWidget,
    new_scroll: *mut GtkWidget,
    new_vte: *mut GtkWidget,
    new_pid: i32,
    cwd: String,
    orientation: c_int,
    remote: Option<RemoteConnection>,
) -> Option<split_pane::SplitOp> {
    app_state::with_app_mut(|app| {
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        let chosen = if s.has_vte(pane_vte) { pane_vte } else { ptr::null_mut() };
        let op = s.panes.prepare_split_vte(
            chosen, s.scroll_wrap, s.vte, new_scroll, new_vte, new_pid, cwd.clone(), orientation,
        );
        let host = remote.as_ref().map(|r| r.host.as_str());
        register_split_pane(s, new_vte, new_scroll, new_pid, cwd, host);
        if let Some(conn) = remote {
            s.panes.record_remote_pane(new_scroll, conn);
        }
        Some(op)
    })
    .flatten()
}

pub(crate) fn split_pane_in_session(
    sid_opt: Option<usize>,
    target_vte: *mut GtkWidget,
    orientation: c_int,
) {
    let Some((sid, pane_vte, target, scale, seq)) = split_context_any(sid_opt, target_vte) else {
        return;
    };
    let (cwd, new_vte, new_pid, remote) = spawn_split_terminal(&target, sid, scale, seq);
    let new_scroll = split_pane::wrap_vte_scroll(new_vte);
    let split_op = commit_split_to_session(
        sid, pane_vte, new_scroll, new_vte, new_pid, cwd, orientation, remote,
    );
    split_pane::finish_split(split_op, new_scroll, new_vte);
    topbar::refresh_ui();
    save_tabs();
}

pub(crate) fn focus_pane_direction(dir: split_pane::FocusDir) -> bool {
    let moved = app_state::with_app_mut(|app| {
        let sid = app.active_id;
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        s.panes.focus_direction(dir, s.scroll_wrap, s.vte)
    })
    .flatten()
    .is_some();
    if moved {
        topbar::refresh_ui();
    }
    moved
}

pub(crate) fn toggle_active_pane_zoom() {
    let toggled = app_state::with_app_mut(|app| {
        let sid = app.active_id;
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        Some(s.panes.toggle_zoom(s.scroll_wrap, s.vte))
    })
    .flatten()
    .unwrap_or(false);
    if toggled {
        topbar::refresh_ui();
    }
}

pub(crate) fn split_active_tab(orientation: c_int) {
    split_pane_in_session(None, ptr::null_mut(), orientation);
}

pub(crate) fn split_vert_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_split_vert
}

pub(crate) fn split_horiz_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_split_horiz
}

extern "C" fn on_split_vert(_btn: *mut GtkWidget, _data: *mut c_void) {
    split_active_tab(GTK_ORIENTATION_HORIZONTAL);
}

extern "C" fn on_split_horiz(_btn: *mut GtkWidget, _data: *mut c_void) {
    split_active_tab(GTK_ORIENTATION_VERTICAL);
}

pub(crate) fn close_active_pane_or_tab() {
    let sid = app_state::active_session_id();
    if sid == 0 {
        return;
    }
    let close_op = app_state::with_app_mut(|app| {
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        let closing_vte = s.active_vte();
        let op =
            s.panes.prepare_close_active(&mut s.scroll_wrap, &mut s.vte, &mut s.child_pid, &mut s.cwd)?;
        s.pane_registry.remove_by_vte(closing_vte);
        sync_session_proc(s, None);
        Some(op)
    })
    .flatten();
    if let Some(op) = close_op {
        op.apply_focus(true);
        topbar::refresh_ui();
        save_tabs();
    } else {
        close_session(sid);
    }
}

extern "C" fn on_close_tab(_btn: *mut GtkWidget, data: *mut c_void) {
    close_session(data as usize);
}

fn teardown_session(s: TermSession) {
    s.panes.stop_all_extra();
    if s.remote().is_some() {
        stop_ssh(s.child_pid);
    }
    unsafe { gtk_widget_destroy(s.panes.root) };
    s.sidebar_row.destroy();
    s.top_tab.destroy();
}

fn pop_session_by_id(app: &mut AppState, sid: usize) -> Option<(TermSession, tab_order::TabPinPopover)> {
    let pos = app.sessions.iter().position(|s| s.id == sid)?;
    let removed = app.sessions.remove(pos);
    if sid == app.active_id {
        let neighbor = pos.min(app.sessions.len().saturating_sub(1));
        app.active_id = app.sessions.get(neighbor).map_or(0, |s| s.id);
    }
    Some((removed, app.pin_pop.clone()))
}

pub(crate) fn close_session(sid: usize) {
    let removed = app_state::with_app_mut(|app| pop_session_by_id(app, sid)).flatten();
    if let Some((s, pin_pop)) = removed {
        pin_pop.detach();
        context_menu::dismiss_rename_popover();
        teardown_session(s);
        select_session(app_state::active_session_id());
    }
}

fn drain_other_sessions(app: &mut AppState, keep_sid: usize) -> Vec<TermSession> {
    if !app.sessions.iter().any(|s| s.id == keep_sid) {
        return Vec::new();
    }
    app.active_id = keep_sid;
    let mut removed = Vec::new();
    let mut idx = 0;
    while idx < app.sessions.len() {
        if app.sessions[idx].id == keep_sid {
            idx += 1;
        } else {
            removed.push(app.sessions.remove(idx));
        }
    }
    removed
}

pub(crate) fn close_other_sessions(keep_sid: usize) {
    let (drained, pin_pop) = app_state::with_app_mut(|app| {
        (drain_other_sessions(app, keep_sid), Some(app.pin_pop.clone()))
    })
    .unwrap_or((Vec::new(), None));
    if drained.is_empty() {
        return;
    }
    if let Some(pin_pop) = pin_pop {
        pin_pop.detach();
    }
    context_menu::dismiss_rename_popover();
    drained.into_iter().for_each(teardown_session);
    select_session(keep_sid);
}

fn stop_ssh(pid: i32) {
    if pid > 0 {
        const SIGKILL: c_int = 9;
        unsafe {
            if kill(-pid, SIGKILL) != 0 {
                kill(pid, SIGKILL);
            }
        }
    }
}

pub(crate) fn reconnect_active_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_reconnect_active
}

extern "C" fn on_reconnect_active(_btn: *mut GtkWidget, _data: *mut c_void) {
    reconnect_session(app_state::active_session_id());
}

pub(crate) fn reconnect_session(sid: usize) {
    let details = app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        Some((s.remote()?.clone(), s.cwd.clone(), app.font_scale, s.child_pid))
    })
    .flatten();
    let Some((remote, cwd, scale, old_pid)) = details else {
        return;
    };
    stop_ssh(old_pid);
    let args = remote.argv();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let (vte, pid) = create_terminal(&cwd, &argv, sid, scale);
    replace_terminal(sid, vte, pid, remote.title());
    select_session(sid);
}

fn reset_replaced_timings(s: &mut TermSession) {
    s.state = WorkState::Idle;
    s.unread = false;
    let now = Instant::now();
    s.last_output = now;
    s.work_started = now;
    s.work_started_at = None;
    s.last_turn_secs = None;
    s.telemetry = telemetry::SessionTelemetry::default();
}

fn reset_replaced_session(
    s: &mut TermSession,
    vte: *mut GtkWidget,
    pid: i32,
    title: String,
) -> *mut GtkWidget {
    let old_vte = std::mem::replace(&mut s.vte, vte);
    s.pane_registry.remove_by_vte(old_vte);
    s.pane_registry.register(vte, s.scroll_wrap, pid, s.cwd.clone());
    s.panes.record_focus(vte);
    s.child_pid = pid;
    s.proc_info = proc_radar::inspect_proc_tree(pid);
    s.remote_tmux = None;
    s.tmux_title_from_vte = false;
    s.osc_title.clear();
    s.base_title.clone_from(&title);
    s.title = title;
    session::refresh_smart_title(s);
    reset_replaced_timings(s);
    old_vte
}

fn replace_terminal(sid: usize, vte: *mut GtkWidget, pid: i32, title: String) {
    let swapped = app_state::with_app_mut(|app| {
        app.remote_telemetry.reset_session(sid);
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        let old_vte = reset_replaced_session(s, vte, pid, title);
        Some((old_vte, s.scroll_wrap))
    })
    .flatten();
    if let Some((old_vte, scroll_wrap)) = swapped {
        swap_vte_child(old_vte, scroll_wrap, vte);
    }
}

fn swap_vte_child(old_vte: *mut GtkWidget, scroll_wrap: *mut GtkWidget, vte: *mut GtkWidget) {
    unsafe {
        if !old_vte.is_null() {
            gtk_widget_destroy(old_vte);
        }
        if !scroll_wrap.is_null() && !vte.is_null() {
            gtk_container_add(scroll_wrap, vte);
            gtk_widget_show_all(scroll_wrap);
        }
    }
}

pub(crate) fn radar_tick_cb() -> extern "C" fn(*mut c_void) -> c_int {
    on_radar_tick
}

fn tick_radar_app(app: &mut AppState) {
    let box_snap = app.box_state.lock().map(|g| g.clone()).unwrap_or_default();
    let now = Instant::now();
    let active = app.active_id;
    let win_active = approval::sync_window_focus_urgency(app.win);
    for s in &mut app.sessions {
        sync_session_proc(s, Some(&box_snap));
        if s.pane_registry.is_empty() {
            approval::settle_working_session(s, now, active, app.win, win_active);
        } else {
            pane_radar::sync_and_settle_session(s, now, s.id == active, app.win, win_active);
        }
    }
    if !box_snap.gpu_lines.is_empty() {
        set_label_text(app.box_gpu_label, &box_snap.gpu_lines.join("\n"));
    }
    ssh_ui::sync_tmux_jump_buttons(app, &box_snap.tmux_items);
}

extern "C" fn on_radar_tick(_data: *mut c_void) -> c_int {
    app_state::with_app_mut(tick_radar_app);
    poll_all_session_telemetry();
    topbar::refresh_ui();
    save_tabs();
    1
}

pub(crate) fn sync_session_proc(s: &mut TermSession, telemetry: Option<&proc_radar::BoxTelemetry>) {
    s.apply_proc_snapshot(proc_radar::inspect_proc_tree(s.active_pid()));
    if let Some(box_snap) = telemetry {
        update_remote_proc_info(s, box_snap);
    } else if let Tab::Remote(remote) = &s.tab {
        if s.child_pid > 0 {
            s.proc_info.ssh_host.clone_from(&remote.host);
        }
    }
    session::refresh_smart_title(s);
}

fn update_remote_proc_info(s: &mut TermSession, telemetry: &proc_radar::BoxTelemetry) {
    let Tab::Remote(remote) = &s.tab else {
        return;
    };
    if s.child_pid <= 0 {
        return;
    }
    s.proc_info.ssh_host.clone_from(&remote.host);
    if s.tmux_title_from_vte {
        return;
    }
    let Some(target) = &remote.tmux_session else {
        return;
    };
    if let Some((_, name, win, cmd, path)) =
        telemetry.active_by_host.iter().find(|(host, name, _, _, _)| host == &remote.host && name == target)
    {
        s.remote_tmux = Some((name.clone(), win.clone(), cmd.clone(), path.clone()));
    }
}

pub(crate) fn remote_telemetry_cb() -> extern "C" fn(*mut c_void) -> c_int {
    on_remote_telemetry_ready
}

extern "C" fn on_remote_telemetry_ready(_data: *mut c_void) -> c_int {
    poll_all_session_telemetry();
    topbar::refresh_ui();
    0
}

fn snapshot_pane_telemetry_jobs() -> (Vec<pane_radar::PanePollJob>, HashMap<usize, remote_telemetry::RemoteTailer>) {
    app_state::with_app(|app| {
        let remote_targets: Vec<remote_telemetry::RemoteSessionTarget> =
            app.sessions.iter().filter_map(remote_telemetry::RemoteSessionTarget::from_session).collect();
        app.remote_telemetry.sync_targets(&remote_targets);
        let jobs = pane_radar::collect_sessions_pane_jobs(&app.sessions);
        (jobs, app.remote_telemetry.snapshot())
    })
    .unwrap_or_else(|| (Vec::new(), HashMap::new()))
}

fn poll_all_session_telemetry() {
    let (jobs, remote_map) = snapshot_pane_telemetry_jobs();
    if jobs.is_empty() {
        return;
    }
    let home = PathBuf::from(paths::home_dir());
    let updated = pane_radar::run_pane_poll_jobs(jobs, &remote_map, &home, Instant::now());
    app_state::with_app_mut(|app| pane_radar::apply_sessions_pane_telemetry(&mut app.sessions, updated));
}

pub(crate) fn send_to_session(sid: usize, text: &str) {
    let Ok(c) = CString::new(text) else {
        return;
    };
    let vte = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.id == sid).map(TermSession::active_vte)
    })
    .flatten();
    if let Some(w) = vte.filter(|w| !w.is_null()) {
        unsafe {
            vte_terminal_feed_child(w, c.as_ptr(), text.len() as isize);
            gtk_widget_grab_focus(w);
        }
    }
}

pub(crate) fn send_to_active(text: &str) {
    send_to_session(app_state::active_session_id(), text);
}

pub(crate) fn sigint_clicked_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_sigint_clicked
}

extern "C" fn on_sigint_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let target = app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        Some((s.active_vte(), s.active_pid()))
    })
    .flatten();
    if let Some((vte, pid)) = target.filter(|(w, _)| !w.is_null()) {
        send_sigint_to_pane(vte, pid);
    }
}

fn send_sigint_to_pane(vte: *mut GtkWidget, pid: i32) {
    unsafe {
        vte_terminal_feed_child(vte, b"\x03\0".as_ptr() as *const c_char, 1);
        if pid > 0 {
            let pgid = getpgid(pid);
            if pgid > 0 {
                kill(-pgid, 2);
            }
        }
        gtk_widget_grab_focus(vte);
    }
}

fn default_tabs(config: &WorkspaceConfig) -> TabsState {
    let tabs = if config.startup.is_empty() {
        vec![config.new_tab()]
    } else {
        config.startup.clone()
    };
    TabsState::new(0, tabs)
}

fn spawn_local_secondary(
    sid: usize,
    raw_cwd: &str,
    fallback_cwd: &str,
    shell: &str,
    scale: f64,
) -> split_pane::SpawnedPane {
    let cwd = split_pane::resolve_pane_cwd(raw_cwd, fallback_cwd);
    let (vte, child_pid) = create_terminal(&cwd, &[shell], sid, scale);
    let scroll = split_pane::wrap_vte_scroll(vte);
    split_pane::SpawnedPane { scroll, vte, child_pid, cwd, remote: None }
}

fn spawn_remote_secondary(
    sid: usize,
    conn: RemoteConnection,
    scale: f64,
    seq: usize,
) -> split_pane::SpawnedPane {
    let args = conn.split_argv(seq);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let (vte, child_pid) = create_terminal(&paths::home_dir(), &argv, sid, scale);
    let scroll = split_pane::wrap_vte_scroll(vte);
    split_pane::SpawnedPane { scroll, vte, child_pid, cwd: String::new(), remote: Some(conn) }
}

fn spawn_one_secondary_pane(
    sid: usize,
    leaf: SplitLeaf,
    fallback_cwd: &str,
    shell: &str,
    scale: f64,
    seq: usize,
) -> split_pane::SpawnedPane {
    match leaf {
        SplitLeaf::Local(raw) => spawn_local_secondary(sid, &raw, fallback_cwd, shell, scale),
        SplitLeaf::Remote(conn) => spawn_remote_secondary(sid, conn, scale, seq),
    }
}

fn spawn_secondary_panes(
    sid: usize,
    tree: &SplitTree,
    fallback_cwd: &str,
    shell: &str,
    scale: f64,
) -> (Vec<split_pane::SpawnedPane>, Vec<*mut GtkWidget>) {
    let mut leaves = Vec::new();
    tree.secondary_leaves(&mut leaves);
    let mut spawned = Vec::with_capacity(leaves.len());
    let mut scrolls = Vec::with_capacity(leaves.len());
    for (idx, leaf) in leaves.into_iter().enumerate() {
        let pane = spawn_one_secondary_pane(sid, leaf, fallback_cwd, shell, scale, idx + 1);
        scrolls.push(pane.scroll);
        spawned.push(pane);
    }
    (spawned, scrolls)
}

fn restore_session_splits(sid: usize, tree: &SplitTree) {
    let ctx = app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        let allowed = s.category == SessionCategory::Project || s.remote().is_some();
        allowed.then(|| (s.cwd.clone(), app.config.shell.clone(), app.font_scale))
    })
    .flatten();
    let Some((fallback_cwd, shell, scale)) = ctx else {
        return;
    };
    let (spawned, raw_scrolls) = spawn_secondary_panes(sid, tree, &fallback_cwd, &shell, scale);
    let op = app_state::with_app_mut(|app| {
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        s.panes.prepare_restore_splits(s.scroll_wrap, s.vte, tree.clone(), spawned)
    })
    .flatten();
    split_pane::finish_restore_splits(op, &raw_scrolls);
}

fn apply_restored_meta(sid: usize, pinned: Option<&str>, splits: Option<&SplitTree>) {
    if let Some(pinned) = pinned {
        app_state::with_app_mut(|app| {
            if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid) {
                s.set_pinned_title(Some(pinned));
            }
        });
    }
    if let Some(tree) = splits {
        restore_session_splits(sid, tree);
    }
}

fn apply_restored_font_scale(scale: f64) {
    let zoom_btn = app_state::with_app_mut(|app| {
        app.font_scale = scale;
        app.zoom_btn
    });
    split_pane::apply_zoom_scale(&[], scale, zoom_btn);
}

pub(crate) fn restore_tabs() {
    let (store, saved) = TabStore::load();
    let state = saved.unwrap_or_else(|| {
        APP.with(|cell| default_tabs(&cell.borrow().as_ref().expect("app is initialized").config))
    });
    apply_restored_font_scale(state.font_scale());
    let mut ids = Vec::with_capacity(state.tabs.len());
    for (idx, tab) in state.tabs.iter().cloned().enumerate() {
        let sid = open_tab(tab);
        apply_restored_meta(sid, state.pinned_at(idx), state.splits_at(idx));
        ids.push(sid);
    }
    select_session(ids.get(state.active).copied().unwrap_or(0));
    app_state::with_app_mut(|app| app.tab_store = Some(store));
    save_tabs();
}

fn snapshot_session_meta(s: &mut TermSession, tab: &Tab) -> TabMeta {
    let mut meta = s.snapshot_meta();
    if matches!(tab, Tab::Remote(_)) {
        meta.splits = s.panes.snapshot_splits_for_tab(s.scroll_wrap, tab);
    } else if s.is_dynamic_ssh() {
        meta.splits = None;
    }
    meta
}

fn collect_tabs_snapshot(app: &mut AppState) -> TabsState {
    let active = app.sessions.iter().position(|s| s.id == app.active_id).unwrap_or(0);
    let mut tabs = Vec::with_capacity(app.sessions.len());
    let mut meta = Vec::with_capacity(app.sessions.len());
    for s in &mut app.sessions {
        let tab = s.snapshot_tab();
        meta.push(snapshot_session_meta(s, &tab));
        tabs.push(tab);
    }
    TabsState::with_meta(active, tabs, meta).with_font_scale(app.font_scale)
}

pub(crate) fn save_tabs() {
    app_state::with_app_mut(|app| {
        let snap = collect_tabs_snapshot(app);
        if let Some(store) = &mut app.tab_store {
            store.checkpoint(snap);
        }
    });
}
