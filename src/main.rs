mod approval;
mod config;
mod ffi;
mod git_wt;
mod handoff;
mod mi;
mod mi_watch;
mod palette;
mod paste_route;
mod paths;
mod proc_radar;
mod session;
mod sidebar;
mod split_pane;
mod ssh_detect;
mod tab_strip;
mod tabs;
mod theme;
mod theme_mode;
mod ui_font;
mod ui_text;

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use approval::{
    find_next_session_attention, is_jump_attention_shortcut, settle_working_session,
    sync_window_focus_urgency,
};
use config::WorkspaceConfig;
use ffi::*;
use git_wt::{agent_slug_from_cmd, create_agent_worktree};
use handoff::{
    current_unix_secs, list_handoff_files, rebuild_handoff_widgets, resolve_handoff_delivery,
    save_vte_capture, CaptureMeta, HandoffCallbacks, HandoffMenu, HandoffTarget,
};
use mi::ProjectMiReport;
use mi_watch::{
    build_mi_refactor_prompt, format_mi_button_label, format_mi_popover_markup, lookup_mi,
    mi_grade_class, resolve_active_mi, MiScheduler,
};
use paste_route::{choose_paste_route, resolve_pane_agent, PasteRoute};
use paths::{expand_home, home_dir, shorten_home};
use proc_radar::{
    detect_local_agent, inspect_proc_tree, load_ssh_hosts, query_ssh_servers, save_ssh_hosts,
    switch_remote_tmux_session, try_paste_clipboard_image, BoxTelemetry, RemoteTmuxItem,
    SshHostEntry,
};
use session::{
    create_vte_widget, refresh_smart_title, register_link_cwd_lookup, LinkCwd, SessionCategory,
    TermSession, WorkState,
};
use sidebar::Sidebar;
use split_pane::{apply_zoom_scale, finish_split, wrap_vte_scroll, TabPanes};
use tab_strip::TabStrip;
use tabs::{Tab, TabStore, TabsState};
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
    agents_pop: *mut GtkWidget,
    box_pop: *mut GtkWidget,
    box_gpu_label: *mut GtkWidget,
    box_tmux_pop_box: *mut GtkWidget,
    settings_pop: *mut GtkWidget,
    ssh_count_label: *mut GtkWidget,
    handoff: HandoffMenu,
    footer_status: *mut GtkWidget,
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
    box_state: Arc<Mutex<BoxTelemetry>>,
    palette: palette::CommandPalette,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
    static PASTE_BUSY: Cell<bool> = const { Cell::new(false) };
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
        let Ok(mut state) = cell.try_borrow_mut() else { return false };
        let Some(app) = state.as_mut() else { return false };
        let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(vte)) else {
            return false;
        };
        s.panes.record_focus(vte);
        sync_session_proc(s, None);
        true
    });
    if updated {
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
    let sid = user_data as usize;
    select_session(sid);
}

pub fn select_session(sid: usize) {
    let targets = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut()?;
        app.active_id = sid;
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        s.unread = false;
        if s.state == WorkState::Ready {
            s.state = WorkState::Idle;
        }
        Some((app.stack, s.active_vte()))
    });
    if let Some((stack, focus_vte)) = targets {
        if !stack.is_null() {
            if let Ok(name_c) = CString::new(format!("term_{sid}")) {
                unsafe { gtk_stack_set_visible_child_name(stack, name_c.as_ptr()) };
            }
        }
        if !focus_vte.is_null() {
            unsafe { gtk_widget_grab_focus(focus_vte) };
        }
    }
    refresh_ui();
    // The tab can be new and still waiting for its GTK size allocation.
    unsafe { g_idle_add(on_reveal_tab, sid as *mut c_void) };
    save_tabs();
}

extern "C" fn on_reveal_tab(data: *mut c_void) -> c_int {
    let sid = data as usize;
    APP.with(|cell| {
        let state = cell.borrow();
        let Some(app) = state.as_ref().filter(|app| app.active_id == sid) else { return };
        if let Some(session) = app.sessions.iter().find(|s| s.id == sid) {
            app.tab_strip.reveal(&session.top_tab);
        }
    });
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
                    s.title = remote.title();
                }
                s.remote_tmux = None;
                s.tmux_title_from_vte = false;
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

fn add_tab(
    title: &str,
    profile_cwd: &str,
    cwd: &str,
    category: SessionCategory,
    argv: &[&str],
    tab: Tab,
) -> usize {
    let mut new_id = 0;
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            let sid = app.next_id;
            app.next_id += 1;
            new_id = sid;

            let (vte, pid) = create_terminal(cwd, argv, sid, app.font_scale);
            unsafe {
                let term_scroll = wrap_vte_scroll(vte);
                let panes = TabPanes::new(term_scroll, vte);

                let stack_name = CString::new(format!("term_{sid}")).unwrap();
                gtk_stack_add_named(app.stack, panes.root, stack_name.as_ptr());

                let sidebar_row = app.sidebar.append_tab(category, sid, on_select_session);
                let top_tab = app.tab_strip.append(sid, category, on_select_session, on_close_tab);

                gtk_widget_show_all(panes.root);

                let mut session = TermSession {
                    id: sid,
                    base_title: title.to_string(),
                    profile_cwd: profile_cwd.to_string(),
                    title: title.to_string(),
                    osc_title: String::new(),
                    custom_title: false,
                    cwd: cwd.to_string(),
                    category,
                    vte,
                    scroll_wrap: term_scroll,
                    panes,
                    sidebar_row,
                    top_tab,
                    child_pid: pid,
                    state: WorkState::Idle,
                    unread: false,
                    last_output: Instant::now(),
                    work_started: Instant::now(),
                    last_notified: None,
                    proc_info: inspect_proc_tree(pid),
                    tab,
                    remote_tmux: None,
                    tmux_title_from_vte: false,
                };
                refresh_smart_title(&mut session);
                app.sessions.push(session);
                app.active_id = sid;
            }
        }
    });
    select_session(new_id);
    new_id
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

struct UiRefreshPlan {
    rows: Vec<sidebar::RowUpdate>,
    tabs: Vec<tab_strip::TabUpdate>,
    sidebar: sidebar::SidebarUpdate,
    btns: [*mut GtkWidget; 3],
    flags: (bool, bool),
    mi: MiUiPlan,
    footer: (*mut GtkWidget, String),
}

fn prepare_session_views(
    app: &AppState,
    mi_map: &std::collections::HashMap<String, ProjectMiReport>,
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
        rows.push(s.sidebar_row.prepare_update(idx, s, mi_badge, is_act));
        tabs.push(s.top_tab.prepare_update(s, is_act));
    }
    (rows, tabs, active_cwd, can_reconnect, counts)
}

impl UiRefreshPlan {
    fn build(app: &AppState) -> Self {
        let home = home_dir();
        let local_cwds = collect_local_cwds(app);
        app.mi_scheduler.sync_targets(&app.config.mi_directories(), &local_cwds, &home);
        let mi_map = app.mi_scheduler.snapshot();
        let (rows, tabs, active_cwd, can_rec, [working, ready, ssh_count]) =
            prepare_session_views(app, &mi_map, &home);
        let (cur_mi, self_mi) = resolve_active_mi(
            &mi_map,
            active_cwd.as_deref(),
            app.config.mi_root.as_deref(),
            &home,
        );
        let footer = format!(
            "Вкладки: {}    В работе: {}    Готово: {}    SSH: {}",
            app.sessions.len(),
            working,
            ready,
            ssh_count
        );
        Self {
            rows,
            tabs,
            sidebar: prepare_sidebar_filter(app),
            btns: [app.reconnect_btn, app.split_vert_btn, app.split_horiz_btn],
            flags: (can_rec, !app.sessions.is_empty() && !can_rec),
            mi: MiUiPlan::from_state(app, cur_mi, self_mi),
            footer: (app.footer_status, footer),
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
        self.mi.apply();
        set_label_text(self.footer.0, &self.footer.1);
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
    APP.with(|cell| cell.borrow().as_ref().map_or(0, |app| app.active_id))
}

fn active_local_split_context() -> Option<(usize, String, String, f64)> {
    APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == app.active_id && s.remote().is_none())?;
        let cwd = s.active_local_cwd().unwrap_or_else(|| s.cwd.clone());
        Some((s.id, cwd, app.config.shell.clone(), app.font_scale))
    })
}

fn split_active_tab(orientation: c_int) {
    let Some((sid, cwd, shell, scale)) = active_local_split_context() else { return };
    let (new_vte, new_pid) = create_terminal(&cwd, &[shell.as_str()], sid, scale);
    let new_scroll = wrap_vte_scroll(new_vte);
    let split_op = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let s = state.as_mut()?.sessions.iter_mut().find(|s| s.id == sid)?;
        Some(s.panes.prepare_split(s.scroll_wrap, s.vte, new_scroll, new_vte, new_pid, cwd, orientation))
    });
    finish_split(split_op, new_scroll, new_vte);
    refresh_ui();
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
        let mut state = cell.borrow_mut();
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
    close_session(data as usize);
}

fn close_session(sid: usize) {
    let removed = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut()?;
        let pos = app.sessions.iter().position(|s| s.id == sid)?;
        let removed = app.sessions.remove(pos);
        if sid == app.active_id {
            let neighbor = pos.min(app.sessions.len().saturating_sub(1));
            app.active_id = app.sessions.get(neighbor).map_or(0, |s| s.id);
        }
        Some(removed)
    });
    if let Some(s) = removed {
        s.panes.stop_all_extra();
        if s.remote().is_some() {
            stop_ssh(s.child_pid);
        }
        unsafe {
            gtk_widget_destroy(s.panes.root);
        }
        s.sidebar_row.destroy();
        s.top_tab.destroy();
        select_session(active_session_id());
    }
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
        let state = cell.borrow();
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
        let mut state = cell.borrow_mut();
        let app = state.as_mut()?;
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        let old_vte = std::mem::replace(&mut s.vte, vte);
        s.panes.record_focus(vte);
        s.child_pid = pid;
        s.proc_info = inspect_proc_tree(pid);
        s.remote_tmux = None;
        s.tmux_title_from_vte = false;
        s.osc_title.clear();
        s.title = title;
        s.state = WorkState::Idle;
        s.unread = false;
        let now = Instant::now();
        s.last_output = now;
        s.work_started = now;
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
        let cwd = s.active_local_cwd().unwrap_or_else(|| s.cwd.clone());
        Ok((key, cwd))
    })
}

fn set_footer_message(message: &str) {
    if message.is_empty() {
        return;
    }
    let footer = APP.with(|cell| cell.borrow().as_ref().map(|app| app.footer_status));
    if let Some(footer) = footer.filter(|w| !w.is_null()) {
        set_label_text(footer, message);
    }
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
        let state = cell.borrow();
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

struct PasteGuard;

impl Drop for PasteGuard {
    fn drop(&mut self) {
        PASTE_BUSY.with(|busy| busy.set(false));
    }
}

fn active_paste_context() -> Option<(usize, *mut GtkWidget, String, String)> {
    APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        let s = app.sessions.iter_mut().find(|s| s.id == app.active_id)?;
        let target_vte = s.active_vte();
        let focused_pid = s.active_pid();
        let prev_agent = s.proc_info.agent.clone();
        let agent = if s.remote().is_none() {
            let live = detect_local_agent(focused_pid);
            sync_session_proc(s, None);
            let fallback = if s.proc_info.agent.is_empty() { &prev_agent } else { &s.proc_info.agent };
            resolve_pane_agent(live.as_deref(), target_vte == s.vte, fallback)
        } else {
            s.proc_info.agent.clone()
        };
        Some((s.id, target_vte, session_ssh_host(s), agent))
    })
}

fn session_paste_widgets(
    sid: usize,
    preferred_vte: *mut GtkWidget,
) -> Option<(*mut GtkWidget, *mut GtkWidget)> {
    APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        let vte = if !preferred_vte.is_null() && s.has_vte(preferred_vte) {
            preferred_vte
        } else {
            s.active_vte()
        };
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

fn paste_routed(plain_ctrl_v: bool, force_image: bool) -> bool {
    if PASTE_BUSY.with(|busy| busy.replace(true)) {
        return !force_image;
    }
    let _guard = PasteGuard;
    let Some((sid, target_vte, host, agent)) = active_paste_context() else { return false };
    if choose_paste_route(plain_ctrl_v, &host, &agent) == PasteRoute::PassThroughToPty {
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
    paste_routed(false, force_image)
}

fn paste_plain_ctrl_v() -> bool {
    paste_routed(true, true)
}

fn copy_active_selection() {
    let vte = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        app.sessions.iter().find(|s| s.id == app.active_id).map(TermSession::active_vte)
    });
    if let Some(vte) = vte.filter(|w| !w.is_null()) {
        unsafe { vte_terminal_copy_clipboard_format(vte, 1) };
    }
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

        // F11 -> toggle window square / fullscreen
        if keyval == 0xffc8 {
            on_toggle_window_square(ptr::null_mut(), ptr::null_mut());
            return 1;
        }

        if (state & GDK_MOD1_MASK) != 0 && (b'1' as u32..=b'9' as u32).contains(&keyval) {
            let idx = (keyval - b'1' as u32) as usize;
            let mut target_sid = None;
            APP.with(|cell| {
                if let Some(app) = cell.borrow().as_ref() {
                    if let Some(s) = app.sessions.get(idx) {
                        target_sid = Some(s.id);
                    }
                }
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
            // Plain Ctrl + V (keycode 55): pass through to local multimodal TUI agents, else paste image path!
            if (state & GDK_SHIFT_MASK) == 0
                && (keyval == b'v' as u32 || keyval == b'V' as u32 || keycode == 55)
            {
                if paste_plain_ctrl_v() {
                    return 1;
                }
            }
        }

        if (state & GDK_CONTROL_MASK) != 0 && (state & GDK_SHIFT_MASK) != 0 {
            if handle_tab_shortcut(keyval, keycode) {
                return 1;
            }
            if keyval == b'C' as u32 || keyval == b'c' as u32 || keycode == 54 {
                copy_active_selection();
                return 1;
            }
            if keyval == b'V' as u32 || keyval == b'v' as u32 || keycode == 55 {
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

fn handle_tab_shortcut(keyval: c_uint, keycode: u16) -> bool {
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
    }
}

fn toggle_palette() {
    let snapshot = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let pal = app.palette.clone();
        if pal.is_open() {
            return Some((pal, None));
        }
        let tmux_items = app.box_state.lock().ok().map(|g| g.tmux_items.clone()).unwrap_or_default();
        let src = palette::PaletteSource {
            sessions: &app.sessions,
            tmux_items: &tmux_items,
            profiles: &app.config.profiles,
            ssh_hosts: &app.ssh_hosts,
            briefs_dir: app.config.briefs_dir.as_deref(),
            extra_handoffs: app.handoff.files(),
        };
        Some((pal, Some(palette::collect_items(&src))))
    });
    if let Some((pal, maybe_items)) = snapshot {
        match maybe_items {
            Some(items) => pal.open(items),
            None => pal.close(),
        }
    }
}

fn route_palette_key(keyval: c_uint, state: c_uint, keycode: u16) -> Option<c_int> {
    if palette::is_palette_shortcut(keyval, state, keycode) {
        toggle_palette();
        return Some(1);
    }
    if theme_mode::is_theme_toggle_shortcut(keyval, state, keycode) {
        toggle_theme();
        return Some(1);
    }
    let open = APP.with(|cell| cell.borrow().as_ref().is_some_and(|app| app.palette.is_open()));
    open.then_some(0)
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
    TabsState { active: 0, tabs }
}

fn restore_tabs() {
    let (store, saved) = TabStore::load();
    let TabsState { active, tabs } = saved.unwrap_or_else(|| {
        APP.with(|cell| default_tabs(&cell.borrow().as_ref().expect("app is initialized").config))
    });
    let ids: Vec<usize> = tabs.into_iter().map(open_tab).collect();
    select_session(ids.get(active).copied().unwrap_or(0));
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.tab_store = Some(store);
        }
    });
    save_tabs();
}

fn save_tabs() {
    APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let Some(app) = state.as_mut() else { return };
        let Some(store) = &mut app.tab_store else { return };
        let active = app.sessions.iter().position(|s| s.id == app.active_id).unwrap_or(0);
        let tabs = app.sessions.iter_mut().map(TermSession::snapshot_tab).collect();
        store.checkpoint(TabsState { active, tabs });
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
    let _ui_font = ui_font::load().inspect_err(|error| eprintln!("AkhTerm: {error}"));
    register_link_cwd_lookup(lookup_vte_link_cwd);
    let config = WorkspaceConfig::load();
    let initial_theme = theme_mode::load_initial_theme(config.theme);
    let mi_dirs = config.mi_directories();
    unsafe {
        gtk_init(ptr::null_mut(), ptr::null_mut());
        theme::apply_gtk_theme_mode(initial_theme);
        const SIGTERM: c_int = 15;
        g_unix_signal_add(SIGTERM, on_terminate, ptr::null_mut());

        let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        gtk_window_set_title(win, text::APP_NAME.as_ptr());
        gtk_window_set_wmclass(win, text::APP_NAME.as_ptr(), text::APP_NAME.as_ptr());
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

        let mi_btn = gtk_button_new_with_label(text::MAINTAINABILITY.as_ptr());
        gtk_box_pack_end(topbar, mi_btn, 0, 0, 0);

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

        // Popover: Handoff Briefs & Terminal Capture
        let handoff = HandoffMenu::new(handoff_btn, on_handoff_open);

        // Workspace: Left Sidebar + Right Terminal Stack
        let workspace_hbox = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
        gtk_box_pack_start(root_vbox, workspace_hbox, 1, 1, 0);

        let sidebar = Sidebar::new(workspace_hbox, on_sidebar_search_changed);

        let stack = gtk_stack_new();
        gtk_box_pack_start(workspace_hbox, stack, 1, 1, 0);

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
        let box_state: Arc<Mutex<BoxTelemetry>> = Arc::new(Mutex::new(BoxTelemetry::default()));
        let cmd_palette = palette::CommandPalette::new(topbar, execute_palette_action, focus_active_vte);

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
                agents_pop,
                box_pop,
                box_gpu_label,
                box_tmux_pop_box,
                settings_pop,
                ssh_count_label,
                handoff,
                footer_status,
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
                box_state: Arc::clone(&box_state),
                palette: cmd_palette,
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
