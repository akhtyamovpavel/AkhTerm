//! `Профили / SSH` and `Серверы` popovers, SSH host toggles, and remote tmux jump UI.

use std::ffi::{c_void, CString};
use std::ptr;

use crate::app_state::{self, AppState, APP};
use crate::ffi::*;
use crate::proc_radar::{RemoteTmuxItem, SshHostEntry};
use crate::{config::WorkspaceConfig, proc_radar, session, session_ops, ssh_config};
use crate::{tabs::Tab, telemetry::SessionTelemetry, ui_text as text};

pub(crate) struct SettingsPopoverWidgets {
    pub(crate) settings_pop: *mut GtkWidget,
    pub(crate) ssh_count_label: *mut GtkWidget,
    pub(crate) settings_list_vbox: *mut GtkWidget,
    pub(crate) settings_search_entry: *mut GtkWidget,
    pub(crate) ssh_toggle_btns: Vec<*mut GtkWidget>,
    pub(crate) ssh_rows: Vec<(String, *mut GtkWidget)>,
}

pub(crate) struct BoxPopoverWidgets {
    pub(crate) box_pop: *mut GtkWidget,
    pub(crate) box_gpu_label: *mut GtkWidget,
    pub(crate) box_tmux_pop_box: *mut GtkWidget,
    pub(crate) box_ssh_list_vbox: *mut GtkWidget,
    pub(crate) box_search_entry: *mut GtkWidget,
    pub(crate) box_ssh_rows: Vec<(String, *mut GtkWidget)>,
}

fn format_ssh_toggle_label(entry: &SshHostEntry) -> String {
    let mark = if entry.enabled { "✓" } else { "  " };
    let sub = if entry.hostname.is_empty() {
        String::new()
    } else {
        format!(" ({})", entry.hostname)
    };
    format!("[{mark}]  {}{sub}", entry.alias)
}

fn format_ssh_count_label(shown: usize, total: usize) -> String {
    format!(
        "Показано серверов: {shown} из {total}  (клик [✓] — опрос tmux/GPU, клик 🖥 — войти)"
    )
}

fn make_popover_scroll(width: i32, min_h: i32, max_h: i32) -> *mut GtkWidget {
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, width);
        gtk_scrolled_window_set_min_content_height(scroll, min_h);
        gtk_scrolled_window_set_max_content_height(scroll, max_h);
        gtk_widget_set_size_request(scroll, width, min_h);
        scroll
    }
}

fn ssh_search_key(entry: &SshHostEntry) -> String {
    format!("{} {} {}", entry.alias, entry.hostname, entry.user).to_lowercase()
}

fn make_ssh_toggle_btn(idx: usize, entry: &SshHostEntry) -> *mut GtkWidget {
    unsafe {
        let lbl = CString::new(format_ssh_toggle_label(entry)).unwrap();
        let b_tog = gtk_button_new_with_label(lbl.as_ptr());
        if entry.enabled {
            add_class(b_tog, "tab-unread");
        }
        app_state::connect_click(b_tog, on_toggle_ssh_host, idx as *mut c_void);
        b_tog
    }
}

fn build_settings_host_row(
    idx: usize,
    entry: &SshHostEntry,
    toggle_btns: &mut Vec<*mut GtkWidget>,
    rows: &mut Vec<(String, *mut GtkWidget)>,
) -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        let b_tog = make_ssh_toggle_btn(idx, entry);
        toggle_btns.push(b_tog);
        let conn_lbl = CString::new(format!("🖥 tmux: {}", entry.alias)).unwrap();
        let b_conn = gtk_button_new_with_label(conn_lbl.as_ptr());
        app_state::connect_click(b_conn, on_connect_ssh_host_tmux, idx as *mut c_void);
        gtk_box_pack_start(row, b_tog, 1, 1, 0);
        gtk_box_pack_end(row, b_conn, 0, 0, 0);
        rows.push((ssh_search_key(entry), row));
        row
    }
}

fn make_ssh_count_label(total: usize) -> *mut GtkWidget {
    unsafe {
        let count_c = CString::new(format_ssh_count_label(total, total)).unwrap();
        let lbl = gtk_label_new(count_c.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "ssh-count");
        lbl
    }
}

fn make_settings_header_box(ssh_count_label: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(vbox, "popover-box");
        let hdr = app_state::make_section_header(
            "УДАЛЁННЫЕ СЕРВЕРЫ ИЗ ~/.SSH/CONFIG (ОТСЛЕЖИВАНИЕ TMUX/GPU)",
        );
        gtk_box_pack_start(vbox, hdr, 0, 0, 0);
        let search = app_state::make_search_entry("🔍 Поиск сервера, хоста или IP в ~/.ssh/config...");
        app_state::connect_entry_changed(search, on_ssh_search_changed);
        gtk_box_pack_start(vbox, search, 0, 1, 0);
        gtk_box_pack_start(vbox, ssh_count_label, 0, 1, 0);
        (vbox, search)
    }
}

pub(crate) fn build_settings_popover(
    settings_btn: *mut GtkWidget,
    ssh_hosts: &[SshHostEntry],
) -> SettingsPopoverWidgets {
    unsafe {
        let settings_pop = gtk_popover_new(settings_btn);
        let ssh_count_label = make_ssh_count_label(ssh_hosts.len());
        let (vbox, settings_search_entry) = make_settings_header_box(ssh_count_label);
        let (scroll, settings_list_vbox, ssh_toggle_btns, ssh_rows) = build_settings_host_list(ssh_hosts);
        gtk_box_pack_start(vbox, scroll, 1, 1, 0);
        gtk_container_add(settings_pop, vbox);
        gtk_widget_show_all(vbox);
        app_state::connect_click(settings_btn, on_open_ssh_popover, settings_pop as *mut c_void);
        SettingsPopoverWidgets {
            settings_pop, ssh_count_label, settings_list_vbox,
            settings_search_entry, ssh_toggle_btns, ssh_rows,
        }
    }
}

fn build_settings_host_list(
    ssh_hosts: &[SshHostEntry],
) -> (*mut GtkWidget, *mut GtkWidget, Vec<*mut GtkWidget>, Vec<(String, *mut GtkWidget)>) {
    unsafe {
        let scroll = make_popover_scroll(520, 420, 460);
        let list_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        let mut toggle_btns = Vec::new();
        let mut rows = Vec::new();
        for (idx, h) in ssh_hosts.iter().enumerate() {
            let row = build_settings_host_row(idx, h, &mut toggle_btns, &mut rows);
            gtk_box_pack_start(list_vbox, row, 0, 1, 0);
        }
        gtk_container_add(scroll, list_vbox);
        (scroll, list_vbox, toggle_btns, rows)
    }
}

fn pack_box_profile_buttons(vbox: *mut GtkWidget, config: &WorkspaceConfig) {
    unsafe {
        for (index, profile) in config.profiles.iter().enumerate() {
            let label = CString::new(format!("+ {} {}", profile.icon, profile.title)).unwrap();
            let b = gtk_button_new_with_label(label.as_ptr());
            app_state::connect_click(b, on_spawn_preset, index as *mut c_void);
            gtk_box_pack_start(vbox, b, 0, 1, 0);
        }
    }
}

fn build_box_ssh_row(
    idx: usize,
    entry: &SshHostEntry,
    rows: &mut Vec<(String, *mut GtkWidget)>,
) -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        let label = CString::new(format!("ssh {}", entry.alias)).unwrap();
        let plain = gtk_button_new_with_label(label.as_ptr());
        let tmux = gtk_button_new_with_label(text::TMUX_MAIN.as_ptr());
        app_state::connect_click(plain, on_connect_ssh_host_plain, idx as *mut c_void);
        app_state::connect_click(tmux, on_connect_ssh_host_tmux, idx as *mut c_void);
        gtk_box_pack_start(row, plain, 1, 1, 0);
        gtk_box_pack_end(row, tmux, 0, 0, 0);
        rows.push((ssh_search_key(entry), row));
        row
    }
}

fn build_box_scroll_body(
    ssh_hosts: &[SshHostEntry],
) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, Vec<(String, *mut GtkWidget)>) {
    unsafe {
        let scroll = make_popover_scroll(480, 320, 380);
        let inner = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        let tmux_hdr = app_state::make_section_header("ЖИВЫЕ СЕССИИ TMUX НА СЕРВЕРАХ (КЛИК — ПРЫГНУТЬ)");
        gtk_box_pack_start(inner, tmux_hdr, 0, 0, 0);
        let box_tmux_pop_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 3);
        gtk_box_pack_start(inner, box_tmux_pop_box, 0, 1, 0);
        gtk_box_pack_start(inner, app_state::make_section_header("СЕРВЕРЫ SSH И TMUX"), 0, 0, 0);
        let box_ssh_list_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        gtk_box_pack_start(inner, box_ssh_list_vbox, 0, 1, 0);
        let rows = pack_box_ssh_rows(box_ssh_list_vbox, ssh_hosts);
        gtk_container_add(scroll, inner);
        (scroll, box_tmux_pop_box, box_ssh_list_vbox, rows)
    }
}

fn pack_box_ssh_rows(inner: *mut GtkWidget, ssh_hosts: &[SshHostEntry]) -> Vec<(String, *mut GtkWidget)> {
    let mut rows = Vec::new();
    for (idx, h) in ssh_hosts.iter().enumerate() {
        let row = build_box_ssh_row(idx, h, &mut rows);
        unsafe { gtk_box_pack_start(inner, row, 0, 1, 0) };
    }
    rows
}

fn build_box_header_vbox(config: &WorkspaceConfig) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 5);
        add_class(vbox, "popover-box");
        let gpu_c = CString::new("GPU: опрос выбранных SSH-серверов...").unwrap();
        let box_gpu_label = gtk_label_new(gpu_c.as_ptr());
        add_class(box_gpu_label, "gpu-pill");
        gtk_box_pack_start(vbox, box_gpu_label, 0, 1, 2);
        pack_box_profile_buttons(vbox, config);
        let search = app_state::make_search_entry("🔍 Поиск сессии tmux или сервера из ~/.ssh/config...");
        app_state::connect_entry_changed(search, on_box_search_changed);
        gtk_box_pack_start(vbox, search, 0, 1, 2);
        (vbox, box_gpu_label, search)
    }
}

pub(crate) fn build_box_popover(
    box_btn: *mut GtkWidget,
    config: &WorkspaceConfig,
    ssh_hosts: &[SshHostEntry],
) -> BoxPopoverWidgets {
    unsafe {
        let box_pop = gtk_popover_new(box_btn);
        let (vbox, box_gpu_label, box_search_entry) = build_box_header_vbox(config);
        let (scroll, box_tmux_pop_box, box_ssh_list_vbox, box_ssh_rows) = build_box_scroll_body(ssh_hosts);
        gtk_box_pack_start(vbox, scroll, 1, 1, 0);
        gtk_container_add(box_pop, vbox);
        gtk_widget_show_all(vbox);
        app_state::connect_click(box_btn, on_open_ssh_popover, box_pop as *mut c_void);
        BoxPopoverWidgets {
            box_pop, box_gpu_label, box_tmux_pop_box,
            box_ssh_list_vbox, box_search_entry, box_ssh_rows,
        }
    }
}

extern "C" fn on_open_ssh_popover(_btn: *mut GtkWidget, pop_ptr: *mut c_void) {
    app_state::with_app_mut(refresh_ssh_hosts_if_changed);
    if !pop_ptr.is_null() {
        unsafe { gtk_popover_popup(pop_ptr) };
    }
}

fn apply_settings_filter(rows: &[(String, *mut GtkWidget)], q: &str) -> usize {
    let mut shown = 0usize;
    for (key, row) in rows {
        let vis = q.is_empty() || key.contains(q);
        shown += usize::from(vis);
        unsafe { gtk_widget_set_visible(*row, i32::from(vis)) };
    }
    shown
}

extern "C" fn on_ssh_search_changed(entry: *mut GtkWidget, _data: *mut c_void) {
    let q = app_state::entry_text_lower(entry);
    app_state::with_app(|app| {
        let shown = apply_settings_filter(&app.ssh_rows, &q);
        set_label_text(app.ssh_count_label, &format_ssh_count_label(shown, app.ssh_rows.len()));
    });
}

fn filter_keyed_rows(rows: &[(String, *mut GtkWidget)], q: &str) {
    for (key, row) in rows {
        let vis = q.is_empty() || key.contains(q);
        unsafe { gtk_widget_set_visible(*row, i32::from(vis)) };
    }
}

fn destroy_row_widgets(rows: &mut Vec<(String, *mut GtkWidget)>) {
    for (_, row) in rows.drain(..) {
        if !row.is_null() {
            unsafe { gtk_widget_destroy(row) };
        }
    }
}

fn pack_and_show_row(parent: *mut GtkWidget, row: *mut GtkWidget) {
    unsafe {
        gtk_box_pack_start(parent, row, 0, 1, 0);
        gtk_widget_show_all(row);
    }
}

fn active_search_query(entry: *mut GtkWidget, fallback: &str) -> String {
    if !entry.is_null() {
        app_state::entry_text_lower(entry)
    } else {
        fallback.to_owned()
    }
}

fn rebuild_settings_list(app: &mut AppState) {
    if app.settings_list_vbox.is_null() {
        return;
    }
    let q = active_search_query(app.settings_search_entry, "");
    app.ssh_toggle_btns.clear();
    destroy_row_widgets(&mut app.ssh_rows);
    for (idx, h) in app.ssh_hosts.iter().enumerate() {
        let row = build_settings_host_row(idx, h, &mut app.ssh_toggle_btns, &mut app.ssh_rows);
        pack_and_show_row(app.settings_list_vbox, row);
    }
    let shown = apply_settings_filter(&app.ssh_rows, &q);
    if !app.ssh_count_label.is_null() {
        set_label_text(app.ssh_count_label, &format_ssh_count_label(shown, app.ssh_rows.len()));
    }
}

fn rebuild_box_ssh_list(app: &mut AppState) {
    if app.box_ssh_list_vbox.is_null() {
        return;
    }
    let q = active_search_query(app.box_search_entry, &app.box_filter);
    destroy_row_widgets(&mut app.box_ssh_rows);
    for (idx, h) in app.ssh_hosts.iter().enumerate() {
        let row = build_box_ssh_row(idx, h, &mut app.box_ssh_rows);
        pack_and_show_row(app.box_ssh_list_vbox, row);
    }
    filter_keyed_rows(&app.box_ssh_rows, &q);
}

pub(crate) fn refresh_ssh_hosts_if_changed(app: &mut AppState) {
    if !app.ssh_fingerprint.is_empty() && !ssh_config::has_fingerprint_changed(&app.ssh_fingerprint) {
        return;
    }
    reload_ssh_hosts_now(app);
}

pub(crate) fn reload_ssh_hosts_now(app: &mut AppState) {
    let (mut next_hosts, next_fp) =
        ssh_config::load_ssh_hosts_with_fingerprint(&app.config.watch, &app.config.startup);
    for entry in &mut next_hosts {
        if let Some(prev) = app.ssh_hosts.iter().find(|h| h.alias == entry.alias) {
            entry.enabled = prev.enabled;
        }
    }
    app.ssh_fingerprint = next_fp;
    if app.ssh_hosts == next_hosts {
        return;
    }
    app.ssh_hosts = next_hosts;
    rebuild_settings_list(app);
    rebuild_box_ssh_list(app);
    sync_enabled_ssh_hosts(app);
}

extern "C" fn on_box_search_changed(entry: *mut GtkWidget, _data: *mut c_void) {
    let q = app_state::entry_text_lower(entry);
    app_state::with_app_mut(|app| {
        app.box_filter.clone_from(&q);
        for (_, _, key, _, b_pop) in &app.tmux_jump_btns {
            let vis = q.is_empty() || key.contains(&q);
            unsafe { gtk_widget_set_visible(*b_pop, i32::from(vis)) };
        }
        filter_keyed_rows(&app.box_ssh_rows, &q);
    });
}

fn sync_toggle_button(btn_w: *mut GtkWidget, entry: &SshHostEntry) {
    let lbl = format_ssh_toggle_label(entry);
    set_class(btn_w, "tab-unread", entry.enabled);
    if let Ok(c) = CString::new(lbl) {
        unsafe { gtk_button_set_label(btn_w, c.as_ptr()) };
    }
}

fn sync_enabled_ssh_hosts(app: &AppState) {
    proc_radar::save_ssh_hosts(&app.ssh_hosts);
    let enabled: Vec<String> =
        app.ssh_hosts.iter().filter(|h| h.enabled).map(|h| h.alias.clone()).collect();
    if let Ok(mut g) = app.enabled_hosts_shared.lock() {
        *g = enabled;
    }
}

extern "C" fn on_toggle_ssh_host(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let idx = idx_ptr as usize;
    app_state::with_app_mut(|app| {
        if let Some(entry) = app.ssh_hosts.get_mut(idx) {
            entry.enabled = !entry.enabled;
            if let Some(&btn_w) = app.ssh_toggle_btns.get(idx) {
                sync_toggle_button(btn_w, entry);
            }
        }
        sync_enabled_ssh_hosts(app);
    });
}

extern "C" fn on_connect_ssh_host_tmux(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    connect_ssh_host(idx_ptr as usize, Some("main"));
}

extern "C" fn on_connect_ssh_host_plain(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    connect_ssh_host(idx_ptr as usize, None);
}

fn connect_ssh_host(idx: usize, tmux: Option<&str>) {
    let target = app_state::with_app(|app| {
        app.ssh_hosts
            .get(idx)
            .map(|h| (app.settings_pop, app.box_pop, h.alias.clone()))
    })
    .flatten();
    if let Some((settings_pop, box_pop, host)) = target {
        popdown_if_live(settings_pop);
        popdown_if_live(box_pop);
        session_ops::add_remote_session(&host, tmux);
    }
}

fn popdown_if_live(pop: *mut GtkWidget) {
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
}

extern "C" fn on_jump_tmux(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let idx = idx_ptr as usize;
    let target = app_state::with_app(|app| {
        let found = app
            .tmux_jump_btns
            .get(idx)
            .map(|(host, sname, _, _, _)| (host.clone(), sname.clone()));
        (app.box_pop, found)
    });
    if let Some((box_pop, chosen)) = target {
        popdown_if_live(box_pop);
        if let Some((host, session)) = chosen {
            jump_to_remote_tmux(&host, &session);
        }
    }
}

fn find_remote_tmux_session(target_host: &str) -> Option<(usize, bool)> {
    app_state::with_app(|app| {
        app.sessions
            .iter()
            .find(|s| s.remote().is_some_and(|r| r.host == target_host && r.tmux_session.is_some()))
            .map(|s| (s.id, s.child_pid > 0))
    })
    .flatten()
}

pub(crate) fn jump_to_remote_tmux(target_host: &str, target_session: &str) {
    if target_session.is_empty() || target_host.is_empty() {
        return;
    }
    let Some((sid, connected)) = find_remote_tmux_session(target_host) else {
        session_ops::add_remote_session(target_host, Some(target_session));
        return;
    };
    retarget_remote_session(sid, target_session);
    if connected {
        proc_radar::switch_remote_tmux_session(target_host, target_session);
        session_ops::select_session(sid);
    } else {
        session_ops::reconnect_session(sid);
    }
}

fn reset_retargeted_session(s: &mut session::TermSession, target_session: &str) {
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
    session::refresh_smart_title(s);
}

fn retarget_remote_session(sid: usize, target_session: &str) {
    app_state::with_app_mut(|app| {
        app.remote_telemetry.reset_session(sid);
        if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid) {
            reset_retargeted_session(s, target_session);
        }
    });
}

fn destroy_tmux_buttons(app: &mut AppState) {
    for (_, _, _, b1, b2) in app.tmux_jump_btns.drain(..) {
        unsafe {
            gtk_widget_destroy(b1);
            gtk_widget_destroy(b2);
        }
    }
}

fn append_tmux_jump_row(app: &mut AppState, idx: usize, item: &RemoteTmuxItem) {
    let label_txt = format!("↳ {}", item.display_line);
    let search_key = format!("{} {} {}", item.host, item.session_name, item.display_line).to_lowercase();
    let Ok(lbl_c) = CString::new(label_txt) else {
        return;
    };
    let desc = item.display_line.split_once("] ").map_or(item.session_name.as_str(), |(_, t)| t);
    let b_side = app.sidebar.append_tmux(&format!("{} · {desc}", item.host), &item.display_line, idx, on_jump_tmux);
    let b_pop = make_tmux_pop_button(&lbl_c, idx, app.box_tmux_pop_box, &search_key, &app.box_filter);
    app.tmux_jump_btns.push((item.host.clone(), item.session_name.clone(), search_key, b_side, b_pop));
}

fn make_tmux_pop_button(
    lbl_c: &CString,
    idx: usize,
    parent: *mut GtkWidget,
    search_key: &str,
    box_filter: &str,
) -> *mut GtkWidget {
    unsafe {
        let b_pop = gtk_button_new_with_label(lbl_c.as_ptr());
        add_class(b_pop, "tmux-jump-btn");
        app_state::connect_click(b_pop, on_jump_tmux, idx as *mut c_void);
        gtk_box_pack_start(parent, b_pop, 0, 1, 0);
        gtk_widget_show_all(b_pop);
        let vis = box_filter.is_empty() || search_key.contains(box_filter);
        gtk_widget_set_visible(b_pop, i32::from(vis));
        b_pop
    }
}

fn update_tmux_active_classes(app: &AppState, items: &[RemoteTmuxItem]) {
    for (idx, (_, _, _, b_side, b_pop)) in app.tmux_jump_btns.iter().enumerate() {
        let active = items.get(idx).is_some_and(|item| item.is_active);
        set_class(*b_side, "sidebar-tmux-current", active);
        set_class(*b_pop, "tmux-jump-active", active);
    }
}

pub(crate) fn sync_tmux_jump_buttons(app: &mut AppState, items: &[RemoteTmuxItem]) {
    let keys: Vec<String> = items.iter().map(|i| i.display_line.clone()).collect();
    if app.last_tmux_keys != keys {
        destroy_tmux_buttons(app);
        app.last_tmux_keys = keys;
        for (idx, item) in items.iter().enumerate() {
            append_tmux_jump_row(app, idx, item);
        }
    }
    update_tmux_active_classes(app, items);
}

pub(crate) fn dismiss_tab_menu() {
    let pop = APP.with(|cell| cell.borrow().as_ref().map(|app| app.box_pop));
    if let Some(pop) = pop {
        popdown_if_live(pop);
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
        session_ops::open_tab(tab);
    }
}

#[cfg(test)]
pub(crate) fn mount_popovers_for_test(
    a: &mut AppState,
    box_btn: *mut GtkWidget,
    servers_btn: *mut GtkWidget,
) {
    a.box_btn = box_btn;
    a.servers_btn = servers_btn;
    a.config.watch.clear();
    a.config.startup.clear();
    let (hosts, fp) = ssh_config::load_ssh_hosts_with_fingerprint(&a.config.watch, &a.config.startup);
    let sp = build_settings_popover(servers_btn, &hosts);
    let bp = build_box_popover(box_btn, &a.config, &hosts);
    a.ssh_hosts = hosts;
    a.ssh_fingerprint = fp;
    a.settings_pop = sp.settings_pop;
    a.settings_list_vbox = sp.settings_list_vbox;
    a.settings_search_entry = sp.settings_search_entry;
    a.ssh_count_label = sp.ssh_count_label;
    a.ssh_toggle_btns = sp.ssh_toggle_btns;
    a.ssh_rows = sp.ssh_rows;
    a.box_pop = bp.box_pop;
    a.box_gpu_label = bp.box_gpu_label;
    a.box_tmux_pop_box = bp.box_tmux_pop_box;
    a.box_ssh_list_vbox = bp.box_ssh_list_vbox;
    a.box_search_entry = bp.box_search_entry;
    a.box_ssh_rows = bp.box_ssh_rows;
}
