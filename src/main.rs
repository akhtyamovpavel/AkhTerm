mod approval;
mod config;
mod context_menu;
mod ffi;
mod file_drop;
mod git_status;
mod git_wt;
mod handoff;
mod mi;
mod mi_watch;
mod palette;
mod paste_route;
mod paths;
mod platform;
mod proc_radar;
mod search_bar;
mod session;
mod sidebar;
mod split_pane;
mod ssh_detect;
mod tab_order;
mod tab_strip;
mod tabs;
mod telemetry;
mod telemetry_agy;
mod theme;
mod theme_mode;
mod ui_font;
mod ui_text;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use approval::{
    find_next_session_attention, is_jump_attention_shortcut, settle_working_session,
    sync_window_focus_urgency,
};
use config::WorkspaceConfig;
use context_menu::{
    format_session_target, save_forward_capture, ContextAction, DispatchFn, SessionTarget,
    TabMenuSpec, VteMenuSpec,
};
use ffi::*;
use file_drop::{
    feed_vte_text, register_file_drop_hooks, DropTarget, FileDropHooks, UploadCompletion,
};
use git_status::{
    build_diff_pane_argv, is_git_popover_shortcut, lookup_git_status, rebuild_git_widgets,
    remove_clean_worktree, resolve_git_repo, update_git_button, GitMenu, GitMenuCallbacks,
    GitMenuWidgets, GitRepoStatus, GitStatusScheduler, WorktreeRemovalOutcome,
};
use git_wt::{agent_slug_from_cmd, create_agent_worktree, GitHeadInfo};
use handoff::{
    current_unix_secs, list_handoff_files, rebuild_handoff_widgets, resolve_handoff_delivery,
    save_vte_capture, CaptureMeta, HandoffCallbacks, HandoffMenu, HandoffTarget,
};
use mi::ProjectMiReport;
use mi_watch::{
    build_mi_refactor_prompt, format_mi_button_label, format_mi_popover_markup, lookup_mi,
    mi_grade_class, resolve_active_mi, MiScheduler,
};
use paste_route::{
    choose_paste_route, is_c_key, is_ctrl_shift_shortcut, is_plain_ctrl_shortcut, is_v_key,
    resolve_pane_agent, PasteRoute,
};
use paths::{expand_home, home_dir, shorten_home};
use proc_radar::{
    clipboard_has_direct_image, detect_local_agent, inspect_proc_tree, load_ssh_hosts,
    open_external_link, query_ssh_servers, save_ssh_hosts, switch_remote_tmux_session,
    try_paste_clipboard_image, BoxTelemetry, RemoteTmuxItem, SshHostEntry,
};
use session::{
    create_vte_widget, refresh_smart_title, register_link_cwd_lookup, register_vte_menu_hook,
    LinkCwd, SessionCategory, TermSession, WorkState,
};
use sidebar::Sidebar;
use split_pane::{apply_zoom_scale, finish_split, wrap_vte_scroll, TabPanes};
use tab_strip::{TabHandlers, TabStrip};
use tabs::{Tab, TabStore, TabsState};
use telemetry::{
    build_sessions_tokens_view, run_poll_job, SessionPollJob, SessionTelemetry, TokensMenu,
    TokensViewData,
};
use ui_text as text;

struct AppState {
    config: WorkspaceConfig,
    win: *mut GtkWidget,
    is_fullscreen: bool,
    sidebar: Sidebar,
    sidebar_visible: bool,
    theme_mode: theme_mode::ThemeMode,
    theme_btn: *mut GtkWidget,
    font_scale: f64,
    zoom_btn: *mut GtkWidget,
    reconnect_btn: *mut GtkWidget,
    split_vert_btn: *mut GtkWidget,
    split_horiz_btn: *mut GtkWidget,
    next_id: usize,
    active_id: usize,
    sessions: Vec<TermSession>,
    tab_store: Option<TabStore>,
    stack: *mut GtkWidget,
    tab_strip: TabStrip,
    mi_btn: *mut GtkWidget,
    mi_pop: *mut GtkWidget,
    mi_pop_label: *mut GtkWidget,
    mi_refactor_btn: *mut GtkWidget,
    tokens_menu: TokensMenu,
    agents_pop: *mut GtkWidget,
    box_pop: *mut GtkWidget,
    box_gpu_label: *mut GtkWidget,
    box_tmux_pop_box: *mut GtkWidget,
    settings_pop: *mut GtkWidget,
    ssh_count_label: *mut GtkWidget,
    git_menu: GitMenu,
    handoff: HandoffMenu,
    footer_status: *mut GtkWidget,
    footer_hold_until: Cell<Option<Instant>>,
    ssh_hosts: Vec<SshHostEntry>,
    ssh_toggle_btns: Vec<*mut GtkWidget>,
    ssh_rows: Vec<(String, *mut GtkWidget)>,
    box_ssh_rows: Vec<(String, *mut GtkWidget)>,
    sidebar_filter: String,
    box_filter: String,
    enabled_hosts_shared: Arc<Mutex<Vec<String>>>,
    tmux_jump_btns: Vec<(String, String, String, *mut GtkWidget, *mut GtkWidget)>,
    last_tmux_keys: Vec<String>,
    mi_scheduler: MiScheduler,
    git_scheduler: GitStatusScheduler,
    box_state: Arc<Mutex<BoxTelemetry>>,
    palette: palette::CommandPalette,
    search_bar: search_bar::SearchBar,
    pin_pop: tab_order::TabPinPopover,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
    static PASTE_BUSY: Cell<bool> = const { Cell::new(false) };
    static WT_REMOVE_BUSY: Cell<bool> = const { Cell::new(false) };
}

fn entry_text_lower(entry: *mut GtkWidget) -> String {
    unsafe {
        let ptr = gtk_entry_get_text(entry);
        if ptr.is_null() {
            String::new()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().trim().to_lowercase()
        }
    }
}

pub fn set_zoom(new_scale: f64) {
    let clamped = new_scale.clamp(0.5, 3.0);
    let mut vtes = Vec::new();
    let zoom_btn = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut()?;
        app.font_scale = clamped;
        for s in &app.sessions {
            s.panes.collect_vtes(s.vte, &mut vtes);
        }
        Some(app.zoom_btn)
    });
    apply_zoom_scale(&vtes, clamped, zoom_btn);
    refresh_ui();
}

pub fn adjust_zoom(delta: f64) {
    let mut target = 1.0;
    APP.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            target = app.font_scale + delta;
        }
    });
    set_zoom(target);
}

extern "C" fn on_zoom_in(_btn: *mut GtkWidget, _data: *mut c_void) {
    adjust_zoom(0.1);
}

extern "C" fn on_zoom_out(_btn: *mut GtkWidget, _data: *mut c_void) {
    adjust_zoom(-0.1);
}

extern "C" fn on_zoom_reset(_btn: *mut GtkWidget, _data: *mut c_void) {
    set_zoom(1.0);
}

extern "C" fn on_toggle_sidebar(_btn: *mut GtkWidget, _data: *mut c_void) {
    let toggle = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut()?;
        app.sidebar_visible = !app.sidebar_visible;
        Some((app.sidebar.panel_widget(), app.sidebar_visible))
    });
    if let Some((panel, vis)) = toggle.filter(|(w, _)| !w.is_null()) {
        unsafe { gtk_widget_set_visible(panel, i32::from(vis)) };
    }
}

fn collect_theme_targets(mode: theme_mode::ThemeMode) -> Option<(*mut GtkWidget, Vec<*mut GtkWidget>)> {
    APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        app.theme_mode = mode;
        let mut vtes = Vec::new();
        for s in &app.sessions {
            s.panes.collect_vtes(s.vte, &mut vtes);
        }
        Some((app.theme_btn, vtes))
    })
}

fn apply_live_theme(mode: theme_mode::ThemeMode, persist: bool) {
    let targets = collect_theme_targets(mode);
    if persist {
        theme_mode::persist_theme_mode(mode);
    }
    theme::apply_gtk_theme_mode(mode);
    if let Some((theme_btn, vtes)) = targets {
        theme::apply_vte_list_colors(&vtes, mode);
        theme::sync_theme_button(theme_btn, mode);
    }
    refresh_ui();
}

fn close_open_palette() {
    let open_pal = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        app.palette.is_open().then(|| app.palette.clone())
    });
    if let Some(pal) = open_pal {
        pal.close();
    }
}

fn toggle_theme() {
    close_open_palette();
    let next = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        Some(state.as_ref()?.theme_mode.toggled())
    });
    if let Some(mode) = next {
        apply_live_theme(mode, true);
        focus_active_vte();
    }
}

extern "C" fn on_toggle_theme(_btn: *mut GtkWidget, _data: *mut c_void) {
    toggle_theme();
}

extern "C" fn on_toggle_window_square(_btn: *mut GtkWidget, _data: *mut c_void) {
    let toggle = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut()?;
        app.is_fullscreen = !app.is_fullscreen;
        Some((app.win, app.is_fullscreen))
    });
    if let Some((win, fullscreen)) = toggle.filter(|(w, _)| !w.is_null()) {
        unsafe {
            if fullscreen {
                gtk_window_fullscreen(win);
            } else {
                gtk_window_unfullscreen(win);
            }
        }
    }
}

extern "C" fn on_ssh_search_changed(entry: *mut GtkWidget, _data: *mut c_void) {
    let q = entry_text_lower(entry);
    APP.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            let mut shown = 0usize;
            for (key, row) in &app.ssh_rows {
                let vis = q.is_empty() || key.contains(&q);
                if vis {
                    shown += 1;
                }
                unsafe {
                    gtk_widget_set_visible(*row, if vis { 1 } else { 0 });
                }
            }
            let txt = format!(
                "Показано серверов: {} из {}  (клик [✓] — опрос tmux/GPU, клик 🖥 — войти)",
                shown,
                app.ssh_rows.len()
            );
            set_label_text(app.ssh_count_label, &txt);
        }
    });
}

extern "C" fn on_box_search_changed(entry: *mut GtkWidget, _data: *mut c_void) {
    let q = entry_text_lower(entry);
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.box_filter = q.clone();
            for (_, _, key, _, b_pop) in &app.tmux_jump_btns {
                let vis = q.is_empty() || key.contains(&q);
                unsafe {
                    gtk_widget_set_visible(*b_pop, if vis { 1 } else { 0 });
                }
            }
            for (key, row) in &app.box_ssh_rows {
                let vis = q.is_empty() || key.contains(&q);
                unsafe {
                    gtk_widget_set_visible(*row, if vis { 1 } else { 0 });
                }
            }
        }
    });
}

extern "C" fn on_sidebar_search_changed(entry: *mut GtkWidget, _data: *mut c_void) {
    let q = entry_text_lower(entry);
    let plan = APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return None };
        let app = state.as_mut()?;
        app.sidebar_filter = q;
        let reveal = (!app.sidebar_filter.is_empty()).then(|| app.sidebar.tmux_widget());
        Some((reveal, prepare_sidebar_filter(app)))
    });
    if let Some((reveal, sidebar_plan)) = plan {
        if let Some(exp) = reveal {
            unsafe { gtk_expander_set_expanded(exp, 1) };
        }
        sidebar_plan.apply();
    }
}

fn prepare_sidebar_filter(app: &AppState) -> sidebar::SidebarUpdate {
    let tmux_rows = app.tmux_jump_btns.iter().map(|(_, _, key, button, _)| (key.as_str(), *button));
    app.sidebar.prepare_filter(&app.sidebar_filter, &app.sessions, tmux_rows)
}

extern "C" fn on_window_destroy(_w: *mut GtkWidget, _data: *mut c_void) {
    save_tabs();
    unsafe { gtk_main_quit() };
}

extern "C" fn on_terminate(_data: *mut c_void) -> c_int {
    on_window_destroy(ptr::null_mut(), ptr::null_mut());
    0
}

extern "C" fn on_vte_focus_in(
    vte: *mut GtkWidget,
    _event: *const GdkEvent,
    user_data: *mut c_void,
) -> c_int {
    let sid = user_data as usize;
    let updated = APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return None };
        let app = state.as_mut()?;
        let s = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(vte))?;
        s.panes.record_focus(vte);
        sync_session_proc(s, None);
        Some((s.id == app.active_id, app.search_bar.clone()))
    });
    if let Some((is_active, bar)) = updated {
        if is_active {
            bar.sync_active_vte(vte);
        }
        refresh_ui();
    }
    0
}

fn lookup_vte_link_cwd(vte: *mut GtkWidget) -> LinkCwd {
    APP.with(|cell| {
        let Ok(state) = cell.try_borrow() else { return LinkCwd::Unmanaged };
        let Some(app) = state.as_ref() else { return LinkCwd::Unmanaged };
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
}

extern "C" fn on_vte_contents_changed(vte: *mut GtkWidget, user_data: *mut c_void) {
    let sid = user_data as usize;
    APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return };
        if let Some(app) = state.as_mut() {
            let active = app.active_id;
            let session = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(vte));
            if let Some(s) = session.filter(|s| s.active_pid() > 0) {
                let now = Instant::now();
                s.last_output = now;
                if s.category != SessionCategory::Daemon && s.state != WorkState::Working {
                    s.state = WorkState::Working;
                    s.work_started = now;
                    s.work_started_at = Some(now);
                }
                if sid == active {
                    s.unread = false;
                }
            }
        }
    });
}

extern "C" fn on_reap_split_pane(data: *mut c_void) -> c_int {
    let exited_vte = data as *mut GtkWidget;
    let reap = APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return None };
        let app = state.as_mut()?;
        let active_id = app.active_id;
        let s = app.sessions.iter_mut().find(|s| s.has_vte(exited_vte))?;
        let op = s.panes.prepare_close_exited(
            exited_vte,
            &mut s.scroll_wrap,
            &mut s.vte,
            &mut s.child_pid,
            &mut s.cwd,
        )?;
        sync_session_proc(s, None);
        Some((s.id == active_id, op))
    });
    if let Some((is_active, op)) = reap {
        op.apply_focus(is_active);
        refresh_ui();
        save_tabs();
    }
    0
}

extern "C" fn on_vte_child_exited(vte: *mut GtkWidget, _status: c_int, user_data: *mut c_void) {
    let sid = user_data as usize;
    let mut reap_split = false;
    APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return };
        let Some(app) = state.as_mut() else { return };
        if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(vte)) {
            if s.panes.has_splits() {
                reap_split = true;
            } else if s.vte == vte {
                s.child_pid = 0;
                s.state = WorkState::Idle;
                s.unread = false;
                s.work_started_at = None;
            }
        }
    });
    if reap_split {
        unsafe { g_idle_add(on_reap_split_pane, vte as *mut c_void) };
    }
}

extern "C" fn on_vte_title_changed(vte: *mut GtkWidget, user_data: *mut c_void) {
    if vte.is_null() {
        return;
    }
    let sid = user_data as usize;
    let title_str = unsafe {
        let ptr = vte_terminal_get_window_title(vte);
        if ptr.is_null() {
            return;
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    };
    let updated = APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return false };
        let Some(app) = state.as_mut() else { return false };
        let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(vte)) else {
            return false;
        };
        sync_session_proc(s, None);
        s.apply_vte_title(vte, &title_str);
        refresh_smart_title(s);
        true
    });
    if updated {
        refresh_ui();
    }
}

extern "C" fn on_select_session(_btn: *mut GtkWidget, user_data: *mut c_void) {
    if context_menu::is_rename_open() {
        return;
    }
    let open_pin = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        app.pin_pop.is_open().then(|| app.pin_pop.clone())
    });
    if let Some(pin_pop) = open_pin {
        pin_pop.focus_entry();
        return;
    }
    select_session(user_data as usize);
}

fn tab_strip_handlers() -> TabHandlers {
    TabHandlers {
        on_select: on_select_session,
        on_close: on_close_tab,
        on_press: on_tab_button_press,
        on_motion: on_tab_motion,
        on_release: on_tab_button_release,
    }
}

fn reorder_session_to(sid: usize, target_idx: usize) -> bool {
    let op = APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return None };
        let app = state.as_mut()?;
        let from = app.sessions.iter().position(|s| s.id == sid)?;
        if !tab_order::move_item(&mut app.sessions, from, target_idx) {
            return None;
        }
        let strip = app.tab_strip.reorder_op(&app.sessions[target_idx].top_tab, target_idx);
        let sidebar = app.sidebar.build_reorder_ops(&app.sessions);
        Some((app.active_id, tab_order::TabReorderOp { strip, sidebar }))
    });
    let Some((active_id, reorder_op)) = op else { return false };
    reorder_op.apply();
    refresh_ui();
    unsafe { g_idle_add(on_reveal_tab, active_id as *mut c_void) };
    save_tabs();
    true
}

fn move_active_tab(delta: isize) {
    let step = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let cur = app.sessions.iter().position(|s| s.id == app.active_id)?;
        let next = tab_order::step_tab_index(cur, app.sessions.len(), delta)?;
        Some((app.active_id, next))
    });
    if let Some((sid, next)) = step {
        reorder_session_to(sid, next);
    }
}

fn open_tab_pin_popover(sid: usize) {
    context_menu::dismiss_rename_popover();
    dismiss_git_popover();
    let target = APP.with(|cell| {
        let Ok(state) = cell.try_borrow() else { return None };
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        let initial = s.pinned_title.as_deref().unwrap_or(&s.title).to_owned();
        let pal = app.palette.is_open().then(|| app.palette.clone());
        Some((pal, app.pin_pop.clone(), s.top_tab.select_button(), initial))
    });
    if let Some((pal, pin_pop, anchor, initial)) = target {
        if let Some(pal) = pal {
            pal.close();
        }
        pin_pop.open(anchor, sid, &initial);
    }
}

fn apply_pinned_tab_title(sid: usize, pinned: Option<String>) {
    let changed = APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return false };
        let Some(app) = state.as_mut() else { return false };
        let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid) else { return false };
        s.pinned_title = session::sanitize_pinned_title(pinned.as_deref());
        sync_session_proc(s, None);
        true
    });
    if changed {
        refresh_ui();
        save_tabs();
    }
    focus_active_vte();
}

fn lookup_tabs_box() -> Option<*mut GtkWidget> {
    APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        Some(state.as_ref()?.tab_strip.tabs_box())
    })
}

fn start_tab_mouse_drag(btn: *mut GtkWidget, sid: usize, event: *const GdkEvent) {
    let Some(tabs_box) = lookup_tabs_box() else {
        return;
    };
    if let Some(strip_x) = platform::translate_event_x_to_ancestor(btn, tabs_box, event) {
        tab_order::begin_tab_drag(sid, strip_x);
    }
}

extern "C" fn on_tab_button_press(
    btn: *mut GtkWidget,
    event: *const GdkEvent,
    data: *mut c_void,
) -> c_int {
    let sid = data as usize;
    let mut button: c_uint = 0;
    let ev_type = unsafe { gdk_event_get_event_type(event) };
    if unsafe { gdk_event_get_button(event, &mut button) } == 0 {
        return 0;
    }
    if button == 3 && ev_type == GDK_BUTTON_PRESS {
        let _ = tab_order::end_tab_drag();
        return on_tab_context_menu(btn, event, data);
    }
    if button == 1 && ev_type == GDK_2BUTTON_PRESS {
        let _ = tab_order::end_tab_drag();
        open_tab_pin_popover(sid);
        return 1;
    }
    if button == 1 && ev_type == GDK_BUTTON_PRESS {
        start_tab_mouse_drag(btn, sid, event);
    }
    0
}

fn compute_drag_reorder(sid: usize, strip_x: f64) -> Option<usize> {
    let (cur, widgets) = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let cur = app.sessions.iter().position(|s| s.id == sid)?;
        let widgets: Vec<*mut GtkWidget> = app.sessions.iter().map(|s| s.top_tab.tab_widget()).collect();
        Some((cur, widgets))
    })?;
    let widths = tab_order::allocated_widths(&widgets);
    let target = tab_order::drag_target_index(&widths, tab_order::TAB_STRIP_SPACING, cur, strip_x);
    (target != cur).then_some(target)
}

extern "C" fn on_tab_motion(
    btn: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    if !tab_order::is_tab_drag_pending() {
        return 0;
    }
    let Some(tabs_box) = lookup_tabs_box() else {
        return 0;
    };
    let Some(cur_strip_x) = platform::translate_event_x_to_ancestor(btn, tabs_box, event) else {
        return 0;
    };
    let Some((sid, strip_x, started_now)) = tab_order::update_tab_drag(cur_strip_x) else {
        return 0;
    };
    if started_now && active_session_id() != sid {
        select_session(sid);
    }
    if let Some(target) = compute_drag_reorder(sid, strip_x) {
        reorder_session_to(sid, target);
    }
    1
}

extern "C" fn on_tab_button_release(
    _btn: *mut GtkWidget,
    _event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    let _ = tab_order::end_tab_drag();
    0
}

pub fn select_session(sid: usize) {
    let targets = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        app.active_id = sid;
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        s.unread = false;
        if s.state == WorkState::Ready {
            s.state = WorkState::Idle;
        }
        let mut vtes = Vec::new();
        s.panes.collect_vtes(s.vte, &mut vtes);
        Some((app.stack, vtes, s.panes.last_focus, app.search_bar.clone()))
    });
    if let Some((stack, vtes, last_focus, search_bar)) = targets {
        let focus_vte = pick_focused_vte(&vtes, last_focus);
        if !stack.is_null() {
            if let Ok(name_c) = CString::new(format!("term_{sid}")) {
                unsafe { gtk_stack_set_visible_child_name(stack, name_c.as_ptr()) };
            }
        }
        if !focus_vte.is_null() {
            unsafe { gtk_widget_grab_focus(focus_vte) };
        }
        search_bar.sync_active_vte(focus_vte);
    }
    refresh_ui();
    // The tab can be new and still waiting for its GTK size allocation.
    unsafe { g_idle_add(on_reveal_tab, sid as *mut c_void) };
    save_tabs();
}

extern "C" fn on_reveal_tab(data: *mut c_void) -> c_int {
    let sid = data as usize;
    let reveal = APP.with(|cell| {
        let Ok(state) = cell.try_borrow() else { return None };
        let app = state.as_ref().filter(|app| app.active_id == sid)?;
        let session = app.sessions.iter().find(|s| s.id == sid)?;
        Some((app.tab_strip, session.top_tab.tab_widget()))
    });
    if let Some((tab_strip, tab_widget)) = reveal {
        tab_strip.reveal_widget(tab_widget);
    }
    0
}

extern "C" fn on_toggle_ssh_host(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let idx = idx_ptr as usize;
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            if let Some(entry) = app.ssh_hosts.get_mut(idx) {
                entry.enabled = !entry.enabled;
                let mark = if entry.enabled { "✓" } else { "  " };
                let sub = if entry.hostname.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", entry.hostname)
                };
                let lbl = format!("[{mark}]  {}{sub}", entry.alias);
                if let Some(&btn_w) = app.ssh_toggle_btns.get(idx) {
                    if entry.enabled {
                        add_class(btn_w, "tab-unread");
                    } else {
                        remove_class(btn_w, "tab-unread");
                    }
                    if let Ok(c) = CString::new(lbl) {
                        unsafe { gtk_button_set_label(btn_w, c.as_ptr()) };
                    }
                }
            }
            save_ssh_hosts(&app.ssh_hosts);
            let enabled_list: Vec<String> = app
                .ssh_hosts
                .iter()
                .filter(|h| h.enabled)
                .map(|h| h.alias.clone())
                .collect();
            if let Ok(mut g) = app.enabled_hosts_shared.lock() {
                *g = enabled_list;
            }
        }
    });
}

extern "C" fn on_connect_ssh_host_tmux(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    connect_ssh_host(idx_ptr as usize, Some("main"));
}

extern "C" fn on_connect_ssh_host_plain(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    connect_ssh_host(idx_ptr as usize, None);
}

fn connect_ssh_host(idx: usize, tmux: Option<&str>) {
    let target = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let host = app.ssh_hosts.get(idx)?;
        Some((app.settings_pop, app.box_pop, host.alias.clone()))
    });
    if let Some((settings_pop, box_pop, host)) = target {
        if !settings_pop.is_null() {
            unsafe { gtk_popover_popdown(settings_pop) };
        }
        if !box_pop.is_null() {
            unsafe { gtk_popover_popdown(box_pop) };
        }
        add_remote_session(&host, tmux);
    }
}

extern "C" fn on_jump_tmux(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let idx = idx_ptr as usize;
    let (box_pop, target) = APP.with(|cell| {
        let state = cell.borrow();
        let Some(app) = state.as_ref() else {
            return (ptr::null_mut(), None);
        };
        let found = app
            .tmux_jump_btns
            .get(idx)
            .map(|(host, sname, _, _, _)| (host.clone(), sname.clone()));
        (app.box_pop, found)
    });
    if !box_pop.is_null() {
        unsafe { gtk_popover_popdown(box_pop) };
    }
    if let Some((host, session)) = target {
        jump_to_remote_tmux(&host, &session);
    }
}

fn jump_to_remote_tmux(target_host: &str, target_session: &str) {
    if target_session.is_empty() || target_host.is_empty() {
        return;
    }
    let existing = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        app.sessions
            .iter()
            .find(|s| s.remote().is_some_and(|r| r.host == target_host && r.tmux_session.is_some()))
            .map(|s| (s.id, s.child_pid > 0))
    });
    let Some((sid, connected)) = existing else {
        add_remote_session(target_host, Some(target_session));
        return;
    };
    retarget_remote_session(sid, target_session);
    if connected {
        switch_remote_tmux_session(target_host, target_session);
        select_session(sid);
    } else {
        reconnect_session(sid);
    }
}

fn retarget_remote_session(sid: usize, target_session: &str) {
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid) {
                if let Tab::Remote(remote) = &mut s.tab {
                    remote.tmux_session = Some(target_session.to_owned());
                    s.base_title = remote.title();
                    s.title = remote.title();
                }
                s.remote_tmux = None;
                s.tmux_title_from_vte = false;
                s.work_started_at = None;
                s.last_turn_secs = None;
                s.telemetry = SessionTelemetry::default();
                refresh_smart_title(s);
            }
        }
    });
}

fn add_remote_session(host: &str, tmux_session: Option<&str>) -> usize {
    open_tab(Tab::remote(host, tmux_session))
}

fn open_tab(tab: Tab) -> usize {
    match tab {
        Tab::Local { profile, cwd } => open_local_tab(profile, &cwd),
        Tab::Remote(ref remote) => {
            let title = remote.title();
            let args = remote.argv();
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            add_tab(&title, "", &home_dir(), SessionCategory::Remote, &argv, tab)
        }
    }
}

fn open_local_tab(profile: String, cwd: &str) -> usize {
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

fn create_terminal(cwd: &str, argv: &[&str], sid: usize, scale: f64) -> (*mut GtkWidget, i32) {
    let (vte, pid) = create_vte_widget(cwd, argv);
    let handlers = [
        (c"key-press-event", on_window_key_press as *const c_void),
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

fn spawn_tab_widgets(
    sid: usize,
    cwd: &str,
    argv: &[&str],
    scale: f64,
    stack: *mut GtkWidget,
) -> (*mut GtkWidget, *mut GtkWidget, TabPanes, i32) {
    let (vte, pid) = create_terminal(cwd, argv, sid, scale);
    let term_scroll = wrap_vte_scroll(vte);
    let panes = TabPanes::new(term_scroll, vte);
    unsafe {
        if !stack.is_null() {
            let stack_name = CString::new(format!("term_{sid}")).unwrap();
            gtk_stack_add_named(stack, panes.root, stack_name.as_ptr());
        }
        gtk_widget_show_all(panes.root);
    }
    (vte, term_scroll, panes, pid)
}

fn build_term_session(
    sid: usize,
    title: &str,
    profile_cwd: &str,
    cwd: &str,
    category: SessionCategory,
    vte: *mut GtkWidget,
    scroll_wrap: *mut GtkWidget,
    panes: TabPanes,
    sidebar_row: sidebar::SidebarRow,
    top_tab: tab_strip::TabView,
    child_pid: i32,
    tab: Tab,
) -> TermSession {
    let now = Instant::now();
    let mut session = TermSession {
        id: sid, base_title: title.to_string(), profile_cwd: profile_cwd.to_string(),
        title: title.to_string(), osc_title: String::new(), pinned_title: None,
        cwd: cwd.to_string(), category, vte, scroll_wrap, panes, sidebar_row, top_tab, child_pid,
        state: WorkState::Idle, unread: false, last_output: now, work_started: now,
        work_started_at: None, last_turn_secs: None, last_notified: None,
        proc_info: inspect_proc_tree(child_pid), tab, remote_tmux: None,
        tmux_title_from_vte: false, telemetry: SessionTelemetry::default(),
    };
    refresh_smart_title(&mut session);
    session
}

fn add_tab(
    title: &str,
    profile_cwd: &str,
    cwd: &str,
    category: SessionCategory,
    argv: &[&str],
    tab: Tab,
) -> usize {
    let Some((sid, scale, stack, sidebar, tab_strip)) = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut()?;
        let sid = app.next_id;
        app.next_id += 1;
        Some((sid, app.font_scale, app.stack, app.sidebar, app.tab_strip))
    }) else {
        return 0;
    };
    let (vte, scroll_wrap, panes, child_pid) = spawn_tab_widgets(sid, cwd, argv, scale, stack);
    let sidebar_row = sidebar.append_tab(category, sid, on_select_session, on_tab_context_menu);
    let top_tab = tab_strip.append(sid, category, tab_strip_handlers());
    let session = build_term_session(
        sid, title, profile_cwd, cwd, category, vte, scroll_wrap, panes, sidebar_row, top_tab,
        child_pid, tab,
    );
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.sessions.push(session);
            app.active_id = sid;
        }
    });
    select_session(sid);
    sid
}

fn sync_tmux_jump_buttons(app: &mut AppState, items: &[RemoteTmuxItem]) {
    let keys: Vec<String> = items.iter().map(|i| i.display_line.clone()).collect();
    if app.last_tmux_keys != keys {
        for (_, _, _, b1, b2) in app.tmux_jump_btns.drain(..) {
            unsafe {
                gtk_widget_destroy(b1);
                gtk_widget_destroy(b2);
            }
        }
        app.last_tmux_keys = keys;

        for (idx, item) in items.iter().enumerate() {
            let label_txt = format!("↳ {}", item.display_line);
            let search_key = format!("{} {} {}", item.host, item.session_name, item.display_line).to_lowercase();
            if let Ok(lbl_c) = CString::new(label_txt) {
                let description = item.display_line.split_once("] ").map_or(item.session_name.as_str(), |(_, text)| text);
                let title = format!("{} · {description}", item.host);
                let b_side = app.sidebar.append_tmux(&title, &item.display_line, idx, on_jump_tmux);
                unsafe {
                    let b_pop = gtk_button_new_with_label(lbl_c.as_ptr());
                    add_class(b_pop, "tmux-jump-btn");
                    connect_click(b_pop, on_jump_tmux, idx as *mut c_void);
                    gtk_box_pack_start(app.box_tmux_pop_box, b_pop, 0, 1, 0);
                    gtk_widget_show_all(b_pop);
                    let vis_pop = app.box_filter.is_empty() || search_key.contains(&app.box_filter);
                    gtk_widget_set_visible(b_pop, if vis_pop { 1 } else { 0 });

                    app.tmux_jump_btns.push((
                        item.host.clone(),
                        item.session_name.clone(),
                        search_key,
                        b_side,
                        b_pop,
                    ));
                }
            }
        }
    }

    for (idx, (_, _, _, b_side, b_pop)) in app.tmux_jump_btns.iter().enumerate() {
        let active = items.get(idx).is_some_and(|item| item.is_active);
        set_class(*b_side, "sidebar-tmux-current", active);
        set_class(*b_pop, "tmux-jump-active", active);
    }
}

fn collect_local_cwds(app: &AppState) -> Vec<String> {
    let mut cwds = Vec::new();
    if let Some(active) = app.sessions.iter().find(|s| s.id == app.active_id) {
        if let Some(cwd) = active.active_local_cwd() {
            cwds.push(cwd);
        }
    }
    for s in &app.sessions {
        if let Some(cwd) = s.active_local_cwd() {
            if !cwds.contains(&cwd) {
                cwds.push(cwd);
            }
        }
    }
    cwds
}

struct MiUiPlan {
    widgets: [*mut GtkWidget; 3],
    grade_cls: Option<&'static str>,
    texts: Option<(String, String)>,
}

impl MiUiPlan {
    fn from_state(
        app: &AppState,
        cur_mi: Option<&ProjectMiReport>,
        self_mi: Option<&ProjectMiReport>,
    ) -> Self {
        Self {
            widgets: [app.mi_btn, app.mi_pop_label, app.mi_refactor_btn],
            grade_cls: cur_mi.map(|rep| mi_grade_class(rep.grade)),
            texts: cur_mi.map(|rep| {
                (format_mi_button_label(rep), format_mi_popover_markup(rep, self_mi))
            }),
        }
    }

    fn apply(self) {
        let [btn, pop_label, refactor_btn] = self.widgets;
        for cls in ["btn-mi-a", "btn-mi-b", "btn-mi-c"] {
            remove_class(btn, cls);
        }
        unsafe { gtk_widget_set_sensitive(refactor_btn, i32::from(self.grade_cls.is_some())) };
        let Some((cls, (lbl, markup))) = self.grade_cls.zip(self.texts) else {
            unsafe {
                gtk_button_set_label(btn, text::MAINTAINABILITY.as_ptr());
                gtk_label_set_text(pop_label, text::MI_EMPTY.as_ptr());
            }
            return;
        };
        add_class(btn, cls);
        if let Ok(c) = CString::new(lbl) {
            unsafe { gtk_button_set_label(btn, c.as_ptr()) };
        }
        if let Ok(c) = CString::new(markup) {
            unsafe { gtk_label_set_markup(pop_label, c.as_ptr()) };
        }
    }
}

extern "C" fn on_mi_ready(_data: *mut c_void) -> c_int {
    refresh_ui();
    0
}

extern "C" fn on_git_status_ready(_data: *mut c_void) -> c_int {
    refresh_ui();
    refresh_open_git_popover();
    0
}

extern "C" fn on_mi_refactor_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let target = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let home = home_dir();
        let mi_map = app.mi_scheduler.snapshot();
        let active_cwd = app
            .sessions
            .iter()
            .find(|s| s.id == app.active_id)
            .and_then(TermSession::active_local_cwd);
        let (cur_mi, _) = resolve_active_mi(
            &mi_map,
            active_cwd.as_deref(),
            app.config.mi_root.as_deref(),
            &home,
        );
        Some((app.mi_pop, cur_mi.map(build_mi_refactor_prompt)))
    });
    if let Some((mi_pop, prompt)) = target {
        if !mi_pop.is_null() {
            unsafe { gtk_popover_popdown(mi_pop) };
        }
        if let Some(prompt) = prompt {
            send_to_active(&prompt);
        }
    }
}

fn collect_tokens_view(app: &AppState, now: Instant) -> (TokensMenu, TokensViewData) {
    (
        app.tokens_menu,
        build_sessions_tokens_view(&app.sessions, app.active_id, app.theme_mode, now),
    )
}

extern "C" fn on_tokens_open(_btn: *mut GtkWidget, _pop_ptr: *mut c_void) {
    let target = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        Some(collect_tokens_view(app, Instant::now()))
    });
    if let Some((menu, view)) = target {
        menu.apply(&view);
        if !menu.popover().is_null() {
            unsafe { gtk_popover_popup(menu.popover()) };
        }
    }
}

fn snapshot_telemetry_jobs() -> Vec<SessionPollJob> {
    APP.with(|cell| {
        let Ok(state) = cell.try_borrow() else { return Vec::new() };
        let Some(app) = state.as_ref() else { return Vec::new() };
        app.sessions.iter().map(SessionPollJob::from_session).collect()
    })
}

fn apply_telemetry_updates(updated: Vec<(usize, SessionTelemetry)>) {
    APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return };
        let Some(app) = state.as_mut() else { return };
        for (sid, telem) in updated {
            if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid) {
                s.telemetry = telem;
            }
        }
    });
}

fn poll_all_session_telemetry() {
    let jobs = snapshot_telemetry_jobs();
    if jobs.is_empty() {
        return;
    }
    let home = PathBuf::from(home_dir());
    let now = Instant::now();
    let updated = jobs.into_iter().map(|job| run_poll_job(job, &home, now)).collect();
    apply_telemetry_updates(updated);
}

fn session_git_badge(
    s: &TermSession,
    local_dir: Option<&str>,
    git_map: &HashMap<String, GitRepoStatus>,
    home: &str,
) -> String {
    local_dir
        .and_then(|dir| {
            lookup_git_status(git_map, dir, home)
                .or_else(|| lookup_git_status(git_map, &s.cwd, home))
        })
        .map_or_else(String::new, GitRepoStatus::dirty_badge)
}

fn prepare_footer_update(
    app: &AppState,
    working: usize,
    ready: usize,
    ssh_count: usize,
) -> Option<(*mut GtkWidget, String)> {
    if app.footer_hold_until.get().is_some_and(|until| Instant::now() < until) {
        return None;
    }
    app.footer_hold_until.set(None);
    let footer = format!(
        "Вкладки: {}    В работе: {}    Готово: {}    SSH: {}",
        app.sessions.len(),
        working,
        ready,
        ssh_count
    );
    Some((app.footer_status, footer))
}

struct UiRefreshPlan {
    rows: Vec<sidebar::RowUpdate>,
    tabs: Vec<tab_strip::TabUpdate>,
    sidebar: sidebar::SidebarUpdate,
    btns: [*mut GtkWidget; 3],
    flags: (bool, bool),
    git: (*mut GtkWidget, bool, Option<GitRepoStatus>),
    mi: MiUiPlan,
    tokens: (TokensMenu, TokensViewData),
    footer: Option<(*mut GtkWidget, String)>,
}

fn prepare_session_views(
    app: &AppState,
    mi_map: &HashMap<String, ProjectMiReport>,
    git_map: &HashMap<String, GitRepoStatus>,
    home: &str,
) -> (Vec<sidebar::RowUpdate>, Vec<tab_strip::TabUpdate>, Option<String>, bool, [usize; 3]) {
    let mut counts = [0usize; 3];
    let mut active_cwd = None;
    let mut can_reconnect = false;
    let mut rows = Vec::with_capacity(app.sessions.len());
    let mut tabs = Vec::with_capacity(app.sessions.len());
    for (idx, s) in app.sessions.iter().enumerate() {
        let is_act = s.id == app.active_id;
        let local_dir = s.active_local_cwd();
        if is_act {
            can_reconnect = s.remote().is_some();
            active_cwd = local_dir.clone();
        }
        counts[0] += usize::from(s.state == WorkState::Working);
        counts[1] += usize::from(s.unread);
        counts[2] += usize::from(!s.proc_info.ssh_host.is_empty());
        let mi_badge = local_dir
            .as_deref()
            .and_then(|d| lookup_mi(mi_map, d, home).or_else(|| lookup_mi(mi_map, &s.cwd, home)))
            .map(|r| (r.avg_mi, r.grade));
        let git_badge = session_git_badge(s, local_dir.as_deref(), git_map, home);
        rows.push(s.sidebar_row.prepare_update(idx, s, mi_badge, &git_badge, is_act));
        tabs.push(s.top_tab.prepare_update(s, is_act));
    }
    (rows, tabs, active_cwd, can_reconnect, counts)
}

impl UiRefreshPlan {
    fn build(app: &AppState) -> Self {
        let home = home_dir();
        let local_cwds = collect_local_cwds(app);
        app.mi_scheduler.sync_targets(&app.config.mi_directories(), &local_cwds, &home);
        app.git_scheduler.sync_targets(&local_cwds, &home);
        let mi_map = app.mi_scheduler.snapshot();
        let git_map = app.git_scheduler.snapshot();
        let (rows, tabs, active_cwd, can_rec, [working, ready, ssh_count]) =
            prepare_session_views(app, &mi_map, &git_map, &home);
        let active_git = active_cwd
            .as_deref()
            .and_then(|dir| lookup_git_status(&git_map, dir, &home))
            .cloned();
        let (cur_mi, self_mi) = resolve_active_mi(
            &mi_map,
            active_cwd.as_deref(),
            app.config.mi_root.as_deref(),
            &home,
        );
        Self {
            rows,
            tabs,
            sidebar: prepare_sidebar_filter(app),
            btns: [app.reconnect_btn, app.split_vert_btn, app.split_horiz_btn],
            flags: (can_rec, !app.sessions.is_empty() && !can_rec),
            git: (app.git_menu.button(), active_cwd.is_some(), active_git),
            mi: MiUiPlan::from_state(app, cur_mi, self_mi),
            tokens: collect_tokens_view(app, Instant::now()),
            footer: prepare_footer_update(app, working, ready, ssh_count),
        }
    }

    fn apply(self) {
        for row in self.rows {
            row.apply();
        }
        for tab in self.tabs {
            tab.apply();
        }
        self.sidebar.apply();
        let [reconnect_btn, split_vert_btn, split_horiz_btn] = self.btns;
        unsafe {
            gtk_widget_set_sensitive(reconnect_btn, i32::from(self.flags.0));
            gtk_widget_set_sensitive(split_vert_btn, i32::from(self.flags.1));
            gtk_widget_set_sensitive(split_horiz_btn, i32::from(self.flags.1));
        }
        update_git_button(self.git.0, self.git.1, self.git.2.as_ref());
        self.mi.apply();
        self.tokens.0.apply(&self.tokens.1);
        if let Some((lbl, text)) = self.footer {
            set_label_text(lbl, &text);
        }
    }
}

pub fn refresh_ui() {
    let plan = APP.with(|cell| {
        let Ok(borrow) = cell.try_borrow() else { return None };
        borrow.as_ref().map(UiRefreshPlan::build)
    });
    if let Some(plan) = plan {
        plan.apply();
    }
}

extern "C" fn on_radar_tick(_data: *mut c_void) -> c_int {
    let mut box_snap = BoxTelemetry::default();

    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            if let Ok(g) = app.box_state.lock() {
                box_snap = g.clone();
            }
            let now = Instant::now();
            let active = app.active_id;
            let win_active = sync_window_focus_urgency(app.win);
            for s in &mut app.sessions {
                sync_session_proc(s, Some(&box_snap));
                settle_working_session(s, now, active, app.win, win_active);
            }
            if !box_snap.gpu_lines.is_empty() {
                set_label_text(app.box_gpu_label, &box_snap.gpu_lines.join("\n"));
            }
            sync_tmux_jump_buttons(app, &box_snap.tmux_items);
        }
    });
    poll_all_session_telemetry();
    refresh_ui();
    save_tabs();
    1
}

fn sync_session_proc(s: &mut TermSession, telemetry: Option<&BoxTelemetry>) {
    s.apply_proc_snapshot(inspect_proc_tree(s.active_pid()));
    if let Some(box_snap) = telemetry {
        update_remote_proc_info(s, box_snap);
    } else if let Tab::Remote(remote) = &s.tab {
        if s.child_pid > 0 {
            s.proc_info.ssh_host.clone_from(&remote.host);
        }
    }
    refresh_smart_title(s);
}

fn update_remote_proc_info(s: &mut TermSession, telemetry: &BoxTelemetry) {
    let Tab::Remote(remote) = &s.tab else { return };
    if s.child_pid <= 0 {
        return;
    }
    s.proc_info.ssh_host.clone_from(&remote.host);
    if s.tmux_title_from_vte {
        return;
    }
    let Some(target) = &remote.tmux_session else { return };
    let client = telemetry.active_by_host.iter().find(|(host, name, _, _, _)| {
        host == &remote.host && name == target
    });
    if let Some((_, name, window, command, path)) = client {
        s.remote_tmux = Some((name.clone(), window.clone(), command.clone(), path.clone()));
    }
}

fn send_to_session(sid: usize, text: &str) {
    let Ok(c) = CString::new(text) else { return };
    let vte = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        app.sessions.iter().find(|s| s.id == sid).map(TermSession::active_vte)
    });
    if let Some(vte) = vte.filter(|w| !w.is_null()) {
        unsafe {
            vte_terminal_feed_child(vte, c.as_ptr(), text.len() as isize);
            gtk_widget_grab_focus(vte);
        }
    }
}

fn send_to_active(text: &str) {
    send_to_session(active_session_id(), text);
}

extern "C" fn on_sigint_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let target = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        Some((s.active_vte(), s.active_pid()))
    });
    if let Some((vte, pid)) = target.filter(|(w, _)| !w.is_null()) {
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
}

fn active_session_id() -> usize {
    APP.with(|cell| {
        cell.try_borrow().ok().and_then(|st| st.as_ref().map(|app| app.active_id)).unwrap_or(0)
    })
}

fn pick_session_vte(s: &TermSession, preferred: *mut GtkWidget) -> *mut GtkWidget {
    if !preferred.is_null() && s.has_vte(preferred) {
        preferred
    } else {
        s.active_vte()
    }
}

fn pane_cwd(s: &TermSession, pane_vte: *mut GtkWidget) -> String {
    s.panes
        .live_cwd_for_vte(pane_vte, s.vte, s.child_pid, &s.cwd)
        .or_else(|| s.active_local_cwd())
        .unwrap_or_else(|| s.cwd.clone())
}

fn local_split_context(sid_opt: Option<usize>, target_vte: *mut GtkWidget) -> Option<(usize, *mut GtkWidget, String, String, f64)> {
    APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let id = sid_opt.unwrap_or(app.active_id);
        let s = app.sessions.iter().find(|s| s.id == id && s.remote().is_none())?;
        let vte = pick_session_vte(s, target_vte);
        Some((s.id, vte, pane_cwd(s, vte), app.config.shell.clone(), app.font_scale))
    })
}

fn split_pane_in_session(sid_opt: Option<usize>, target_vte: *mut GtkWidget, orientation: c_int) {
    let Some((sid, pane_vte, cwd, shell, scale)) = local_split_context(sid_opt, target_vte) else { return };
    let (new_vte, new_pid) = create_terminal(&cwd, &[shell.as_str()], sid, scale);
    let new_scroll = wrap_vte_scroll(new_vte);
    let split_op = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let s = state.as_mut()?.sessions.iter_mut().find(|s| s.id == sid && s.remote().is_none())?;
        let chosen = if s.has_vte(pane_vte) { pane_vte } else { ptr::null_mut() };
        Some(s.panes.prepare_split_vte(chosen, s.scroll_wrap, s.vte, new_scroll, new_vte, new_pid, cwd, orientation))
    });
    finish_split(split_op, new_scroll, new_vte);
    refresh_ui();
    save_tabs();
}

fn split_active_tab(orientation: c_int) {
    let target = ptr::null_mut();
    split_pane_in_session(None, target, orientation);
}

extern "C" fn on_split_vert(_btn: *mut GtkWidget, _data: *mut c_void) {
    split_active_tab(GTK_ORIENTATION_HORIZONTAL);
}

extern "C" fn on_split_horiz(_btn: *mut GtkWidget, _data: *mut c_void) {
    split_active_tab(GTK_ORIENTATION_VERTICAL);
}

fn close_active_pane_or_tab() {
    let sid = active_session_id();
    if sid == 0 {
        return;
    }
    let close_op = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let s = state.as_mut()?.sessions.iter_mut().find(|s| s.id == sid)?;
        let op = s
            .panes
            .prepare_close_active(&mut s.scroll_wrap, &mut s.vte, &mut s.child_pid, &mut s.cwd)?;
        sync_session_proc(s, None);
        Some(op)
    });
    if let Some(op) = close_op {
        op.apply_focus(true);
        refresh_ui();
        save_tabs();
    } else {
        close_session(sid);
    }
}

extern "C" fn on_close_tab(_btn: *mut GtkWidget, data: *mut c_void) {
    let sid = data as usize;
    close_session(sid);
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

fn close_session(sid: usize) {
    let removed = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        let pos = app.sessions.iter().position(|s| s.id == sid)?;
        let removed = app.sessions.remove(pos);
        if sid == app.active_id {
            let neighbor = pos.min(app.sessions.len().saturating_sub(1));
            app.active_id = app.sessions.get(neighbor).map_or(0, |s| s.id);
        }
        Some((removed, app.pin_pop.clone()))
    });
    if let Some((s, pin_pop)) = removed {
        pin_pop.detach();
        context_menu::dismiss_rename_popover();
        teardown_session(s);
        select_session(active_session_id());
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
        if app.sessions[idx].id == keep_sid { idx += 1 } else { removed.push(app.sessions.remove(idx)) }
    }
    removed
}

fn close_other_sessions(keep_sid: usize) {
    let (drained, pin_pop) = APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return (Vec::new(), None) };
        let Some(app) = state.as_mut() else { return (Vec::new(), None) };
        (drain_other_sessions(app, keep_sid), Some(app.pin_pop.clone()))
    });
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
        // A stalled client may not process a hangup from the closed PTY.
        const SIGKILL: c_int = 9;
        unsafe {
            if kill(-pid, SIGKILL) != 0 {
                kill(pid, SIGKILL);
            }
        }
    }
}

extern "C" fn on_reconnect_active(_btn: *mut GtkWidget, _data: *mut c_void) {
    reconnect_session(active_session_id());
}

fn reconnect_session(sid: usize) {
    let details = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        Some((s.remote()?.clone(), s.cwd.clone(), app.font_scale, s.child_pid))
    });
    let Some((remote, cwd, scale, old_pid)) = details else { return };
    stop_ssh(old_pid);
    let args = remote.argv();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let (vte, pid) = create_terminal(&cwd, &argv, sid, scale);
    replace_terminal(sid, vte, pid, remote.title());
    select_session(sid);
}

fn replace_terminal(sid: usize, vte: *mut GtkWidget, pid: i32, title: String) {
    let swapped = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        let old_vte = std::mem::replace(&mut s.vte, vte);
        s.panes.record_focus(vte);
        s.child_pid = pid;
        s.proc_info = inspect_proc_tree(pid);
        s.remote_tmux = None;
        s.tmux_title_from_vte = false;
        s.osc_title.clear();
        s.base_title.clone_from(&title);
        s.title = title;
        refresh_smart_title(s);
        s.state = WorkState::Idle;
        s.unread = false;
        let now = Instant::now();
        s.last_output = now;
        s.work_started = now;
        s.work_started_at = None;
        s.last_turn_secs = None;
        s.telemetry = SessionTelemetry::default();
        Some((old_vte, s.scroll_wrap))
    });
    if let Some((old_vte, scroll_wrap)) = swapped {
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
}

fn dismiss_tab_menu() {
    let pop = APP.with(|cell| cell.borrow().as_ref().map(|app| app.box_pop));
    if let Some(pop) = pop.filter(|p| !p.is_null()) {
        unsafe { gtk_popover_popdown(pop) };
    }
}

extern "C" fn on_popup_popover(_btn: *mut GtkWidget, pop_ptr: *mut c_void) {
    if !pop_ptr.is_null() {
        unsafe { gtk_popover_popup(pop_ptr) };
    }
}

fn dismiss_agents_popover() {
    let pop = APP.with(|cell| cell.borrow().as_ref().map(|app| app.agents_pop));
    if let Some(pop) = pop.filter(|p| !p.is_null()) {
        unsafe { gtk_popover_popdown(pop) };
    }
}

extern "C" fn on_agent_cmd(_btn: *mut GtkWidget, code: *mut c_void) {
    let Some((_, command)) = text::AGENT_ACTIONS.get(code as usize) else {
        return;
    };
    dismiss_agents_popover();
    send_to_active(command);
}

fn active_worktree_context() -> Result<(String, String), String> {
    dismiss_agents_popover();
    APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref().ok_or_else(String::new)?;
        let err = || text::WORKTREE_LOCAL_ONLY.to_string_lossy().into_owned();
        let s = app.sessions.iter().find(|s| s.id == app.active_id).ok_or_else(err)?;
        let Tab::Local { ref profile, .. } = s.tab else { return Err(err()) };
        let key = if app.config.profile(profile).monitor {
            app.config.profile("shell").key
        } else {
            profile.clone()
        };
        let cwd = s.active_local_cwd().ok_or_else(err)?;
        Ok((key, cwd))
    })
}

fn set_footer_message(message: &str) {
    if message.is_empty() {
        return;
    }
    let footer = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        app.footer_hold_until.set(Some(Instant::now() + Duration::from_secs(4)));
        Some(app.footer_status)
    });
    if let Some(footer) = footer.filter(|w| !w.is_null()) {
        set_label_text(footer, message);
    }
}

pub(crate) fn find_live_drop_vte(
    sessions: &[TermSession],
    active_id: usize,
    sid: usize,
    target_vte: *mut GtkWidget,
) -> Option<(*mut GtkWidget, bool)> {
    if target_vte.is_null() {
        return None;
    }
    let session = sessions.iter().find(|s| s.id == sid && s.has_vte(target_vte))?;
    Some((target_vte, session.id == active_id))
}

enum PendingDropTarget {
    Ignore,
    Remote { sid: usize, host: String },
    CheckLive { sid: usize, pid: Option<i32>, cached_host: Option<String> },
}

fn snapshot_drop_target(s: &TermSession, vte: *mut GtkWidget) -> PendingDropTarget {
    if let Some(remote) = s.remote() {
        return PendingDropTarget::Remote { sid: s.id, host: remote.host.clone() };
    }
    let cached_host = (vte == s.active_vte() && s.effective_category() == SessionCategory::Remote)
        .then(|| s.proc_info.ssh_host.clone())
        .filter(|h| !h.is_empty());
    PendingDropTarget::CheckLive {
        sid: s.id,
        pid: Some(s.pid_for_vte(vte)).filter(|&p| p > 0),
        cached_host,
    }
}

pub(crate) fn lookup_vte_drop_target(vte: *mut GtkWidget) -> DropTarget {
    let pending = APP.with(|cell| {
        let Ok(state) = cell.try_borrow() else { return PendingDropTarget::Ignore };
        let Some(app) = state.as_ref() else {
            return PendingDropTarget::CheckLive { sid: 0, pid: None, cached_host: None };
        };
        app.sessions
            .iter()
            .find(|s| s.has_vte(vte))
            .map_or(PendingDropTarget::Ignore, |s| snapshot_drop_target(s, vte))
    });
    match pending {
        PendingDropTarget::Ignore => DropTarget::Ignore,
        PendingDropTarget::Remote { sid, host } => DropTarget::Remote { sid, host },
        PendingDropTarget::CheckLive { sid, pid, cached_host } => {
            let live = pid.map(inspect_proc_tree).map(|p| p.ssh_host).filter(|h| !h.is_empty());
            match live.or(cached_host) {
                Some(host) if sid > 0 => DropTarget::Remote { sid, host },
                _ => DropTarget::Local,
            }
        }
    }
}

pub(crate) fn apply_vte_drop_upload(done: UploadCompletion) {
    let target_vte = done.vte_addr as *mut GtkWidget;
    let resolved = APP.with(|cell| {
        let Ok(state) = cell.try_borrow() else { return None };
        let app = state.as_ref()?;
        let live = find_live_drop_vte(&app.sessions, app.active_id, done.sid, target_vte);
        Some((app.footer_status, live))
    });
    let Some((footer, live)) = resolved else { return };
    if !footer.is_null() && !done.footer_msg.is_empty() {
        set_label_text(footer, &done.footer_msg);
    }
    if let (Some((vte, focus)), Some(text)) = (live, done.injected_text.as_deref()) {
        feed_vte_text(vte, text, focus);
    }
}

pub(crate) fn install_file_drop_hooks() {
    register_file_drop_hooks(FileDropHooks {
        resolve_target: lookup_vte_drop_target,
        set_status: set_footer_message,
        complete_upload: apply_vte_drop_upload,
    });
}

extern "C" fn on_agent_worktree_cmd(_btn: *mut GtkWidget, code: *mut c_void) {
    let Some((_, command)) = text::AGENT_ACTIONS.get(code as usize) else { return };
    let (profile, cwd) = match active_worktree_context() {
        Ok(ctx) => ctx,
        Err(msg) => return set_footer_message(&msg),
    };
    let slug = agent_slug_from_cmd(command);
    match create_agent_worktree(Path::new(&cwd), &slug) {
        Ok(wt_path) => {
            let wt_str = wt_path.to_string_lossy().into_owned();
            let sid = open_local_tab(profile, &wt_str);
            send_to_session(sid, command);
            set_footer_message(&format!("⎇ Worktree {} · запущен {slug}", shorten_home(&wt_str)));
        }
        Err(err) => set_footer_message(&format!("⚠ {err}")),
    }
}

fn build_agents_popover(agents_btn: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let agents_pop = gtk_popover_new(agents_btn);
        let agents_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        add_class(agents_box, "popover-box");
        for (code, (label, _)) in text::AGENT_ACTIONS.iter().enumerate() {
            let b = gtk_button_new_with_label(label.as_ptr());
            connect_click(b, on_agent_cmd, code as *mut c_void);
            gtk_box_pack_start(agents_box, b, 0, 1, 0);
        }
        pack_worktree_agent_actions(agents_box);
        gtk_container_add(agents_pop, agents_box);
        gtk_widget_show_all(agents_box);
        connect_click(agents_btn, on_popup_popover, agents_pop);
        agents_pop
    }
}

fn pack_worktree_agent_actions(agents_box: *mut GtkWidget) {
    unsafe {
        let wt_hdr = gtk_label_new(text::AGENTS_WORKTREE_HEADER.as_ptr());
        gtk_label_set_xalign(wt_hdr, 0.0);
        add_class(wt_hdr, "section-header");
        gtk_box_pack_start(agents_box, wt_hdr, 0, 0, 0);
        let wt_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        for (code, (_, cmd)) in text::AGENT_ACTIONS.iter().enumerate() {
            let lbl = CString::new(format!("⎇ {}", agent_slug_from_cmd(cmd))).unwrap();
            let b = gtk_button_new_with_label(lbl.as_ptr());
            gtk_widget_set_tooltip_text(b, text::AGENT_WORKTREE_TIP.as_ptr());
            connect_click(b, on_agent_worktree_cmd, code as *mut c_void);
            gtk_box_pack_start(wt_row, b, 1, 1, 0);
        }
        gtk_box_pack_start(agents_box, wt_row, 0, 1, 0);
    }
}

extern "C" fn on_spawn_preset(_btn: *mut GtkWidget, index: *mut c_void) {
    dismiss_tab_menu();
    let tab = APP.with(|cell| {
        let state = cell.borrow();
        let profile = state.as_ref()?.config.profiles.get(index as usize)?;
        Some(Tab::local(&profile.key))
    });
    if let Some(tab) = tab {
        open_tab(tab);
    }
}

fn session_ssh_host(s: &TermSession) -> String {
    s.remote().map_or(&s.proc_info.ssh_host, |r| &r.host).clone()
}

fn capture_active_to_file() -> Option<PathBuf> {
    let ctx = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        let vte = s.active_vte();
        let cwd = s.active_local_cwd().unwrap_or_else(|| {
            if !s.proc_info.cwd.is_empty() { expand_home(&s.proc_info.cwd) } else { s.cwd.clone() }
        });
        Some((vte, s.title.clone(), cwd, session_ssh_host(s), app.config.briefs_dir.clone()))
    })?;
    let (vte, title, cwd, ssh_host, briefs_dir) = ctx;
    if vte.is_null() {
        return None;
    }
    let meta = CaptureMeta {
        title: &title,
        cwd: &cwd,
        ssh_host: &ssh_host,
        timestamp_secs: current_unix_secs(),
    };
    match save_vte_capture(vte, &meta, briefs_dir.as_deref()) {
        Ok(path) => Some(path),
        Err(err) => {
            set_footer_message(&format!("⚠ Ошибка сохранения снимка: {err}"));
            None
        }
    }
}

fn handoff_file_to_session(target_sid: usize, local_path: &Path) {
    let target_host = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        app.sessions.iter().find(|s| s.id == target_sid).map(session_ssh_host)
    });
    let Some(host) = target_host else { return };
    match resolve_handoff_delivery(local_path, &host) {
        Ok((prompt, status)) => {
            select_session(target_sid);
            send_to_session(target_sid, &prompt);
            set_footer_message(&status);
        }
        Err(err) => set_footer_message(&err),
    }
}

extern "C" fn on_handoff_open(_btn: *mut GtkWidget, _data: *mut c_void) {
    let prep = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut()?;
        let briefs_dir = app.config.briefs_dir.clone();
        let has_regular = app.sessions.iter().any(|s| s.category != SessionCategory::Daemon);
        let targets: Vec<HandoffTarget> = app
            .sessions
            .iter()
            .filter(|s| !has_regular || s.category != SessionCategory::Daemon)
            .map(|s| HandoffTarget {
                sid: s.id,
                title: s.title.clone(),
                ssh_host: session_ssh_host(s),
                is_active: s.id == app.active_id,
            })
            .collect();
        Some((app.handoff.take_widgets(), briefs_dir, targets))
    });
    let Some((widgets, briefs_dir, targets)) = prep else { return };
    let files = list_handoff_files(briefs_dir.as_deref());
    let target_ids: Vec<usize> = targets.iter().map(|t| t.sid).collect();
    let cbs = HandoffCallbacks {
        on_capture: on_handoff_capture,
        on_forward: on_handoff_forward,
        on_inject: on_handoff_inject,
    };
    let new_inner = rebuild_handoff_widgets(widgets, &files, &targets, cbs);
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.handoff.save_snapshot(new_inner, files, target_ids);
        }
    });
    if !widgets.popover.is_null() {
        unsafe { gtk_popover_popup(widgets.popover) };
    }
}

extern "C" fn on_handoff_capture(_btn: *mut GtkWidget, _data: *mut c_void) {
    let popover = APP.with(|cell| cell.borrow().as_ref().map(|app| app.handoff.popover()));
    if let Some(pop) = popover.filter(|p| !p.is_null()) {
        unsafe { gtk_popover_popdown(pop) };
    }
    if let Some(path) = capture_active_to_file() {
        let shown = shorten_home(&path.display().to_string());
        set_footer_message(&format!("📋 Вывод сохранён в {shown}"));
    }
}

extern "C" fn on_handoff_forward(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let action = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        Some((app.handoff.popover(), app.handoff.target_at(idx_ptr as usize)))
    });
    let Some((pop, target_sid)) = action else { return };
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
    if let (Some(sid), Some(saved_path)) = (target_sid, capture_active_to_file()) {
        handoff_file_to_session(sid, &saved_path);
    }
}

extern "C" fn on_handoff_inject(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let action = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        Some((app.handoff.popover(), app.active_id, app.handoff.file_at(idx_ptr as usize)))
    });
    let Some((pop, sid, chosen)) = action else { return };
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
    if let Some(path) = chosen {
        handoff_file_to_session(sid, &path);
    }
}

fn dismiss_git_popover() {
    let pop = APP.with(|cell| cell.try_borrow().ok()?.as_ref().map(|app| app.git_menu.popover()));
    if let Some(pop) = pop.filter(|p| !p.is_null()) {
        unsafe { gtk_popover_popdown(pop) };
    }
}

fn prepare_git_popover_state(
    force_refresh: bool,
) -> Result<(GitMenuWidgets, Option<(String, GitHeadInfo)>, Option<GitRepoStatus>), String> {
    APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut().ok_or_else(String::new)?;
        let local_err = || text::GIT_LOCAL_ONLY.to_string_lossy().into_owned();
        let active = app
            .sessions
            .iter()
            .find(|s| s.id == app.active_id)
            .ok_or_else(local_err)?;
        let cwd = active.active_local_cwd().ok_or_else(local_err)?;
        let repo = resolve_git_repo(&cwd, &home_dir());
        let status = repo.as_ref().and_then(|(root, _)| {
            let cached = app.git_scheduler.snapshot().remove(root);
            if force_refresh || cached.is_none() {
                app.git_scheduler.request_refresh(root);
            }
            cached
        });
        Ok((app.git_menu.take_widgets(), repo, status))
    })
}

fn populate_git_popover(force_refresh: bool, popup_after: bool) {
    let (widgets, repo, status) = match prepare_git_popover_state(force_refresh) {
        Ok(prep) => prep,
        Err(msg) => {
            set_footer_message(&msg);
            focus_active_vte();
            return;
        }
    };
    let repo_ref = repo.as_ref().map(|(r, h)| (r.as_str(), h));
    let cbs = GitMenuCallbacks {
        on_file: on_git_file_clicked,
        on_remove_wt: on_git_remove_worktree,
    };
    let (inner, entries) = rebuild_git_widgets(widgets, repo_ref, status.as_ref(), cbs);
    let pin_pop = APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.git_menu.save_snapshot(inner, entries);
            popup_after.then(|| app.pin_pop.clone())
        } else {
            None
        }
    });
    if let Some(pin_pop) = pin_pop {
        pin_pop.detach();
    }
    if popup_after {
        context_menu::dismiss_rename_popover();
        if !widgets.popover.is_null() {
            unsafe { gtk_popover_popup(widgets.popover) };
        }
    }
}

fn refresh_open_git_popover() {
    let pop = APP.with(|cell| cell.borrow().as_ref().map(|app| app.git_menu.popover()));
    let is_open = pop
        .filter(|p| !p.is_null())
        .is_some_and(|p| unsafe { gtk_widget_get_visible(p) != 0 });
    if is_open {
        populate_git_popover(false, false);
    }
}

fn toggle_git_popover() {
    let pop = APP.with(|cell| cell.borrow().as_ref().map(|app| app.git_menu.popover()));
    let is_open = pop
        .filter(|p| !p.is_null())
        .is_some_and(|p| unsafe { gtk_widget_get_visible(p) != 0 });
    if is_open {
        if let Some(pop) = pop {
            unsafe { gtk_popover_popdown(pop) };
        }
        focus_active_vte();
    } else {
        populate_git_popover(true, true);
    }
}

extern "C" fn on_git_open(_btn: *mut GtkWidget, _data: *mut c_void) {
    populate_git_popover(true, true);
}

extern "C" fn on_git_popover_key_press(
    _pop: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
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
    if keyval == 0xff1b || is_git_popover_shortcut(keyval, state, keycode) {
        dismiss_git_popover();
        focus_active_vte();
        return 1;
    }
    0
}

fn open_git_diff_split(repo_root: &str, rel_path: &str) {
    let local_ok = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        app.sessions.iter().find(|s| s.id == app.active_id)?.active_local_cwd()
    });
    let Some((sid, _, _, shell, scale)) = local_ok.and_then(|_| local_split_context(None, ptr::null_mut())) else {
        return set_footer_message(&text::GIT_LOCAL_ONLY.to_string_lossy());
    };
    let argv = build_diff_pane_argv(repo_root, rel_path, &shell);
    let (new_vte, new_pid) = create_terminal(repo_root, &argv, sid, scale);
    let new_scroll = wrap_vte_scroll(new_vte);
    let split_op = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let s = state.as_mut()?.sessions.iter_mut().find(|s| s.id == sid)?;
        Some(s.panes.prepare_split(
            s.scroll_wrap,
            s.vte,
            new_scroll,
            new_vte,
            new_pid,
            repo_root.to_owned(),
            GTK_ORIENTATION_HORIZONTAL,
        ))
    });
    finish_split(split_op, new_scroll, new_vte);
    refresh_ui();
    set_footer_message(&format!("⎇ Diff: {rel_path}"));
}

fn open_git_file_action(repo_root: &str, rel_path: &str, untracked: bool) {
    if untracked {
        let full = Path::new(repo_root).join(rel_path);
        open_external_link(&full.to_string_lossy());
        set_footer_message(&format!("📄 Открыт файл {rel_path}"));
        focus_active_vte();
    } else {
        open_git_diff_split(repo_root, rel_path);
    }
}

extern "C" fn on_git_file_clicked(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let chosen = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        Some((app.git_menu.popover(), app.git_menu.entry_at(idx_ptr as usize)))
    });
    let Some((pop, entry)) = chosen else { return };
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
    if let Some(item) = entry {
        open_git_file_action(&item.repo_root, &item.path, item.kind.is_untracked());
    }
}

struct WtRemoveDone {
    sid: usize,
    profile: String,
    cwd: String,
    outcome: Result<WorktreeRemovalOutcome, String>,
}

fn request_git_refresh_for_cwd(cwd: &str) {
    let Some((root, _)) = resolve_git_repo(cwd, &home_dir()) else { return };
    APP.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.git_scheduler.request_refresh(&root);
        }
    });
}

fn find_main_repo_tab(
    sessions: &[TermSession],
    skip_sid: usize,
    main_root: &str,
    home: &str,
) -> Option<usize> {
    sessions.iter().find_map(|s| {
        if s.id == skip_sid || s.remote().is_some() {
            return None;
        }
        let cwd = s.active_local_cwd()?;
        let (root, _) = resolve_git_repo(&cwd, home)?;
        (root == main_root).then_some(s.id)
    })
}

fn finish_worktree_removal(sid: usize, profile: &str, outcome: WorktreeRemovalOutcome) {
    let wt_str = outcome.wt_root.to_string_lossy().into_owned();
    let main_str = outcome.main_repo.to_string_lossy().into_owned();
    let nav = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        app.git_scheduler.remove_root(&wt_str);
        app.git_scheduler.request_refresh(&main_str);
        let has_tab = app.sessions.iter().any(|s| s.id == sid);
        let sole_tab = has_tab && app.sessions.len() == 1;
        let main_sid = find_main_repo_tab(&app.sessions, sid, &main_str, &home_dir());
        Some((has_tab, sole_tab, main_sid))
    });
    if let Some((has_tab, sole_tab, main_sid)) = nav {
        if sole_tab {
            open_local_tab(profile.to_owned(), &main_str);
            close_session(sid);
        } else if has_tab {
            close_session(sid);
            if let Some(target_sid) = main_sid {
                select_session(target_sid);
            }
        }
    }
    set_footer_message(&outcome.success_message());
}

extern "C" fn on_worktree_remove_done(data: *mut c_void) -> c_int {
    WT_REMOVE_BUSY.with(|b| b.set(false));
    if data.is_null() {
        return 0;
    }
    let done = unsafe { *Box::from_raw(data as *mut WtRemoveDone) };
    match done.outcome {
        Ok(outcome) => finish_worktree_removal(done.sid, &done.profile, outcome),
        Err(err) => {
            request_git_refresh_for_cwd(&done.cwd);
            set_footer_message(&err);
            focus_active_vte();
        }
    }
    0
}

fn start_remove_clean_worktree() {
    dismiss_git_popover();
    if WT_REMOVE_BUSY.with(|b| b.replace(true)) {
        return;
    }
    let ctx = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        let Tab::Local { ref profile, .. } = s.tab else { return None };
        let key = if app.config.profile(profile).monitor {
            app.config.profile("shell").key
        } else {
            profile.clone()
        };
        let cwd = s.active_local_cwd()?;
        Some((s.id, key, cwd))
    });
    let Some((sid, profile, cwd)) = ctx else {
        WT_REMOVE_BUSY.with(|b| b.set(false));
        set_footer_message(&text::GIT_LOCAL_ONLY.to_string_lossy());
        focus_active_vte();
        return;
    };
    focus_active_vte();
    thread::spawn(move || {
        let outcome = remove_clean_worktree(Path::new(&cwd));
        let payload = Box::new(WtRemoveDone { sid, profile, cwd, outcome });
        unsafe { g_idle_add(on_worktree_remove_done, Box::into_raw(payload) as *mut c_void) };
    });
}

extern "C" fn on_git_remove_worktree(_btn: *mut GtkWidget, _data: *mut c_void) {
    start_remove_clean_worktree();
}

struct PasteGuard;

impl Drop for PasteGuard {
    fn drop(&mut self) {
        PASTE_BUSY.with(|busy| busy.set(false));
    }
}

fn resolve_session_agent(s: &mut TermSession, target_vte: *mut GtkWidget) -> String {
    if s.effective_category() == SessionCategory::Remote {
        return s.proc_info.agent.clone();
    }
    let prev = s.proc_info.agent.clone();
    let live = detect_local_agent(s.pid_for_vte(target_vte));
    sync_session_proc(s, None);
    let fallback = if s.proc_info.agent.is_empty() { &prev } else { &s.proc_info.agent };
    resolve_pane_agent(live.as_deref(), target_vte == s.vte, fallback)
}

fn pane_paste_context(sid_opt: Option<usize>, preferred_vte: *mut GtkWidget) -> Option<(usize, *mut GtkWidget, String, String)> {
    APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        let target_id = sid_opt.unwrap_or(app.active_id);
        let s = app.sessions.iter_mut().find(|s| s.id == target_id)?;
        let target_vte = pick_session_vte(s, preferred_vte);
        s.panes.record_focus(target_vte);
        let agent = resolve_session_agent(s, target_vte);
        Some((s.id, target_vte, session_ssh_host(s), agent))
    })
}

fn session_paste_widgets(sid: usize, preferred_vte: *mut GtkWidget) -> Option<(*mut GtkWidget, *mut GtkWidget)> {
    APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        let vte = pick_session_vte(s, preferred_vte);
        (!vte.is_null()).then_some((vte, app.footer_status))
    })
}

fn finish_paste(vte: *mut GtkWidget, footer: *mut GtkWidget, image: Option<(String, String)>) {
    if vte.is_null() {
        return;
    }
    if let Some((path, status)) = image {
        if let Ok(c) = CString::new(path.as_str()) {
            unsafe { vte_terminal_feed_child(vte, c.as_ptr(), path.len() as isize) };
        }
        if !footer.is_null() {
            set_label_text(footer, &status);
        }
    } else {
        unsafe { vte_terminal_paste_clipboard(vte) };
    }
    unsafe { gtk_widget_grab_focus(vte) };
}

fn paste_routed_to(sid_opt: Option<usize>, preferred_vte: *mut GtkWidget, plain_ctrl_v: bool, force_image: bool) -> bool {
    if PASTE_BUSY.with(|busy| busy.replace(true)) {
        return !force_image;
    }
    let _guard = PasteGuard;
    let Some((sid, target_vte, host, agent)) = pane_paste_context(sid_opt, preferred_vte) else { return false };
    let has_image = plain_ctrl_v && clipboard_has_direct_image();
    if choose_paste_route(plain_ctrl_v, has_image, &host, &agent) == PasteRoute::PassThroughToPty {
        return false;
    }
    let image = try_paste_clipboard_image(&host, force_image);
    if image.is_none() && force_image {
        return false;
    }
    let Some((vte, footer)) = session_paste_widgets(sid, target_vte) else { return false };
    finish_paste(vte, footer, image);
    true
}

fn paste_smart(force_image: bool) -> bool {
    let target = ptr::null_mut();
    paste_routed_to(None, target, false, force_image)
}

fn paste_plain_ctrl_v() -> bool {
    let sid = active_session_id();
    paste_routed_to(Some(sid), ptr::null_mut(), true, false)
}

fn has_live_vte(vte: *mut GtkWidget) -> bool {
    if vte.is_null() {
        return false;
    }
    APP.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|st| st.as_ref().map(|a| a.sessions.iter().any(|s| s.has_vte(vte))))
            .unwrap_or(false)
    })
}

fn collect_forward_targets(app: &AppState, source_sid: usize) -> Vec<SessionTarget> {
    let has_regular = app.sessions.iter().any(|s| s.category != SessionCategory::Daemon);
    app.sessions
        .iter()
        .filter(|s| !has_regular || s.category != SessionCategory::Daemon)
        .map(|s| format_session_target(s.id, &s.title, &session_ssh_host(s), source_sid))
        .collect()
}

fn forward_pane_to_tab(source_sid: usize, source_vte: *mut GtkWidget, target_sid: usize, selected_text: Option<String>) {
    let ctx = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == source_sid)?;
        let pane_vte = pick_session_vte(s, source_vte);
        Some((pane_vte, s.title.clone(), pane_cwd(s, pane_vte), session_ssh_host(s), app.config.briefs_dir.clone()))
    });
    let Some((pane_vte, title, cwd, ssh_host, briefs_dir)) = ctx else { return };
    match save_forward_capture(briefs_dir.as_deref(), pane_vte, &title, &cwd, &ssh_host, selected_text) {
        Ok(path) => handoff_file_to_session(target_sid, &path),
        Err(msg) => set_footer_message(&msg),
    }
}

fn close_pane_in_session(sid: usize, target_vte: *mut GtkWidget) {
    let outcome = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        let active_id = app.active_id;
        let s = app.sessions.iter_mut().find(|s| s.id == sid && s.remote().is_none() && s.has_vte(target_vte))?;
        if !s.panes.has_splits() {
            return Some((false, None));
        }
        let op = s.panes.prepare_close_vte(target_vte, &mut s.scroll_wrap, &mut s.vte, &mut s.child_pid, &mut s.cwd)?;
        sync_session_proc(s, None);
        Some((s.id == active_id, Some(op)))
    });
    if let Some((is_active, maybe_op)) = outcome {
        match maybe_op {
            Some(op) => { op.apply_focus(is_active); refresh_ui(); save_tabs(); }
            None => close_session(sid),
        }
    }
}

fn duplicate_session_tab(sid: usize) {
    let dup = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let s = state.as_ref()?.sessions.iter().find(|s| s.id == sid)?;
        Some(match &s.tab {
            Tab::Local { profile, .. } => {
                Tab::Local { profile: profile.clone(), cwd: s.active_local_cwd().unwrap_or_else(|| s.cwd.clone()) }
            }
            Tab::Remote(remote) => Tab::Remote(remote.clone()),
        })
    });
    if let Some(tab) = dup {
        open_tab(tab);
    }
}

fn commit_tab_rename(sid: usize, new_title: String) {
    apply_pinned_tab_title(sid, session::sanitize_pinned_title(Some(&new_title)));
}

fn open_rename_for_tab(sid: usize, anchor: *mut GtkWidget) {
    dismiss_git_popover();
    let target = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        let row = s.sidebar_row.row_widget();
        let valid_anchor = if anchor == row { row } else { s.top_tab.select_button() };
        let initial = s.pinned_title.as_deref().unwrap_or(&s.title).to_owned();
        let pal = app.palette.is_open().then(|| app.palette.clone());
        Some((pal, app.pin_pop.clone(), valid_anchor, initial))
    });
    if let Some((pal, pin_pop, valid_anchor, current_title)) = target {
        if let Some(pal) = pal {
            pal.close();
        }
        pin_pop.detach();
        context_menu::popup_rename_popover(
            valid_anchor,
            sid,
            &current_title,
            commit_tab_rename,
            focus_active_vte,
        );
    }
}

fn clear_vte_buffer(vte: *mut GtkWidget) {
    if !has_live_vte(vte) {
        return;
    }
    unsafe {
        vte_terminal_reset(vte, 0, 1);
        gtk_widget_grab_focus(vte);
    }
}

fn copy_vte_selection(vte: *mut GtkWidget) {
    if !has_live_vte(vte) {
        return;
    }
    let _ = context_menu::copy_vte_selection_to_clipboard(vte, false);
    unsafe { gtk_widget_grab_focus(vte) };
}

fn execute_vte_action(action: ContextAction) -> Result<(), ContextAction> {
    match action {
        ContextAction::CopySelection(vte) => copy_vte_selection(vte),
        ContextAction::PasteSmart { sid, vte } => { let _ = paste_routed_to(Some(sid), vte, false, false); }
        ContextAction::OpenLink(url) => open_external_link(&url),
        ContextAction::CopyText(value) => context_menu::copy_text_to_clipboard(&value),
        ContextAction::ForwardToTab { source_sid, source_vte, target_sid, selected_text } => {
            forward_pane_to_tab(source_sid, source_vte, target_sid, selected_text);
        }
        ContextAction::SplitRight { sid, vte } => split_pane_in_session(Some(sid), vte, GTK_ORIENTATION_HORIZONTAL),
        ContextAction::SplitDown { sid, vte } => split_pane_in_session(Some(sid), vte, GTK_ORIENTATION_VERTICAL),
        ContextAction::ClosePane { sid, vte } => close_pane_in_session(sid, vte),
        ContextAction::ClearBuffer(vte) => clear_vte_buffer(vte),
        other => return Err(other),
    }
    Ok(())
}

fn execute_context_action(action: ContextAction) {
    let Err(tab_action) = execute_vte_action(action) else { return };
    match tab_action {
        ContextAction::RenameTab { sid, anchor } => open_rename_for_tab(sid, anchor),
        ContextAction::DuplicateTab(sid) => duplicate_session_tab(sid),
        ContextAction::ReconnectTab(sid) => reconnect_session(sid),
        ContextAction::CloseTab(sid) => close_session(sid),
        ContextAction::CloseOtherTabs(sid) => close_other_sessions(sid),
        _ => {}
    }
}

fn make_context_dispatcher() -> DispatchFn {
    let handler: DispatchFn = Rc::new(execute_context_action);
    handler
}

fn open_vte_context_menu(vte: *mut GtkWidget, event: *const GdkEvent, link_target: Option<String>) {
    let has_selection = !vte.is_null() && unsafe { vte_terminal_get_has_selection(vte) != 0 };
    let selected_text = if has_selection { context_menu::read_vte_selection(vte) } else { None };
    let spec = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        let idx = app.sessions.iter().position(|s| s.has_vte(vte))?;
        app.sessions[idx].panes.record_focus(vte);
        let sid = app.sessions[idx].id;
        let is_local = app.sessions[idx].remote().is_none();
        let targets = collect_forward_targets(app, sid);
        Some(VteMenuSpec { sid, vte, has_selection, selected_text, link_target, is_local, targets })
    });
    if let Some(spec) = spec {
        context_menu::popup_vte_menu(&spec, event, make_context_dispatcher());
    }
}

extern "C" fn on_tab_context_menu(widget: *mut GtkWidget, event: *const GdkEvent, data: *mut c_void) -> c_int {
    if !context_menu::is_right_button_press(event) {
        return 0;
    }
    let sid = data as usize;
    let spec = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        Some(TabMenuSpec { sid, anchor: widget, local_cwd: s.active_local_cwd(), is_remote: s.remote().is_some(), can_close_others: app.sessions.len() > 1 })
    });
    let Some(spec) = spec else { return 0 };
    context_menu::popup_tab_menu(&spec, event, make_context_dispatcher());
    1
}

fn copy_active_selection(clear_after: bool) -> bool {
    let vte = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        app.sessions.iter().find(|s| s.id == app.active_id).map(TermSession::active_vte)
    });
    vte.is_some_and(|w| context_menu::copy_vte_selection_to_clipboard(w, clear_after))
}

extern "C" fn on_paste_image_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let _ = paste_smart(true);
}

extern "C" fn on_window_key_press(
    _win: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    unsafe {
        let mut keyval: c_uint = 0;
        let mut state: c_uint = 0;
        let mut keycode: u16 = 0;
        if gdk_event_get_keyval(event, &mut keyval) == 0 {
            return 0;
        }
        gdk_event_get_state(event, &mut state);
        gdk_event_get_keycode(event, &mut keycode);

        if let Some(handled) = route_palette_key(keyval, state, keycode) {
            return handled;
        }
        if let Some(handled) = route_search_key(keyval, state, keycode) {
            return handled;
        }

        // F11 -> toggle window square / fullscreen
        if keyval == 0xffc8 {
            on_toggle_window_square(ptr::null_mut(), ptr::null_mut());
            return 1;
        }

        if let Some(idx) = tab_order::alt_tab_index(keyval, state) {
            let target_sid = APP.with(|cell| {
                cell.try_borrow().ok()?.as_ref()?.sessions.get(idx).map(|s| s.id)
            });
            if let Some(sid) = target_sid {
                select_session(sid);
                return 1;
            }
        }

        if (state & GDK_CONTROL_MASK) != 0 {
            // Zoom In: Ctrl + '+' / '=' / KP_Add / physical keycode 21 (=/+) or 86 (KP_Add)
            if keyval == b'+' as u32
                || keyval == b'=' as u32
                || keyval == 0xffab
                || keycode == 21
                || keycode == 86
            {
                adjust_zoom(0.1);
                return 1;
            }
            // Zoom Out: Ctrl + '-' / '_' / KP_Subtract / physical keycode 20 (-/_) or 82 (KP_Sub)
            if keyval == b'-' as u32
                || keyval == b'_' as u32
                || keyval == 0xffad
                || keycode == 20
                || keycode == 82
            {
                adjust_zoom(-0.1);
                return 1;
            }
            // Zoom Reset: Ctrl + '0' / KP_0 / physical keycode 19 (0)
            if (state & GDK_SHIFT_MASK) == 0
                && (keyval == b'0' as u32 || keyval == 0xffb0 || keycode == 19)
            {
                set_zoom(1.0);
                return 1;
            }
            if is_plain_ctrl_shortcut(state) {
                if is_c_key(keyval, keycode) {
                    return c_int::from(copy_active_selection(true));
                }
                if is_v_key(keyval, keycode) && paste_plain_ctrl_v() {
                    return 1;
                }
            }
        }

        if (state & GDK_CONTROL_MASK) != 0 && (state & GDK_SHIFT_MASK) != 0 {
            if handle_tab_shortcut(keyval, state, keycode) {
                return 1;
            }
            if is_ctrl_shift_shortcut(state) && is_c_key(keyval, keycode) {
                let _ = copy_active_selection(false);
                return 1;
            }
            if is_ctrl_shift_shortcut(state) && is_v_key(keyval, keycode) {
                let _ = paste_smart(false);
                return 1;
            }
        }
    }
    0
}

fn jump_next_attention() {
    let next_sid = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        find_next_session_attention(&app.sessions, app.active_id)
    });
    if let Some(sid) = next_sid {
        select_session(sid);
    }
}

fn handle_tab_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if let Some(delta) = tab_order::tab_reorder_delta(keyval, state, keycode) {
        move_active_tab(delta);
        return true;
    }
    if is_jump_attention_shortcut(keyval, keycode) {
        jump_next_attention();
        return true;
    }
    match (char::from_u32(keyval), keycode) {
        (Some('T' | 't'), _) | (_, 28) => open_default_tab(),
        (Some('W' | 'w'), _) | (_, 25) => close_active_pane_or_tab(),
        (Some('R' | 'r'), _) | (_, 27) => reconnect_session(active_session_id()),
        (Some('D' | 'd'), _) | (_, 40) => split_active_tab(GTK_ORIENTATION_HORIZONTAL),
        (Some('E' | 'e'), _) | (_, 26) => split_active_tab(GTK_ORIENTATION_VERTICAL),
        _ => return false,
    }
    true
}

fn focus_active_vte() {
    let vte = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        app.sessions.iter().find(|s| s.id == app.active_id).map(TermSession::active_vte)
    });
    if let Some(vte) = vte.filter(|w| !w.is_null()) {
        unsafe { gtk_widget_grab_focus(vte) };
    }
}

fn execute_palette_action(action: palette::PaletteAction) {
    match action {
        palette::PaletteAction::SelectTab(sid) => select_session(sid),
        palette::PaletteAction::JumpTmux { host, session } => jump_to_remote_tmux(&host, &session),
        palette::PaletteAction::OpenProfile(key) => {
            open_tab(Tab::local(&key));
        }
        palette::PaletteAction::OpenSsh { host, tmux_session } => {
            add_remote_session(&host, tmux_session.as_deref());
        }
        palette::PaletteAction::RunAgent(code) => {
            if let Some((_, command)) = text::AGENT_ACTIONS.get(code) {
                send_to_active(command);
            }
        }
        palette::PaletteAction::InjectHandoff(path) => {
            handoff_file_to_session(active_session_id(), &path);
        }
        palette::PaletteAction::ToggleTheme => toggle_theme(),
        palette::PaletteAction::OpenSearch => open_search_bar(),
        palette::PaletteAction::OpenGitMenu => populate_git_popover(true, true),
        palette::PaletteAction::GitFile { repo_root, path, untracked } => {
            open_git_file_action(&repo_root, &path, untracked);
        }
        palette::PaletteAction::RemoveCleanWorktree => start_remove_clean_worktree(),
    }
}

fn snapshot_active_panes() -> Option<(usize, Vec<*mut GtkWidget>, *mut GtkWidget)> {
    APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        let mut vtes = Vec::new();
        s.panes.collect_vtes(s.vte, &mut vtes);
        Some((s.id, vtes, s.panes.last_focus))
    })
}

fn record_pane_focus(sid: usize, vte: *mut GtkWidget) {
    if vte.is_null() {
        return;
    }
    APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return };
        let Some(app) = state.as_mut() else { return };
        if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid) {
            s.panes.record_focus(vte);
        }
    });
}

fn pick_focused_vte(vtes: &[*mut GtkWidget], last_focus: *mut GtkWidget) -> *mut GtkWidget {
    vtes.iter()
        .copied()
        .find(|&w| split_pane::is_widget_focused(w))
        .or_else(|| vtes.contains(&last_focus).then_some(last_focus))
        .or_else(|| vtes.first().copied())
        .unwrap_or(ptr::null_mut())
}

fn active_search_vte() -> *mut GtkWidget {
    let Some((sid, vtes, last_focus)) = snapshot_active_panes() else {
        return ptr::null_mut();
    };
    let chosen = pick_focused_vte(&vtes, last_focus);
    record_pane_focus(sid, chosen);
    chosen
}

fn collect_close_search_vtes() -> (Vec<*mut GtkWidget>, *mut GtkWidget) {
    let focus_vte = active_search_vte();
    let mut all = Vec::new();
    APP.with(|cell| {
        let Ok(state) = cell.try_borrow() else { return };
        if let Some(app) = state.as_ref() {
            for s in &app.sessions {
                s.panes.collect_vtes(s.vte, &mut all);
            }
        }
    });
    (all, focus_vte)
}

fn clone_search_bar() -> Option<search_bar::SearchBar> {
    APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        Some(state.as_ref()?.search_bar.clone())
    })
}

fn open_search_bar() {
    if let Some(bar) = clone_search_bar() {
        bar.open();
    }
}

fn toggle_search_bar() {
    context_menu::dismiss_rename_popover();
    dismiss_git_popover();
    let triple = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        Some((app.palette.clone(), app.pin_pop.clone(), app.search_bar.clone()))
    });
    if let Some((pal, pin_pop, bar)) = triple {
        if pal.is_open() {
            pal.close();
        }
        pin_pop.detach();
        bar.toggle();
    }
}

fn route_search_key(keyval: c_uint, state: c_uint, keycode: u16) -> Option<c_int> {
    if search_bar::is_search_shortcut(keyval, state, keycode) {
        toggle_search_bar();
        return Some(1);
    }
    let focused = clone_search_bar().is_some_and(|bar| bar.entry_has_focus());
    focused.then_some(0)
}

fn active_palette_git_state(app: &AppState) -> (Option<GitRepoStatus>, bool) {
    let Some(cwd) = app
        .sessions
        .iter()
        .find(|s| s.id == app.active_id)
        .and_then(TermSession::active_local_cwd)
    else {
        return (None, false);
    };
    let Some((root, head)) = resolve_git_repo(&cwd, &home_dir()) else {
        return (None, false);
    };
    let status = app.git_scheduler.snapshot().remove(&root);
    if status.is_none() {
        app.git_scheduler.request_refresh(&root);
    }
    (status, head.is_linked_worktree)
}

fn toggle_palette() {
    let snapshot = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let pal = app.palette.clone();
        let pin_pop = app.pin_pop.clone();
        if pal.is_open() {
            return Some((pal, pin_pop, None));
        }
        let tmux_items = app.box_state.lock().ok().map(|g| g.tmux_items.clone()).unwrap_or_default();
        let (active_git, active_is_worktree) = active_palette_git_state(app);
        let src = palette::PaletteSource {
            sessions: &app.sessions,
            tmux_items: &tmux_items,
            profiles: &app.config.profiles,
            ssh_hosts: &app.ssh_hosts,
            briefs_dir: app.config.briefs_dir.as_deref(),
            extra_handoffs: app.handoff.files(),
            active_git: active_git.as_ref(),
            active_is_worktree,
        };
        Some((pal, pin_pop, Some(palette::collect_items(&src))))
    });
    if let Some((pal, pin_pop, maybe_items)) = snapshot {
        match maybe_items {
            Some(items) => {
                context_menu::dismiss_rename_popover();
                pin_pop.detach();
                pal.open(items);
            }
            None => pal.close(),
        }
    }
}

fn is_overlay_open() -> bool {
    APP.with(|cell| {
        cell.try_borrow()
            .ok()
            .and_then(|g| g.as_ref().map(|app| app.palette.is_open() || app.pin_pop.is_open()))
            .unwrap_or(false)
    })
}

fn route_palette_key(keyval: c_uint, state: c_uint, keycode: u16) -> Option<c_int> {
    if context_menu::is_rename_open() {
        return Some(0);
    }
    if palette::is_palette_shortcut(keyval, state, keycode) {
        dismiss_git_popover();
        toggle_palette();
        return Some(1);
    }
    if theme_mode::is_theme_toggle_shortcut(keyval, state, keycode) {
        toggle_theme();
        return Some(1);
    }
    if search_bar::is_search_shortcut(keyval, state, keycode) {
        toggle_search_bar();
        return Some(1);
    }
    if is_git_popover_shortcut(keyval, state, keycode) {
        let overlays = APP.with(|cell| {
            let st = cell.try_borrow().ok()?;
            let a = st.as_ref()?;
            Some((a.palette.is_open().then(|| a.palette.clone()), a.pin_pop.clone()))
        });
        if let Some((pal, pin_pop)) = overlays {
            if let Some(pal) = pal {
                pal.close();
            }
            pin_pop.detach();
        }
        toggle_git_popover();
        return Some(1);
    }
    is_overlay_open().then_some(0)
}

fn open_default_tab() {
    dismiss_tab_menu();
    let tab = APP.with(|cell| cell.borrow().as_ref().map(|app| app.config.new_tab()));
    if let Some(tab) = tab {
        open_tab(tab);
    }
}

extern "C" fn on_new_tab(_btn: *mut GtkWidget, _data: *mut c_void) {
    open_default_tab();
}

fn make_section_header(title: &str) -> *mut GtkWidget {
    let c = CString::new(title).unwrap();
    unsafe {
        let lbl = gtk_label_new(c.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-header");
        lbl
    }
}

fn connect_click(btn: *mut GtkWidget, cb: extern "C" fn(*mut GtkWidget, *mut c_void), data: *mut c_void) {
    let sig = CString::new("clicked").unwrap();
    unsafe {
        g_signal_connect_data(btn, sig.as_ptr(), cb as *const c_void, data, ptr::null(), 0);
    }
}

fn connect_entry_changed(entry: *mut GtkWidget, cb: extern "C" fn(*mut GtkWidget, *mut c_void)) {
    let sig = CString::new("changed").unwrap();
    unsafe {
        g_signal_connect_data(
            entry,
            sig.as_ptr(),
            cb as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );
    }
}

fn make_search_entry(placeholder: &str) -> *mut GtkWidget {
    let c = CString::new(placeholder).unwrap();
    unsafe {
        let entry = gtk_entry_new();
        gtk_entry_set_placeholder_text(entry, c.as_ptr());
        add_class(entry, "search-entry");
        entry
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

fn spawn_one_secondary_pane(
    sid: usize,
    raw_cwd: &str,
    fallback_cwd: &str,
    shell: &str,
    scale: f64,
) -> split_pane::SpawnedPane {
    let cwd = split_pane::resolve_pane_cwd(raw_cwd, fallback_cwd);
    let (vte, child_pid) = create_terminal(&cwd, &[shell], sid, scale);
    let scroll = wrap_vte_scroll(vte);
    split_pane::SpawnedPane { scroll, vte, child_pid, cwd }
}

fn spawn_secondary_panes(
    sid: usize,
    tree: &tabs::SplitTree,
    fallback_cwd: &str,
    shell: &str,
    scale: f64,
) -> (Vec<split_pane::SpawnedPane>, Vec<*mut GtkWidget>) {
    let mut raw_cwds = Vec::new();
    tree.secondary_cwds(&mut raw_cwds);
    let mut spawned = Vec::with_capacity(raw_cwds.len());
    let mut scrolls = Vec::with_capacity(raw_cwds.len());
    for raw in raw_cwds {
        let pane = spawn_one_secondary_pane(sid, &raw, fallback_cwd, shell, scale);
        scrolls.push(pane.scroll);
        spawned.push(pane);
    }
    (spawned, scrolls)
}

fn restore_session_splits(sid: usize, tree: &tabs::SplitTree) {
    let ctx = APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == sid && s.category == SessionCategory::Project)?;
        s.remote().is_none().then(|| (s.cwd.clone(), app.config.shell.clone(), app.font_scale))
    });
    let Some((fallback_cwd, shell, scale)) = ctx else { return };
    let (spawned, raw_scrolls) = spawn_secondary_panes(sid, tree, &fallback_cwd, &shell, scale);
    let op = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let s = state.as_mut()?.sessions.iter_mut().find(|s| s.id == sid)?;
        s.panes.prepare_restore_splits(s.scroll_wrap, s.vte, tree.clone(), spawned)
    });
    split_pane::finish_restore_splits(op, &raw_scrolls);
}

fn apply_restored_meta(sid: usize, pinned: Option<&str>, splits: Option<&tabs::SplitTree>) {
    if let Some(pinned) = pinned {
        APP.with(|cell| {
            let Ok(mut state) = cell.try_borrow_mut() else { return };
            if let Some(s) = state.as_mut().and_then(|app| app.sessions.iter_mut().find(|s| s.id == sid)) {
                s.set_pinned_title(Some(pinned));
            }
        });
    }
    if let Some(tree) = splits {
        restore_session_splits(sid, tree);
    }
}

fn restore_tabs() {
    let (store, saved) = TabStore::load();
    let state = saved.unwrap_or_else(|| {
        APP.with(|cell| default_tabs(&cell.borrow().as_ref().expect("app is initialized").config))
    });
    let mut ids = Vec::with_capacity(state.tabs.len());
    for (idx, tab) in state.tabs.iter().cloned().enumerate() {
        let sid = open_tab(tab);
        apply_restored_meta(sid, state.pinned_at(idx), state.splits_at(idx));
        ids.push(sid);
    }
    select_session(ids.get(state.active).copied().unwrap_or(0));
    APP.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut() {
            if let Some(app) = guard.as_mut() {
                app.tab_store = Some(store);
            }
        }
    });
    save_tabs();
}

fn save_tabs() {
    APP.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else { return };
        let Some(app) = state.as_mut() else { return };
        let Some(store) = &mut app.tab_store else { return };
        let active = app.sessions.iter().position(|s| s.id == app.active_id).unwrap_or(0);
        let mut tabs = Vec::with_capacity(app.sessions.len());
        let mut meta = Vec::with_capacity(app.sessions.len());
        for s in &mut app.sessions {
            tabs.push(s.snapshot_tab());
            meta.push(s.snapshot_meta());
        }
        store.checkpoint(TabsState::with_meta(active, tabs, meta));
    });
}

fn main() -> std::process::ExitCode {
    let _workspace = match tabs::lock_workspace() {
        Ok(file) => file,
        Err(error) => {
            eprintln!("AkhTerm: cannot lock workspace: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    run_app();
    std::process::ExitCode::SUCCESS
}

unsafe fn add_toolbar_action(
    toolbar: *mut GtkWidget,
    label: &CStr,
    tooltip: &CStr,
    on_click: extern "C" fn(*mut GtkWidget, *mut c_void),
) -> *mut GtkWidget {
    let button = gtk_button_new_with_label(label.as_ptr());
    gtk_widget_set_tooltip_text(button, tooltip.as_ptr());
    connect_click(button, on_click, ptr::null_mut());
    gtk_box_pack_end(toolbar, button, 0, 0, 0);
    button
}

fn run_app() {
    platform::init_process_identity();
    let _ui_font = ui_font::load().inspect_err(|error| eprintln!("AkhTerm: {error}"));
    register_link_cwd_lookup(lookup_vte_link_cwd);
    install_file_drop_hooks();
    register_vte_menu_hook(open_vte_context_menu);
    let config = WorkspaceConfig::load();
    let initial_theme = theme_mode::load_initial_theme(config.theme);
    let mi_dirs = config.mi_directories();
    unsafe {
        gtk_init(ptr::null_mut(), ptr::null_mut());
        let display_backend = platform::detect_display_backend();
        let desktop_env = platform::detect_desktop_environment();
        theme::apply_gtk_theme_mode(initial_theme);
        const SIGTERM: c_int = 15;
        g_unix_signal_add(SIGTERM, on_terminate, ptr::null_mut());

        let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        gtk_window_set_title(win, text::APP_NAME.as_ptr());
        platform::configure_window_identity(win, display_backend, desktop_env);
        gtk_window_set_decorated(win, 1);
        gtk_window_set_resizable(win, 1);
        gtk_window_set_default_size(win, 1480, 900);
        add_class(win, "main-bg");

        let sig_destroy = CString::new("destroy").unwrap();
        g_signal_connect_data(
            win,
            sig_destroy.as_ptr(),
            on_window_destroy as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );
        let sig_key = CString::new("key-press-event").unwrap();
        g_signal_connect_data(
            win,
            sig_key.as_ptr(),
            on_window_key_press as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );

        let root_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_container_add(win, root_vbox);

        let header = gtk_header_bar_new();
        add_class(header, "window-header");
        let brand = gtk_label_new(text::APP_NAME.as_ptr());
        add_class(brand, "brand-label");
        gtk_header_bar_set_custom_title(header, brand);
        gtk_header_bar_set_show_close_button(header, 1);
        gtk_window_set_titlebar(win, header);

        let topbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 5);
        add_class(topbar, "topbar");
        gtk_box_pack_start(root_vbox, topbar, 0, 1, 0);
        let tab_strip = TabStrip::new(root_vbox, on_new_tab);

        let box_btn = gtk_button_new_with_label(text::PROFILES.as_ptr());
        gtk_box_pack_start(topbar, box_btn, 0, 0, 0);

        let side_tog_btn = add_toolbar_action(
            topbar,
            text::TOGGLE_SIDEBAR,
            text::TOGGLE_SIDEBAR_TIP,
            on_toggle_sidebar,
        );
        add_class(side_tog_btn, "btn-icon-sq");
        let theme_btn = add_toolbar_action(
            topbar,
            initial_theme.toggle_icon(),
            initial_theme.toggle_tooltip(),
            on_toggle_theme,
        );
        add_class(theme_btn, "btn-icon-sq");
        let split_horiz_btn =
            add_toolbar_action(topbar, text::SPLIT_HORIZ, text::SPLIT_HORIZ_TIP, on_split_horiz);
        add_class(split_horiz_btn, "btn-icon-sq");
        let split_vert_btn =
            add_toolbar_action(topbar, text::SPLIT_VERT, text::SPLIT_VERT_TIP, on_split_vert);
        add_class(split_vert_btn, "btn-icon-sq");
        let reconnect_btn = add_toolbar_action(
            topbar,
            text::RECONNECT,
            text::RECONNECT_TIP,
            on_reconnect_active,
        );
        add_toolbar_action(topbar, text::INTERRUPT, text::INTERRUPT_TIP, on_sigint_clicked);
        add_toolbar_action(topbar, text::PASTE_IMAGE, text::PASTE_IMAGE_TIP, on_paste_image_clicked);

        let zoom_in_btn = add_toolbar_action(topbar, text::ZOOM_IN, text::ZOOM_IN_TIP, on_zoom_in);
        add_class(zoom_in_btn, "btn-icon-sq");
        let zoom_btn = add_toolbar_action(topbar, text::ZOOM_RESET, text::ZOOM_RESET_TIP, on_zoom_reset);
        let zoom_out_btn = add_toolbar_action(topbar, text::ZOOM_OUT, text::ZOOM_OUT_TIP, on_zoom_out);
        add_class(zoom_out_btn, "btn-icon-sq");

        let settings_btn = gtk_button_new_with_label(text::SERVERS.as_ptr());
        gtk_box_pack_end(topbar, settings_btn, 0, 0, 0);

        let handoff_btn = gtk_button_new_with_label(text::HANDOFF.as_ptr());
        gtk_widget_set_tooltip_text(handoff_btn, text::HANDOFF_TIP.as_ptr());
        gtk_box_pack_end(topbar, handoff_btn, 0, 0, 0);

        let agents_btn = gtk_button_new_with_label(text::AGENTS.as_ptr());
        gtk_box_pack_end(topbar, agents_btn, 0, 0, 0);

        let git_btn = gtk_button_new_with_label(text::GIT_MENU.as_ptr());
        gtk_widget_set_tooltip_text(git_btn, text::GIT_MENU_TIP.as_ptr());
        gtk_box_pack_end(topbar, git_btn, 0, 0, 0);

        let mi_btn = gtk_button_new_with_label(text::MAINTAINABILITY.as_ptr());
        gtk_box_pack_end(topbar, mi_btn, 0, 0, 0);

        let tokens_menu = TokensMenu::new(topbar, on_tokens_open);

        // Popover: Maintainability Index (MI)
        let mi_pop = gtk_popover_new(mi_btn);
        let mi_pop_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(mi_pop_box, "popover-box");
        let mi_pop_label = gtk_label_new(text::MI_EMPTY.as_ptr());
        gtk_label_set_xalign(mi_pop_label, 0.0);
        gtk_box_pack_start(mi_pop_box, mi_pop_label, 0, 1, 0);
        let mi_refactor_btn = gtk_button_new_with_label(text::MI_REFACTOR.as_ptr());
        gtk_widget_set_tooltip_text(mi_refactor_btn, text::MI_REFACTOR_TIP.as_ptr());
        gtk_widget_set_sensitive(mi_refactor_btn, 0);
        connect_click(mi_refactor_btn, on_mi_refactor_clicked, ptr::null_mut());
        gtk_box_pack_start(mi_pop_box, mi_refactor_btn, 0, 1, 0);
        gtk_container_add(mi_pop, mi_pop_box);
        gtk_widget_show_all(mi_pop_box);
        connect_click(mi_btn, on_popup_popover, mi_pop);

        // Popover: Agents
        let agents_pop = build_agents_popover(agents_btn);

        // Popover: Settings (SSH Servers from ~/.ssh/config) with live Search + GtkScrolledWindow
        let ssh_hosts = load_ssh_hosts(&config.watch, &config.startup);
        let mut ssh_toggle_btns = Vec::new();
        let mut ssh_rows = Vec::new();
        let settings_pop = gtk_popover_new(settings_btn);
        let settings_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(settings_vbox, "popover-box");
        gtk_box_pack_start(
            settings_vbox,
            make_section_header("УДАЛЁННЫЕ СЕРВЕРЫ ИЗ ~/.SSH/CONFIG (ОТСЛЕЖИВАНИЕ TMUX/GPU)"),
            0,
            0,
            0,
        );

        let ssh_search_entry = make_search_entry("🔍 Поиск сервера, хоста или IP в ~/.ssh/config...");
        connect_entry_changed(ssh_search_entry, on_ssh_search_changed);
        gtk_box_pack_start(settings_vbox, ssh_search_entry, 0, 1, 0);

        let count_init = format!(
            "Показано серверов: {} из {}  (клик [✓] — опрос tmux/GPU, клик 🖥 — войти)",
            ssh_hosts.len(),
            ssh_hosts.len()
        );
        let ssh_count_label = gtk_label_new(CString::new(count_init).unwrap().as_ptr());
        gtk_label_set_xalign(ssh_count_label, 0.0);
        add_class(ssh_count_label, "ssh-count");
        gtk_box_pack_start(settings_vbox, ssh_count_label, 0, 1, 0);

        let settings_scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(settings_scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(settings_scroll, 520);
        gtk_scrolled_window_set_min_content_height(settings_scroll, 420);
        gtk_scrolled_window_set_max_content_height(settings_scroll, 460);
        gtk_widget_set_size_request(settings_scroll, 520, 420);

        let settings_list_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        for (idx, h) in ssh_hosts.iter().enumerate() {
            let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
            let mark = if h.enabled { "✓" } else { "  " };
            let sub = if h.hostname.is_empty() {
                String::new()
            } else {
                format!(" ({})", h.hostname)
            };
            let lbl = format!("[{mark}]  {}{sub}", h.alias);
            let b_tog = gtk_button_new_with_label(CString::new(lbl).unwrap().as_ptr());
            if h.enabled {
                add_class(b_tog, "tab-unread");
            }
            connect_click(b_tog, on_toggle_ssh_host, idx as *mut c_void);
            ssh_toggle_btns.push(b_tog);

            let b_conn = gtk_button_new_with_label(
                CString::new(format!("🖥 tmux: {}", h.alias)).unwrap().as_ptr(),
            );
            connect_click(b_conn, on_connect_ssh_host_tmux, idx as *mut c_void);

            gtk_box_pack_start(row, b_tog, 1, 1, 0);
            gtk_box_pack_end(row, b_conn, 0, 0, 0);
            gtk_box_pack_start(settings_list_vbox, row, 0, 1, 0);

            let search_key = format!("{} {} {}", h.alias, h.hostname, h.user).to_lowercase();
            ssh_rows.push((search_key, row));
        }
        gtk_container_add(settings_scroll, settings_list_vbox);
        gtk_box_pack_start(settings_vbox, settings_scroll, 1, 1, 0);
        gtk_container_add(settings_pop, settings_vbox);
        gtk_widget_show_all(settings_vbox);
        connect_click(settings_btn, on_popup_popover, settings_pop);

        // Popover: New Tab + SSH Box (with Search + GtkScrolledWindow for tmux & ~/.ssh/config)
        let mut box_ssh_rows = Vec::new();
        let box_pop = gtk_popover_new(box_btn);
        let box_pop_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 5);
        add_class(box_pop_vbox, "popover-box");
        let box_gpu_label = gtk_label_new(CString::new("GPU: опрос выбранных SSH-серверов...").unwrap().as_ptr());
        add_class(box_gpu_label, "gpu-pill");
        gtk_box_pack_start(box_pop_vbox, box_gpu_label, 0, 1, 2);
        for (index, profile) in config.profiles.iter().enumerate() {
            let label = format!("+ {} {}", profile.icon, profile.title);
            let b = gtk_button_new_with_label(CString::new(label).unwrap().as_ptr());
            connect_click(b, on_spawn_preset, index as *mut c_void);
            gtk_box_pack_start(box_pop_vbox, b, 0, 1, 0);
        }

        let box_search_entry = make_search_entry("🔍 Поиск сессии tmux или сервера из ~/.ssh/config...");
        connect_entry_changed(box_search_entry, on_box_search_changed);
        gtk_box_pack_start(box_pop_vbox, box_search_entry, 0, 1, 2);

        let box_scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(box_scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(box_scroll, 480);
        gtk_scrolled_window_set_min_content_height(box_scroll, 320);
        gtk_scrolled_window_set_max_content_height(box_scroll, 380);
        gtk_widget_set_size_request(box_scroll, 480, 320);

        let box_scroll_inner = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        gtk_box_pack_start(
            box_scroll_inner,
            make_section_header("ЖИВЫЕ СЕССИИ TMUX НА СЕРВЕРАХ (КЛИК — ПРЫГНУТЬ)"),
            0,
            0,
            0,
        );
        let box_tmux_pop_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 3);
        gtk_box_pack_start(box_scroll_inner, box_tmux_pop_box, 0, 1, 0);

        gtk_box_pack_start(
            box_scroll_inner,
            make_section_header("СЕРВЕРЫ SSH И TMUX"),
            0,
            0,
            0,
        );
        for (idx, h) in ssh_hosts.iter().enumerate() {
            let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
            let label = format!("ssh {}", h.alias);
            let plain = gtk_button_new_with_label(CString::new(label).unwrap().as_ptr());
            let tmux = gtk_button_new_with_label(text::TMUX_MAIN.as_ptr());
            connect_click(plain, on_connect_ssh_host_plain, idx as *mut c_void);
            connect_click(tmux, on_connect_ssh_host_tmux, idx as *mut c_void);
            gtk_box_pack_start(row, plain, 1, 1, 0);
            gtk_box_pack_end(row, tmux, 0, 0, 0);
            gtk_box_pack_start(box_scroll_inner, row, 0, 1, 0);
            let search_key = format!("{} {} {}", h.alias, h.hostname, h.user).to_lowercase();
            box_ssh_rows.push((search_key, row));
        }

        gtk_container_add(box_scroll, box_scroll_inner);
        gtk_box_pack_start(box_pop_vbox, box_scroll, 1, 1, 0);
        gtk_container_add(box_pop, box_pop_vbox);
        gtk_widget_show_all(box_pop_vbox);
        connect_click(box_btn, on_popup_popover, box_pop);

        // Popover: Git Changed Files & Clean Worktree Removal
        let git_menu = GitMenu::new(git_btn, on_git_open, on_git_popover_key_press);

        // Popover: Handoff Briefs & Terminal Capture
        let handoff = HandoffMenu::new(handoff_btn, on_handoff_open);

        // Workspace: Left Sidebar + Right Terminal Stack
        let workspace_hbox = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
        gtk_box_pack_start(root_vbox, workspace_hbox, 1, 1, 0);

        let sidebar = Sidebar::new(workspace_hbox, on_sidebar_search_changed);

        let term_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_box_pack_start(workspace_hbox, term_vbox, 1, 1, 0);
        let search_bar = search_bar::SearchBar::new(active_search_vte, collect_close_search_vtes);
        gtk_box_pack_start(term_vbox, search_bar.widget(), 0, 1, 0);

        let stack = gtk_stack_new();
        gtk_box_pack_start(term_vbox, stack, 1, 1, 0);

        let footer_box = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        add_class(footer_box, "footer-bar");
        let footer_status = gtk_label_new(CString::new("Запуск ядра VTE-2.91...").unwrap().as_ptr());
        gtk_label_set_xalign(footer_status, 0.0);
        gtk_box_pack_start(footer_box, footer_status, 1, 1, 0);
        gtk_box_pack_start(root_vbox, footer_box, 0, 1, 0);

        let initial_enabled: Vec<String> = ssh_hosts
            .iter()
            .filter(|h| h.enabled)
            .map(|h| h.alias.clone())
            .collect();
        let enabled_hosts_shared = Arc::new(Mutex::new(initial_enabled));
        let mi_scheduler = MiScheduler::default();
        mi_scheduler.sync_targets(&mi_dirs, &[], &home_dir());
        let git_scheduler = GitStatusScheduler::default();
        let box_state: Arc<Mutex<BoxTelemetry>> = Arc::new(Mutex::new(BoxTelemetry::default()));
        let cmd_palette = palette::CommandPalette::new(topbar, execute_palette_action, focus_active_vte);
        let pin_pop = tab_order::TabPinPopover::new(topbar, apply_pinned_tab_title, focus_active_vte);

        APP.with(|cell| {
            *cell.borrow_mut() = Some(AppState {
                config,
                win,
                is_fullscreen: false,
                sidebar,
                sidebar_visible: true,
                theme_mode: initial_theme,
                theme_btn,
                font_scale: 1.0,
                zoom_btn,
                reconnect_btn,
                split_vert_btn,
                split_horiz_btn,
                next_id: 1,
                active_id: 1,
                sessions: Vec::new(),
                tab_store: None,
                stack,
                tab_strip,
                mi_btn,
                mi_pop,
                mi_pop_label,
                mi_refactor_btn,
                tokens_menu,
                agents_pop,
                box_pop,
                box_gpu_label,
                box_tmux_pop_box,
                settings_pop,
                ssh_count_label,
                git_menu,
                handoff,
                footer_status,
                footer_hold_until: Cell::new(None),
                ssh_hosts,
                ssh_toggle_btns,
                ssh_rows,
                box_ssh_rows,
                sidebar_filter: String::new(),
                box_filter: String::new(),
                enabled_hosts_shared: Arc::clone(&enabled_hosts_shared),
                tmux_jump_btns: Vec::new(),
                last_tmux_keys: Vec::new(),
                mi_scheduler: mi_scheduler.clone(),
                git_scheduler: git_scheduler.clone(),
                box_state: Arc::clone(&box_state),
                palette: cmd_palette,
                search_bar,
                pin_pop,
            });
        });

        {
            let mi_bg = mi_scheduler.clone();
            thread::spawn(move || loop {
                if mi_bg.refresh_due(Instant::now()) > 0 {
                    g_idle_add(on_mi_ready, ptr::null_mut());
                }
                mi_bg.wait_for_turn(Duration::from_secs(5));
            });
        }

        {
            let git_bg = git_scheduler.clone();
            thread::spawn(move || loop {
                if git_bg.refresh_due(Instant::now()) > 0 {
                    g_idle_add(on_git_status_ready, ptr::null_mut());
                }
                git_bg.wait_for_turn(Duration::from_secs(4));
            });
        }

        {
            let box_bg = Arc::clone(&box_state);
            let hosts_bg = Arc::clone(&enabled_hosts_shared);
            thread::spawn(move || {
                loop {
                    let hosts_now = hosts_bg.lock().ok().map(|g| g.clone()).unwrap_or_default();
                    let telem = query_ssh_servers(&hosts_now);
                    if let Ok(mut g) = box_bg.lock() {
                        *g = telem;
                    }
                    thread::sleep(std::time::Duration::from_secs(3));
                }
            });
        }

        restore_tabs();

        gtk_widget_show_all(win);
        g_timeout_add(900, on_radar_tick, ptr::null_mut());
        gtk_main();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_drop::schedule_upload_completion;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

    fn make_test_app(win: *mut GtkWidget) -> AppState {
        unsafe {
            let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
            gtk_container_add(win, vbox);
            let btn = gtk_button_new();
            gtk_box_pack_start(vbox, btn, 0, 0, 0);
            let lbl = gtk_label_new(c"".as_ptr());
            gtk_box_pack_start(vbox, lbl, 0, 0, 0);
            let footer = gtk_label_new(c"".as_ptr());
            gtk_box_pack_start(vbox, footer, 0, 0, 0);
            let stack = gtk_stack_new();
            gtk_box_pack_start(vbox, stack, 1, 1, 0);
            AppState {
                config: WorkspaceConfig::load(),
                win,
                is_fullscreen: false,
                sidebar: Sidebar::new(vbox, on_sidebar_search_changed),
                sidebar_visible: true,
                theme_mode: theme_mode::ThemeMode::Dark,
                theme_btn: btn,
                font_scale: 1.0,
                zoom_btn: btn,
                reconnect_btn: btn,
                split_vert_btn: btn,
                split_horiz_btn: btn,
                next_id: 1,
                active_id: 1,
                sessions: Vec::new(),
                tab_store: None,
                stack,
                tab_strip: TabStrip::new(vbox, on_new_tab),
                mi_btn: btn,
                mi_pop: ptr::null_mut(),
                mi_pop_label: lbl,
                mi_refactor_btn: btn,
                tokens_menu: TokensMenu::new(vbox, on_tokens_open),
                agents_pop: ptr::null_mut(),
                box_pop: ptr::null_mut(),
                box_gpu_label: lbl,
                box_tmux_pop_box: vbox,
                settings_pop: ptr::null_mut(),
                ssh_count_label: lbl,
                git_menu: GitMenu::new(btn, on_git_open, on_git_popover_key_press),
                handoff: HandoffMenu::new(btn, on_handoff_open),
                footer_status: footer,
                footer_hold_until: Cell::new(None),
                ssh_hosts: Vec::new(),
                ssh_toggle_btns: Vec::new(),
                ssh_rows: Vec::new(),
                box_ssh_rows: Vec::new(),
                sidebar_filter: String::new(),
                box_filter: String::new(),
                enabled_hosts_shared: Arc::new(Mutex::new(Vec::new())),
                tmux_jump_btns: Vec::new(),
                last_tmux_keys: Vec::new(),
                mi_scheduler: MiScheduler::default(),
                git_scheduler: GitStatusScheduler::default(),
                box_state: Arc::new(Mutex::new(BoxTelemetry::default())),
                palette: palette::CommandPalette::new(vbox, execute_palette_action, focus_active_vte),
                search_bar: search_bar::SearchBar::new(
                    active_search_vte,
                    collect_close_search_vtes,
                ),
                pin_pop: tab_order::TabPinPopover::new(
                    vbox,
                    apply_pinned_tab_title,
                    focus_active_vte,
                ),
            }
        }
    }

    fn verify_app_drop_routing_and_mid_upload_close() {
        install_file_drop_hooks();
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let app = make_test_app(win);
        let footer = app.footer_status;
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        let local_sid = add_tab("Local", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
        let rem_tab = Tab::remote("gpu-box", Some("main"));
        let rem_sid = add_tab("gpu-box", "", "/tmp", SessionCategory::Remote, &["sh"], rem_tab);
        let (local_vte, rem_vte) = APP.with(|cell| {
            let st = cell.borrow();
            let a = st.as_ref().unwrap();
            (a.sessions[0].vte, a.sessions[1].vte)
        });
        assert_eq!(lookup_vte_drop_target(local_vte), DropTarget::Local);
        APP.with(|cell| {
            cell.borrow_mut().as_mut().unwrap().sessions[0].proc_info.ssh_host = "jump-box".into();
        });
        assert_eq!(
            lookup_vte_drop_target(local_vte),
            DropTarget::Remote { sid: local_sid, host: "jump-box".to_owned() }
        );
        APP.with(|cell| {
            cell.borrow_mut().as_mut().unwrap().sessions[0].proc_info.ssh_host.clear();
        });
        assert_eq!(lookup_vte_drop_target(local_vte), DropTarget::Local);
        assert_eq!(
            lookup_vte_drop_target(rem_vte),
            DropTarget::Remote { sid: rem_sid, host: "gpu-box".to_owned() }
        );
        close_session(rem_sid);
        pump_gtk_events();
        schedule_upload_completion(UploadCompletion {
            sid: rem_sid,
            vte_addr: rem_vte as usize,
            injected_text: Some("/tmp/akhterm_closed ".to_owned()),
            footer_msg: "📎 Загружено после закрытия".to_owned(),
        });
        pump_gtk_events();
        let footer_txt = unsafe { CStr::from_ptr(gtk_label_get_text(footer)).to_string_lossy().into_owned() };
        assert_eq!(footer_txt, "📎 Загружено после закрытия");
        close_session(local_sid);
        APP.with(|cell| *cell.borrow_mut() = None);
        unsafe { gtk_widget_destroy(win) };
        pump_gtk_events();
    }

    fn verify_unified_rename_persists_and_resets_in_tabs_v1() {
        let state_path = std::env::temp_dir().join(format!("akhterm_k8_tabs_{}", std::process::id()));
        let _ = std::fs::remove_file(&state_path);
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let mut app = make_test_app(win);
        app.tab_store = Some(TabStore::new_for_test(state_path.clone()));
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        let sid = add_tab("Shell", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
        commit_tab_rename(sid, "Прод ЦУ".to_owned());
        let saved = std::fs::read_to_string(&state_path).expect("tabs-v1 written");
        let decoded = TabsState::decode(&saved).expect("valid tabs-v1");
        assert_eq!(decoded.pinned_at(0), Some("Прод ЦУ"));
        commit_tab_rename(sid, "   ".to_owned());
        let cleared = std::fs::read_to_string(&state_path).expect("tabs-v1 updated");
        let decoded_clear = TabsState::decode(&cleared).expect("valid cleared tabs-v1");
        assert_eq!(decoded_clear.pinned_at(0), None);
        close_session(sid);
        APP.with(|cell| *cell.borrow_mut() = None);
        unsafe { gtk_widget_destroy(win) };
        pump_gtk_events();
        let _ = std::fs::remove_file(&state_path);
    }

    #[repr(C)]
    struct RawTestEventButton {
        type_: c_int,
        window: *mut c_void,
        send_event: i8,
        time: u32,
        x: f64,
        y: f64,
        axes: *mut f64,
        state: c_uint,
        button: c_uint,
        device: *mut c_void,
        x_root: f64,
        y_root: f64,
    }

    fn current_session_ids() -> Vec<usize> {
        APP.with(|cell| {
            cell.borrow()
                .as_ref()
                .unwrap()
                .sessions
                .iter()
                .map(|s| s.id)
                .collect()
        })
    }

    fn verify_wayland_safe_mouse_tab_drag_without_root_coords() {
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        unsafe { gtk_window_set_default_size(win, 640, 320) };
        let app = make_test_app(win);
        let tabs_box = app.tab_strip.tabs_box();
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        let sid1 = add_tab("Длинная вкладка ЦУ", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
        let sid2 = add_tab("Кратко", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
        unsafe { gtk_widget_show_all(win) };
        thread::sleep(Duration::from_millis(25));
        pump_gtk_events();
        let btn1 = APP.with(|cell| cell.borrow().as_ref().unwrap().sessions[0].top_tab.select_button());

        let press = unsafe { gdk_event_new(GDK_BUTTON_PRESS) };
        let motion = unsafe { gdk_event_new(GDK_MOTION_NOTIFY) };
        unsafe {
            (*(press as *mut RawTestEventButton)).x = 12.0;
            (*(press as *mut RawTestEventButton)).button = 1;
            on_tab_button_press(btn1, press, sid1 as *mut c_void);
            (*(motion as *mut RawTestEventButton)).x = 15.0;
            assert_eq!(on_tab_motion(btn1, motion, sid1 as *mut c_void), 0);
            assert_eq!(current_session_ids(), vec![sid1, sid2]);
            (*(motion as *mut RawTestEventButton)).x = 300.0;
            assert_eq!(on_tab_motion(btn1, motion, sid1 as *mut c_void), 1);
            assert_eq!(current_session_ids(), vec![sid2, sid1]);
            thread::sleep(Duration::from_millis(25));
            pump_gtk_events();
            let post_ox = platform::translate_widget_x_to_ancestor(btn1, tabs_box, 0.0).unwrap();
            assert!(post_ox > 20.0, "expected shifted tab allocation, got {post_ox}");
            (*(motion as *mut RawTestEventButton)).x = 300.0 - post_ox;
            assert_eq!(on_tab_motion(btn1, motion, sid1 as *mut c_void), 1);
            assert_eq!(current_session_ids(), vec![sid2, sid1]);
            on_tab_button_release(btn1, press, sid1 as *mut c_void);
            gdk_event_free(press);
            gdk_event_free(motion);
        }
        close_session(sid1);
        close_session(sid2);
        APP.with(|cell| *cell.borrow_mut() = None);
        unsafe { gtk_widget_destroy(win) };
        pump_gtk_events();
    }

    fn set_test_clipboard_pixbuf() {
        unsafe {
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            let pix = gdk_pixbuf_new(0, 1, 8, 4, 4);
            gtk_clipboard_set_image(clip, pix);
            g_object_unref(pix);
            pump_gtk_events();
        }
    }

    fn wait_captured_output(path: &Path, needle: &str) -> String {
        for _ in 0..40 {
            pump_gtk_events();
            if std::fs::read_to_string(path).is_ok_and(|s| s.contains(needle)) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        std::fs::read_to_string(path).unwrap_or_default()
    }

    fn verify_smart_copy_on_vte(vte: *mut GtkWidget) {
        let banner = "Скопируй меня";
        unsafe { vte_terminal_feed(vte, banner.as_ptr() as *const c_char, banner.len() as isize) };
        thread::sleep(Duration::from_millis(40));
        pump_gtk_events();
        unsafe { vte_terminal_select_all(vte) };
        pump_gtk_events();
        assert!(copy_active_selection(true));
        assert!(!copy_active_selection(true));
        assert!(!clipboard_has_direct_image());
    }

    fn verify_smart_ctrl_c_and_ctrl_v_across_shell_and_agy() {
        let out_file = std::env::temp_dir().join(format!("akhterm_ctrlv_{}", std::process::id()));
        let _ = std::fs::remove_file(&out_file);
        let cmd = format!("cat > '{}'", out_file.display());
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        APP.with(|cell| *cell.borrow_mut() = Some(make_test_app(win)));
        let sid = add_tab("AGY", "", "/tmp", SessionCategory::Project, &["sh", "-c", &cmd], Tab::local("shell"));
        let vte = APP.with(|cell| cell.borrow().as_ref().unwrap().sessions[0].vte);
        unsafe { gtk_widget_show_all(win) };
        pump_gtk_events();

        APP.with(|c| c.borrow_mut().as_mut().unwrap().sessions[0].proc_info.agent = "AGY".into());
        context_menu::copy_text_to_clipboard("текст для AGY\n");
        pump_gtk_events();
        assert!(paste_plain_ctrl_v());

        set_test_clipboard_pixbuf();
        APP.with(|c| c.borrow_mut().as_mut().unwrap().sessions[0].proc_info.agent = "AGY".into());
        assert!(!paste_plain_ctrl_v());

        verify_smart_copy_on_vte(vte);
        feed_vte_text(vte, "\x04", false);
        let captured = wait_captured_output(&out_file, "текст для AGY");
        assert!(captured.contains("текст для AGY"), "captured: {captured:?}");
        let _ = std::fs::remove_file(&out_file);
        close_session(sid);
        APP.with(|cell| *cell.borrow_mut() = None);
        unsafe { gtk_widget_destroy(win) };
        pump_gtk_events();
    }

    #[test]
    fn app_routes_local_and_remote_drops_and_ignores_closed_vte_on_idle() {
        with_headless_gtk(|| {
            verify_app_drop_routing_and_mid_upload_close();
            verify_unified_rename_persists_and_resets_in_tabs_v1();
            verify_wayland_safe_mouse_tab_drag_without_root_coords();
            verify_smart_ctrl_c_and_ctrl_v_across_shell_and_agy();
        });
    }

    #[test]
    fn k8_full_stack_maintainability_index_targets() {
        let mut rows = Vec::new();
        for entry in std::fs::read_dir("src").expect("src dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let content = std::fs::read_to_string(&path).expect("read rs");
            let (sloc, cc, score) = mi::compute_file_mi(&content);
            rows.push((name, sloc, cc, score, mi::grade_for_mi(score)));
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, sloc, cc, score, grade) in &rows {
            println!("{name:<18} | SLOC {sloc:>4} | CC {cc:>2} | MI {score:>6.2} | Grade {grade}");
            if name == "main.rs" {
                assert!(*score >= 54.87, "main.rs MI {score:.2} < 54.87");
            }
            if name == "platform.rs" {
                assert!(*score >= 60.0, "platform.rs MI {score:.2} < 60.0");
            }
            let is_new_v04 = matches!(
                name.as_str(),
                "context_menu.rs"
                    | "file_drop.rs"
                    | "git_status.rs"
                    | "search_bar.rs"
                    | "ssh_detect.rs"
                    | "tab_order.rs"
                    | "telemetry.rs"
                    | "telemetry_agy.rs"
                    | "theme_mode.rs"
            );
            if name == "telemetry_agy.rs" {
                assert!(*score >= 62.0, "telemetry_agy.rs MI {score:.2} < 62.0");
            } else if is_new_v04 {
                assert!(*score >= 58.0, "{name} MI {score:.2} < 58.0");
            }
        }
    }
}
