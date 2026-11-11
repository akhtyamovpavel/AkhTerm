mod approval;
mod config;
mod ffi;
mod git_wt;
mod mi;
mod paste_route;
mod paths;
mod proc_radar;
mod session;
mod sidebar;
mod tab_strip;
mod tabs;
mod theme;
mod ui_font;
mod ui_text;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use approval::{
    find_next_session_attention, is_jump_attention_shortcut, settle_working_session,
    sync_window_focus_urgency,
};
use config::WorkspaceConfig;
use ffi::*;
use git_wt::{agent_slug_from_cmd, create_agent_worktree};
use mi::{analyze_project_mi, ProjectMiReport};
use paste_route::{choose_paste_route, PasteRoute};
use paths::{expand_home, home_dir, shorten_home};
use proc_radar::{
    foreground_cwd, inspect_proc_tree, load_ssh_hosts, query_ssh_servers, save_ssh_hosts,
    switch_remote_tmux_session, try_paste_clipboard_image, BoxTelemetry, RemoteTmuxItem,
    SshHostEntry,
};
use session::{create_vte_widget, refresh_smart_title, SessionCategory, TermSession, WorkState};
use sidebar::Sidebar;
use tab_strip::TabStrip;
use tabs::{Tab, TabStore, TabsState};
use ui_text as text;

struct AppState {
    config: WorkspaceConfig,
    win: *mut GtkWidget,
    is_fullscreen: bool,
    sidebar: Sidebar,
    sidebar_visible: bool,
    font_scale: f64,
    zoom_btn: *mut GtkWidget,
    reconnect_btn: *mut GtkWidget,
    next_id: usize,
    active_id: usize,
    sessions: Vec<TermSession>,
    tab_store: Option<TabStore>,
    stack: *mut GtkWidget,
    tab_strip: TabStrip,
    mi_btn: *mut GtkWidget,
    mi_pop_label: *mut GtkWidget,
    agents_pop: *mut GtkWidget,
    box_pop: *mut GtkWidget,
    box_gpu_label: *mut GtkWidget,
    box_tmux_pop_box: *mut GtkWidget,
    settings_pop: *mut GtkWidget,
    ssh_count_label: *mut GtkWidget,
    handoff_pop: *mut GtkWidget,
    handoff_files: Vec<PathBuf>,
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
    mi_cache: Arc<Mutex<HashMap<String, ProjectMiReport>>>,
    box_state: Arc<Mutex<BoxTelemetry>>,
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
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.font_scale = clamped;
            for s in &app.sessions {
                unsafe {
                    vte_terminal_set_font_scale(s.vte, clamped);
                }
            }
            let pct = (clamped * 100.0).round() as i32;
            if let Ok(c) = CString::new(format!("{pct}%")) {
                unsafe { gtk_button_set_label(app.zoom_btn, c.as_ptr()) };
            }
        }
    });
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
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.sidebar_visible = !app.sidebar_visible;
            app.sidebar.set_visible(app.sidebar_visible);
        }
    });
}

extern "C" fn on_toggle_window_square(_btn: *mut GtkWidget, _data: *mut c_void) {
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            unsafe {
                if app.is_fullscreen {
                    gtk_window_unfullscreen(app.win);
                    app.is_fullscreen = false;
                } else {
                    gtk_window_fullscreen(app.win);
                    app.is_fullscreen = true;
                }
            }
        }
    });
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
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.sidebar_filter = entry_text_lower(entry);
            if !app.sidebar_filter.is_empty() {
                app.sidebar.reveal_tmux_results();
            }
            filter_sidebar(app);
        }
    });
}

fn filter_sidebar(app: &AppState) {
    let tmux_rows = app.tmux_jump_btns.iter().map(|(_, _, key, button, _)| (key.as_str(), *button));
    app.sidebar.filter(&app.sidebar_filter, &app.sessions, tmux_rows);
}

extern "C" fn on_window_destroy(_w: *mut GtkWidget, _data: *mut c_void) {
    save_tabs();
    unsafe { gtk_main_quit() };
}

extern "C" fn on_terminate(_data: *mut c_void) -> c_int {
    on_window_destroy(ptr::null_mut(), ptr::null_mut());
    0
}

extern "C" fn on_vte_contents_changed(vte: *mut GtkWidget, user_data: *mut c_void) {
    let sid = user_data as usize;
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            let active = app.active_id;
            let session = app.sessions.iter_mut().find(|s| s.id == sid && s.vte == vte);
            if let Some(s) = session.filter(|s| s.child_pid > 0) {
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

extern "C" fn on_vte_child_exited(vte: *mut GtkWidget, _status: c_int, user_data: *mut c_void) {
    let sid = user_data as usize;
    APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let Some(app) = state.as_mut() else { return };
        if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid && s.vte == vte) {
            s.child_pid = 0;
            s.state = WorkState::Idle;
            s.unread = false;
        }
    });
}

extern "C" fn on_vte_title_changed(vte: *mut GtkWidget, user_data: *mut c_void) {
    let sid = user_data as usize;
    let title_str = unsafe {
        let ptr = vte_terminal_get_window_title(vte);
        if ptr.is_null() {
            return;
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    };
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid && s.vte == vte) {
                if let Some(rest) = title_str.strip_prefix("AKHTERM_TMUX:") {
                    let parts: Vec<&str> = rest.splitn(4, '|').collect();
                    if parts.len() == 4 && !parts[0].is_empty() {
                        if let Tab::Remote(remote) = &mut s.tab {
                            if let Some(target) = &mut remote.tmux_session {
                                *target = parts[0].to_string();
                            }
                        }
                        s.remote_tmux = Some((
                            parts[0].to_string(),
                            parts[1].to_string(),
                            parts[2].to_string(),
                            parts[3].to_string(),
                        ));
                        s.tmux_title_from_vte = true;
                    }
                } else {
                    let clean = title_str.trim();
                    if !clean.is_empty()
                        && !clean.contains('@')
                        && !clean.starts_with('~')
                        && !clean.starts_with('/')
                        && clean != "zsh"
                        && clean != "bash"
                    {
                        s.osc_title = clean.chars().take(28).collect();
                    } else {
                        s.osc_title.clear();
                    }
                }
                sync_session_proc(s, None);
            }
        }
    });
    refresh_ui();
}

extern "C" fn on_select_session(_btn: *mut GtkWidget, user_data: *mut c_void) {
    let sid = user_data as usize;
    select_session(sid);
}

pub fn select_session(sid: usize) {
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.active_id = sid;
            if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid) {
                s.unread = false;
                if s.state == WorkState::Ready {
                    s.state = WorkState::Idle;
                }
                if let Ok(name_c) = CString::new(format!("term_{sid}")) {
                    unsafe {
                        gtk_stack_set_visible_child_name(app.stack, name_c.as_ptr());
                        gtk_widget_grab_focus(s.vte);
                    }
                }
            }
        }
    });
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
    let host = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let host = app.ssh_hosts.get(idx)?;
        unsafe {
            gtk_popover_popdown(app.settings_pop);
            gtk_popover_popdown(app.box_pop);
        }
        Some(host.alias.clone())
    });
    if let Some(host) = host {
        add_remote_session(&host, tmux);
    }
}

extern "C" fn on_jump_tmux(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let idx = idx_ptr as usize;
    let mut target_host = String::new();
    let mut target_session = String::new();
    let mut existing_remote_sid = None;

    APP.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            unsafe { gtk_popover_popdown(app.box_pop) };
            if let Some((host, sname, _, _, _)) = app.tmux_jump_btns.get(idx) {
                target_host = host.clone();
                target_session = sname.clone();
            }
            existing_remote_sid = app.sessions.iter().find(|s| {
                s.remote().is_some_and(|r| r.host == target_host && r.tmux_session.is_some())
            }).map(|s| (s.id, s.child_pid > 0));
        }
    });

    if target_session.is_empty() || target_host.is_empty() {
        return;
    }

    if let Some((sid, connected)) = existing_remote_sid {
        APP.with(|cell| {
            if let Some(app) = cell.borrow_mut().as_mut() {
                if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid) {
                    if let Tab::Remote(remote) = &mut s.tab {
                        remote.tmux_session = Some(target_session.clone());
                        s.title = remote.title();
                    }
                    s.remote_tmux = None;
                    s.tmux_title_from_vte = false;
                }
            }
        });
        if connected {
            switch_remote_tmux_session(&target_host, &target_session);
            select_session(sid);
        } else {
            reconnect_session(sid);
        }
    } else {
        add_remote_session(&target_host, Some(&target_session));
    }
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
                let term_scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
                gtk_scrolled_window_set_policy(term_scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
                gtk_container_add(term_scroll, vte);

                let stack_name = CString::new(format!("term_{sid}")).unwrap();
                gtk_stack_add_named(app.stack, term_scroll, stack_name.as_ptr());

                let sidebar_row = app.sidebar.append_tab(category, sid, on_select_session);
                let top_tab = app.tab_strip.append(sid, category, on_select_session, on_close_tab);

                gtk_widget_show_all(term_scroll);

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

pub fn refresh_ui() {
    APP.with(|cell| {
        let Some(app) = cell.borrow().as_ref().map(|a| a as *const AppState) else {
            return;
        };
        let app = unsafe { &*app };
        let mi_map = app.mi_cache.lock().ok().map(|g| g.clone()).unwrap_or_default();

        let mut working = 0usize;
        let mut ready = 0usize;
        let mut ssh_count = 0usize;
        let mut active_cwd = String::new();
        let mut can_reconnect = false;

        for (idx, s) in app.sessions.iter().enumerate() {
            let is_act = s.id == app.active_id;
            if is_act {
                can_reconnect = s.remote().is_some();
                active_cwd = if !s.proc_info.cwd.is_empty() {
                    expand_home(&s.proc_info.cwd)
                } else {
                    s.cwd.clone()
                };
            }
            if s.state == WorkState::Working {
                working += 1;
            }
            if s.unread {
                ready += 1;
            }
            if !s.proc_info.ssh_host.is_empty() {
                ssh_count += 1;
            }

            let lookup_dir = if !s.proc_info.cwd.is_empty() {
                expand_home(&s.proc_info.cwd)
            } else {
                s.cwd.clone()
            };
            let mi_badge = mi_map
                .get(&lookup_dir)
                .or_else(|| mi_map.get(&s.cwd))
                .map(|r| (r.avg_mi, r.grade));

            s.sidebar_row.update(idx, s, mi_badge, is_act);
            s.top_tab.update(&s.title, &s.proc_info.agent, is_act, s.unread, s.state.needs_approval());
        }
        filter_sidebar(app);
        unsafe { gtk_widget_set_sensitive(app.reconnect_btn, i32::from(can_reconnect)) };

        let self_mi = app.config.mi_root.as_ref().and_then(|path| mi_map.get(path));
        let cur_mi = mi_map.get(&active_cwd).or(self_mi);

        if let Some(rep) = cur_mi {
            remove_class(app.mi_btn, "btn-mi-a");
            remove_class(app.mi_btn, "btn-mi-b");
            remove_class(app.mi_btn, "btn-mi-c");
            let cls = match rep.grade {
                "A" => "btn-mi-a",
                "B" => "btn-mi-b",
                _ => "btn-mi-c",
            };
            add_class(app.mi_btn, cls);
            let lbl = format!("MI {:.1} [{}] ▾", rep.avg_mi, rep.grade);
            if let Ok(c) = CString::new(lbl) {
                unsafe { gtk_button_set_label(app.mi_btn, c.as_ptr()) };
            }

            let mut pop_lines = Vec::new();
            pop_lines.push(format!(
                "<b>ИНДЕКС ПОДДЕРЖИВАЕМОСТИ (Maintainability Index)</b>\n<span foreground='#a1a1aa'>Каталог: {}</span>",
                rep.root
            ));
            pop_lines.push(format!(
                "Средний MI (по функциям): <b>{:.1} / 100 (Класс {})</b>  ·  Файлов: {}  ·  SLOC: {}  ·  Средняя CC: {:.1}",
                rep.avg_mi, rep.grade, rep.files_count, rep.total_sloc, rep.avg_cc
            ));
            if !rep.worst_files.is_empty() {
                pop_lines.push("\n<b>Точки внимания (файлы с минимальным MI):</b>".to_string());
                for f in &rep.worst_files {
                    pop_lines.push(format!(
                        "  • <tt>{}</tt> — <b>MI {:.1}</b> (SLOC: {}, CC: {})",
                        f.rel_path, f.mi, f.sloc, f.cc
                    ));
                }
            }
            if let Some(ar) = self_mi {
                pop_lines.push(format!(
                    "\n<span foreground='#c4b5fd'>Ядро AkhTerm (Rust · VTE-2.91): <b>MI {:.1} [{}]</b> ({} файлов, {} SLOC)</span>",
                    ar.avg_mi, ar.grade, ar.files_count, ar.total_sloc
                ));
            }
            if let Ok(c) = CString::new(pop_lines.join("\n")) {
                unsafe { gtk_label_set_markup(app.mi_pop_label, c.as_ptr()) };
            }
        }

        let footer = format!(
            "Вкладки: {}    В работе: {}    Готово: {}    SSH: {}",
            app.sessions.len(),
            working,
            ready,
            ssh_count
        );
        set_label_text(app.footer_status, &footer);
    });
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
    s.proc_info = inspect_proc_tree(s.child_pid);
    if s.remote().is_none() && !s.proc_info.cwd.is_empty() {
        s.cwd = expand_home(&s.proc_info.cwd);
    }
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
    let vte = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        app.sessions.iter().find(|s| s.id == sid).map(|s| s.vte)
    });
    if let (Some(vte), Ok(c)) = (vte.filter(|w| !w.is_null()), CString::new(text)) {
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
    APP.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            if let Some(s) = app.sessions.iter().find(|s| s.id == app.active_id) {
                unsafe {
                    vte_terminal_feed_child(s.vte, b"\x03\0".as_ptr() as *const c_char, 1);
                    if s.child_pid > 0 {
                        let pgid = getpgid(s.child_pid);
                        if pgid > 0 {
                            kill(-pgid, 2);
                        }
                    }
                    gtk_widget_grab_focus(s.vte);
                }
            }
        }
    });
}

fn active_session_id() -> usize {
    APP.with(|cell| cell.borrow().as_ref().map_or(0, |app| app.active_id))
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
        if s.remote().is_some() {
            stop_ssh(s.child_pid);
        }
        unsafe {
            gtk_widget_destroy(s.scroll_wrap);
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
    let (old_vte, scroll_wrap) = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut().unwrap();
        let s = app.sessions.iter_mut().find(|s| s.id == sid).unwrap();
        let old_vte = std::mem::replace(&mut s.vte, vte);
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
        (old_vte, s.scroll_wrap)
    });
    unsafe {
        gtk_widget_destroy(old_vte);
        gtk_container_add(scroll_wrap, vte);
        gtk_widget_show_all(scroll_wrap);
    }
}

fn dismiss_tab_menu() {
    APP.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            unsafe { gtk_popover_popdown(app.box_pop) };
        }
    });
}

extern "C" fn on_popup_popover(_btn: *mut GtkWidget, pop_ptr: *mut c_void) {
    unsafe { gtk_popover_popup(pop_ptr) };
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
        let cwd = foreground_cwd(s.child_pid)
            .or_else(|| (!s.proc_info.cwd.is_empty()).then(|| expand_home(&s.proc_info.cwd)))
            .unwrap_or_else(|| s.cwd.clone());
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

extern "C" fn on_handoff_inject(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let path = APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let chosen = app.handoff_files.get(idx_ptr as usize)?;
        unsafe { gtk_popover_popdown(app.handoff_pop) };
        Some(chosen.clone())
    });
    if let Some(path) = path {
        let message = format!("Прочитай файл {} и действуй по нему.", path.display());
        send_to_active(&message);
    }
}

struct PasteGuard;

impl Drop for PasteGuard {
    fn drop(&mut self) {
        PASTE_BUSY.with(|busy| busy.set(false));
    }
}

fn active_paste_context() -> Option<(usize, String, String)> {
    APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        let s = app.sessions.iter_mut().find(|s| s.id == app.active_id)?;
        if s.remote().is_none() {
            sync_session_proc(s, None);
        }
        let host = s.remote().map_or(&s.proc_info.ssh_host, |r| &r.host).clone();
        Some((s.id, host, s.proc_info.agent.clone()))
    })
}

fn session_paste_widgets(sid: usize) -> Option<(*mut GtkWidget, *mut GtkWidget)> {
    APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref()?;
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        Some((s.vte, app.footer_status))
    })
}

fn finish_paste(vte: *mut GtkWidget, footer: *mut GtkWidget, image: Option<(String, String)>) {
    if let Some((path, status)) = image {
        if let Ok(c) = CString::new(path.as_str()) {
            unsafe { vte_terminal_feed_child(vte, c.as_ptr(), path.len() as isize) };
        }
        set_label_text(footer, &status);
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
    let Some((sid, host, agent)) = active_paste_context() else { return false };
    if choose_paste_route(plain_ctrl_v, &host, &agent) == PasteRoute::PassThroughToPty {
        return false;
    }
    let image = try_paste_clipboard_image(&host, force_image);
    if image.is_none() && force_image {
        return false;
    }
    let Some((vte, footer)) = session_paste_widgets(sid) else { return false };
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
        app.sessions.iter().find(|s| s.id == app.active_id).map(|s| s.vte)
    });
    if let Some(vte) = vte {
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
        (Some('W' | 'w'), _) | (_, 25) => close_session(active_session_id()),
        (Some('R' | 'r'), _) | (_, 27) => reconnect_session(active_session_id()),
        _ => return false,
    }
    true
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

fn list_handoff_files(directory: Option<&Path>) -> Vec<PathBuf> {
    let Some(entries) = directory.and_then(|path| fs::read_dir(path).ok()) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries.flatten().map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    files.sort_by(|left, right| right.cmp(left));
    files.truncate(7);
    files
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
    let config = WorkspaceConfig::load();
    let handoff_files = list_handoff_files(config.briefs_dir.as_deref());
    let mi_dirs = config.mi_directories();
    unsafe {
        gtk_init(ptr::null_mut(), ptr::null_mut());
        theme::apply_gtk_theme();
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
        if handoff_files.is_empty() {
            gtk_widget_set_no_show_all(handoff_btn, 1);
        }
        gtk_box_pack_end(topbar, handoff_btn, 0, 0, 0);

        let agents_btn = gtk_button_new_with_label(text::AGENTS.as_ptr());
        gtk_box_pack_end(topbar, agents_btn, 0, 0, 0);

        let mi_btn = gtk_button_new_with_label(text::MAINTAINABILITY.as_ptr());
        gtk_box_pack_end(topbar, mi_btn, 0, 0, 0);

        // Popover: Maintainability Index (MI)
        let mi_pop = gtk_popover_new(mi_btn);
        let mi_pop_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(mi_pop_box, "popover-box");
        let mi_pop_label = gtk_label_new(CString::new("Расчёт Maintainability Index...").unwrap().as_ptr());
        gtk_label_set_xalign(mi_pop_label, 0.0);
        gtk_container_add(mi_pop_box, mi_pop_label);
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

        // Popover: Handoff Briefs
        let handoff_pop = gtk_popover_new(handoff_btn);
        let handoff_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        add_class(handoff_vbox, "popover-box");
        for (idx, path) in handoff_files.iter().enumerate() {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                let b = gtk_button_new_with_label(CString::new(name).unwrap().as_ptr());
                connect_click(b, on_handoff_inject, idx as *mut c_void);
                gtk_box_pack_start(handoff_vbox, b, 0, 1, 0);
            }
        }
        gtk_container_add(handoff_pop, handoff_vbox);
        gtk_widget_show_all(handoff_vbox);
        connect_click(handoff_btn, on_popup_popover, handoff_pop);

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
        let mi_cache: Arc<Mutex<HashMap<String, ProjectMiReport>>> = Arc::new(Mutex::new(HashMap::new()));
        let box_state: Arc<Mutex<BoxTelemetry>> = Arc::new(Mutex::new(BoxTelemetry::default()));

        APP.with(|cell| {
            *cell.borrow_mut() = Some(AppState {
                config,
                win,
                is_fullscreen: false,
                sidebar,
                sidebar_visible: true,
                font_scale: 1.0,
                zoom_btn,
                reconnect_btn,
                next_id: 1,
                active_id: 1,
                sessions: Vec::new(),
                tab_store: None,
                stack,
                tab_strip,
                mi_btn,
                mi_pop_label,
                agents_pop,
                box_pop,
                box_gpu_label,
                box_tmux_pop_box,
                settings_pop,
                ssh_count_label,
                handoff_pop,
                handoff_files,
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
                mi_cache: Arc::clone(&mi_cache),
                box_state: Arc::clone(&box_state),
            });
        });

        if !mi_dirs.is_empty() {
            let mi_cache_bg = Arc::clone(&mi_cache);
            thread::spawn(move || {
                loop {
                    for d in &mi_dirs {
                        let rep = analyze_project_mi(d);
                        if let Ok(mut map) = mi_cache_bg.lock() {
                            map.insert(d.clone(), rep);
                        }
                    }
                    thread::sleep(std::time::Duration::from_secs(25));
                }
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
