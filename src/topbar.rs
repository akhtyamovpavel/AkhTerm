//! Top header bar, toolbar construction, zoom/theme controls, and UI refresh planning.

use std::collections::HashMap;
use std::ffi::{c_int, c_void, CStr, CString};
use std::{ptr, time::Instant};

use crate::app_state::{self, AppState, APP};
use crate::ffi::*;
use crate::git_status::{self, GitRepoStatus};
use crate::mi::ProjectMiReport;
use crate::{git_wt, mi_watch, pane_actions, paths, session, session_ops, sidebar, split_pane};
use crate::{tab_strip, telemetry, theme, theme_mode::ThemeMode, ui_text as text};

pub(crate) struct TopbarWidgets {
    pub(crate) topbar: *mut GtkWidget,
    pub(crate) tab_strip: tab_strip::TabStrip,
    pub(crate) box_btn: *mut GtkWidget,
    pub(crate) theme_btn: *mut GtkWidget,
    pub(crate) zoom_btn: *mut GtkWidget,
    pub(crate) reconnect_btn: *mut GtkWidget,
    pub(crate) split_vert_btn: *mut GtkWidget,
    pub(crate) split_horiz_btn: *mut GtkWidget,
    pub(crate) settings_btn: *mut GtkWidget,
    pub(crate) handoff_btn: *mut GtkWidget,
    pub(crate) git_btn: *mut GtkWidget,
    pub(crate) mi_btn: *mut GtkWidget,
    pub(crate) mi_pop: *mut GtkWidget,
    pub(crate) mi_pop_label: *mut GtkWidget,
    pub(crate) mi_refactor_btn: *mut GtkWidget,
    pub(crate) tokens_menu: telemetry::TokensMenu,
    pub(crate) agents_pop: *mut GtkWidget,
}

pub(crate) fn build_header_bar(win: *mut GtkWidget) {
    unsafe {
        let header = gtk_header_bar_new();
        add_class(header, "window-header");
        let brand = gtk_label_new(text::APP_NAME.as_ptr());
        add_class(brand, "brand-label");
        gtk_header_bar_set_custom_title(header, brand);
        gtk_header_bar_set_show_close_button(header, 1);
        gtk_window_set_titlebar(win, header);
    }
}

fn add_toolbar_action(
    toolbar: *mut GtkWidget,
    label: &CStr,
    tooltip: &CStr,
    on_click: extern "C" fn(*mut GtkWidget, *mut c_void),
) -> *mut GtkWidget {
    unsafe {
        let button = gtk_button_new_with_label(label.as_ptr());
        gtk_widget_set_tooltip_text(button, tooltip.as_ptr());
        app_state::connect_click(button, on_click, ptr::null_mut());
        gtk_box_pack_end(toolbar, button, 0, 0, 0);
        button
    }
}

fn add_toolbar_sq_action(
    toolbar: *mut GtkWidget,
    label: &CStr,
    tooltip: &CStr,
    on_click: extern "C" fn(*mut GtkWidget, *mut c_void),
) -> *mut GtkWidget {
    let button = add_toolbar_action(toolbar, label, tooltip, on_click);
    add_class(button, "btn-icon-sq");
    button
}

fn add_end_button(
    toolbar: *mut GtkWidget,
    label: &CStr,
    tip: Option<&CStr>,
) -> *mut GtkWidget {
    unsafe {
        let button = gtk_button_new_with_label(label.as_ptr());
        if let Some(t) = tip {
            gtk_widget_set_tooltip_text(button, t.as_ptr());
        }
        gtk_box_pack_end(toolbar, button, 0, 0, 0);
        button
    }
}

fn build_mi_popover_box(mi_pop_label: *mut GtkWidget, mi_refactor_btn: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let mi_pop_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(mi_pop_box, "popover-box");
        gtk_label_set_xalign(mi_pop_label, 0.0);
        gtk_box_pack_start(mi_pop_box, mi_pop_label, 0, 1, 0);
        gtk_widget_set_tooltip_text(mi_refactor_btn, text::MI_REFACTOR_TIP.as_ptr());
        gtk_widget_set_sensitive(mi_refactor_btn, 0);
        app_state::connect_click(mi_refactor_btn, on_mi_refactor_clicked, ptr::null_mut());
        gtk_box_pack_start(mi_pop_box, mi_refactor_btn, 0, 1, 0);
        mi_pop_box
    }
}

fn build_mi_popover(mi_btn: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let mi_pop = gtk_popover_new(mi_btn);
        let mi_pop_label = gtk_label_new(text::MI_EMPTY.as_ptr());
        let mi_refactor_btn = gtk_button_new_with_label(text::MI_REFACTOR.as_ptr());
        let mi_pop_box = build_mi_popover_box(mi_pop_label, mi_refactor_btn);
        gtk_container_add(mi_pop, mi_pop_box);
        gtk_widget_show_all(mi_pop_box);
        connect_popover(mi_btn, mi_pop);
        (mi_pop, mi_pop_label, mi_refactor_btn)
    }
}

fn add_worktree_agent_btn(wt_row: *mut GtkWidget, code: usize, cmd: &str) {
    unsafe {
        let lbl = CString::new(format!("⎇ {}", git_wt::agent_slug_from_cmd(cmd))).unwrap();
        let b = gtk_button_new_with_label(lbl.as_ptr());
        gtk_widget_set_tooltip_text(b, text::AGENT_WORKTREE_TIP.as_ptr());
        app_state::connect_click(b, pane_actions::agent_worktree_cmd_cb(), code as *mut c_void);
        gtk_box_pack_start(wt_row, b, 1, 1, 0);
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
            add_worktree_agent_btn(wt_row, code, cmd);
        }
        gtk_box_pack_start(agents_box, wt_row, 0, 1, 0);
    }
}

fn pack_agent_buttons(agents_box: *mut GtkWidget) {
    unsafe {
        for (code, (label, _)) in text::AGENT_ACTIONS.iter().enumerate() {
            let b = gtk_button_new_with_label(label.as_ptr());
            app_state::connect_click(b, pane_actions::agent_cmd_cb(), code as *mut c_void);
            gtk_box_pack_start(agents_box, b, 0, 1, 0);
        }
    }
}

fn build_agents_popover(agents_btn: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let agents_pop = gtk_popover_new(agents_btn);
        let agents_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        add_class(agents_box, "popover-box");
        pack_agent_buttons(agents_box);
        pack_worktree_agent_actions(agents_box);
        gtk_container_add(agents_pop, agents_box);
        gtk_widget_show_all(agents_box);
        connect_popover(agents_btn, agents_pop);
        agents_pop
    }
}

fn pack_left_controls(topbar: *mut GtkWidget, initial_theme: ThemeMode) -> [*mut GtkWidget; 5] {
    let _side = add_toolbar_sq_action(topbar, text::TOGGLE_SIDEBAR, text::TOGGLE_SIDEBAR_TIP, on_toggle_sidebar);
    let theme_btn = add_toolbar_sq_action(topbar, initial_theme.toggle_icon(), initial_theme.toggle_tooltip(), on_toggle_theme);
    let split_horiz_btn = add_toolbar_sq_action(topbar, text::SPLIT_HORIZ, text::SPLIT_HORIZ_TIP, session_ops::split_horiz_cb());
    let split_vert_btn = add_toolbar_sq_action(topbar, text::SPLIT_VERT, text::SPLIT_VERT_TIP, session_ops::split_vert_cb());
    let reconnect_btn = add_toolbar_action(topbar, text::RECONNECT, text::RECONNECT_TIP, session_ops::reconnect_active_cb());
    add_toolbar_action(topbar, text::INTERRUPT, text::INTERRUPT_TIP, session_ops::sigint_clicked_cb());
    add_toolbar_action(topbar, text::PASTE_IMAGE, text::PASTE_IMAGE_TIP, pane_actions::paste_image_clicked_cb());
    let _zin = add_toolbar_sq_action(topbar, text::ZOOM_IN, text::ZOOM_IN_TIP, on_zoom_in);
    let zoom_btn = add_toolbar_action(topbar, text::ZOOM_RESET, text::ZOOM_RESET_TIP, on_zoom_reset);
    let _zout = add_toolbar_sq_action(topbar, text::ZOOM_OUT, text::ZOOM_OUT_TIP, on_zoom_out);
    [theme_btn, split_horiz_btn, split_vert_btn, reconnect_btn, zoom_btn]
}

fn create_topbar_shell(root_vbox: *mut GtkWidget) -> (*mut GtkWidget, tab_strip::TabStrip, *mut GtkWidget) {
    unsafe {
        let topbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 5);
        add_class(topbar, "topbar");
        gtk_box_pack_start(root_vbox, topbar, 0, 1, 0);
        let tab_strip = tab_strip::TabStrip::new(root_vbox, session_ops::new_tab_cb());
        let box_btn = gtk_button_new_with_label(text::PROFILES.as_ptr());
        gtk_box_pack_start(topbar, box_btn, 0, 0, 0);
        (topbar, tab_strip, box_btn)
    }
}

fn pack_right_menus(
    topbar: *mut GtkWidget,
) -> ([*mut GtkWidget; 6], telemetry::TokensMenu) {
    let settings_btn = add_end_button(topbar, text::SERVERS, None);
    let handoff_btn = add_end_button(topbar, text::HANDOFF, Some(text::HANDOFF_TIP));
    let agents_btn = add_end_button(topbar, text::AGENTS, None);
    let git_btn = add_end_button(topbar, text::GIT_MENU, Some(text::GIT_MENU_TIP));
    let mi_btn = add_end_button(topbar, text::MAINTAINABILITY, None);
    let tokens_menu = telemetry::TokensMenu::new(topbar, on_tokens_open);
    ([settings_btn, handoff_btn, agents_btn, git_btn, mi_btn, ptr::null_mut()], tokens_menu)
}

pub(crate) fn build_topbar_widgets(
    root_vbox: *mut GtkWidget,
    initial_theme: ThemeMode,
) -> TopbarWidgets {
    let (topbar, tab_strip, box_btn) = create_topbar_shell(root_vbox);
    let [theme_btn, split_horiz_btn, split_vert_btn, reconnect_btn, zoom_btn] =
        pack_left_controls(topbar, initial_theme);
    let ([settings_btn, handoff_btn, agents_btn, git_btn, mi_btn, _], tokens_menu) =
        pack_right_menus(topbar);
    let (mi_pop, mi_pop_label, mi_refactor_btn) = build_mi_popover(mi_btn);
    let agents_pop = build_agents_popover(agents_btn);
    TopbarWidgets {
        topbar, tab_strip, box_btn, theme_btn, zoom_btn, reconnect_btn,
        split_vert_btn, split_horiz_btn, settings_btn, handoff_btn, git_btn,
        mi_btn, mi_pop, mi_pop_label, mi_refactor_btn, tokens_menu, agents_pop,
    }
}

pub fn set_zoom(new_scale: f64) {
    let clamped = new_scale.clamp(0.5, 3.0);
    let mut vtes = Vec::new();
    let zoom_btn = app_state::with_app_mut(|app| {
        app.font_scale = clamped;
        for s in &app.sessions {
            s.panes.collect_vtes(s.vte, &mut vtes);
        }
        app.zoom_btn
    });
    split_pane::apply_zoom_scale(&vtes, clamped, zoom_btn);
    refresh_ui();
}

pub fn adjust_zoom(delta: f64) {
    let target = app_state::with_app(|app| app.font_scale + delta).unwrap_or(1.0);
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
    let toggle = app_state::with_app_mut(|app| {
        app.sidebar_visible = !app.sidebar_visible;
        (app.sidebar.panel_widget(), app.sidebar_visible)
    });
    if let Some((panel, vis)) = toggle.filter(|(w, _)| !w.is_null()) {
        unsafe { gtk_widget_set_visible(panel, i32::from(vis)) };
    }
}

fn collect_theme_targets(mode: ThemeMode) -> Option<(*mut GtkWidget, Vec<*mut GtkWidget>)> {
    app_state::with_app_mut(|app| {
        app.theme_mode = mode;
        let mut vtes = Vec::new();
        for s in &app.sessions {
            s.panes.collect_vtes(s.vte, &mut vtes);
        }
        (app.theme_btn, vtes)
    })
}

pub(crate) fn apply_live_theme(mode: ThemeMode, persist: bool) {
    let targets = collect_theme_targets(mode);
    if persist {
        crate::theme_mode::persist_theme_mode(mode);
    }
    theme::apply_gtk_theme_mode(mode);
    if let Some((theme_btn, vtes)) = targets {
        theme::apply_vte_list_colors(&vtes, mode);
        theme::sync_theme_button(theme_btn, mode);
    }
    refresh_ui();
}

pub(crate) fn toggle_theme() {
    app_state::close_open_palette();
    if let Some(mode) = app_state::with_app(|app| app.theme_mode.toggled()) {
        apply_live_theme(mode, true);
        app_state::focus_active_vte();
    }
}

extern "C" fn on_toggle_theme(_btn: *mut GtkWidget, _data: *mut c_void) {
    toggle_theme();
}

pub(crate) fn toggle_window_square() {
    let toggle = app_state::with_app_mut(|app| {
        app.is_fullscreen = !app.is_fullscreen;
        (app.win, app.is_fullscreen)
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

extern "C" fn on_popup_popover(_btn: *mut GtkWidget, pop_ptr: *mut c_void) {
    if !pop_ptr.is_null() {
        unsafe { gtk_popover_popup(pop_ptr) };
    }
}

pub(crate) fn connect_popover(btn: *mut GtkWidget, pop: *mut GtkWidget) {
    app_state::connect_click(btn, on_popup_popover, pop as *mut c_void);
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
            grade_cls: cur_mi.map(|rep| mi_watch::mi_grade_class(rep.grade)),
            texts: cur_mi.map(|rep| {
                (
                    mi_watch::format_mi_button_label(rep),
                    mi_watch::format_mi_popover_markup(rep, self_mi),
                )
            }),
        }
    }

    fn apply(self) {
        let [btn, pop_label, refactor_btn] = self.widgets;
        clear_mi_classes(btn);
        unsafe { gtk_widget_set_sensitive(refactor_btn, i32::from(self.grade_cls.is_some())) };
        let Some((cls, (lbl, markup))) = self.grade_cls.zip(self.texts) else {
            apply_empty_mi_labels(btn, pop_label);
            return;
        };
        apply_filled_mi_labels(btn, pop_label, cls, &lbl, &markup);
    }
}

fn clear_mi_classes(btn: *mut GtkWidget) {
    for cls in ["btn-mi-a", "btn-mi-b", "btn-mi-c"] {
        remove_class(btn, cls);
    }
}

fn apply_empty_mi_labels(btn: *mut GtkWidget, pop_label: *mut GtkWidget) {
    unsafe {
        gtk_button_set_label(btn, text::MAINTAINABILITY.as_ptr());
        gtk_label_set_text(pop_label, text::MI_EMPTY.as_ptr());
    }
}

fn apply_filled_mi_labels(
    btn: *mut GtkWidget,
    pop_label: *mut GtkWidget,
    cls: &str,
    lbl: &str,
    markup: &str,
) {
    add_class(btn, cls);
    if let Ok(c) = CString::new(lbl) {
        unsafe { gtk_button_set_label(btn, c.as_ptr()) };
    }
    if let Ok(c) = CString::new(markup) {
        unsafe { gtk_label_set_markup(pop_label, c.as_ptr()) };
    }
}

pub(crate) fn mi_ready_cb() -> extern "C" fn(*mut c_void) -> c_int {
    on_mi_ready
}

extern "C" fn on_mi_ready(_data: *mut c_void) -> c_int {
    refresh_ui();
    0
}

fn active_mi_refactor_target(app: &AppState) -> (*mut GtkWidget, Option<String>) {
    let home = paths::home_dir();
    let mi_map = app.mi_scheduler.snapshot();
    let active_cwd = app
        .sessions
        .iter()
        .find(|s| s.id == app.active_id)
        .and_then(session::TermSession::active_local_cwd);
    let (cur_mi, _) = mi_watch::resolve_active_mi(
        &mi_map,
        active_cwd.as_deref(),
        app.config.mi_root.as_deref(),
        &home,
    );
    (app.mi_pop, cur_mi.map(mi_watch::build_mi_refactor_prompt))
}

extern "C" fn on_mi_refactor_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let Some((mi_pop, prompt)) = app_state::with_app(active_mi_refactor_target) else {
        return;
    };
    if !mi_pop.is_null() {
        unsafe { gtk_popover_popdown(mi_pop) };
    }
    if let Some(prompt) = prompt {
        session_ops::send_to_active(&prompt);
    }
}

fn collect_tokens_view(
    app: &AppState,
    now: Instant,
) -> (telemetry::TokensMenu, telemetry::TokensViewData) {
    (
        app.tokens_menu,
        telemetry::build_sessions_tokens_view(&app.sessions, app.active_id, app.theme_mode, now),
    )
}

#[cfg(test)]
pub(crate) fn tokens_open_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_tokens_open
}

extern "C" fn on_tokens_open(_btn: *mut GtkWidget, _pop_ptr: *mut c_void) {
    let Some((menu, view)) = app_state::with_app(|app| collect_tokens_view(app, Instant::now()))
    else {
        return;
    };
    menu.apply(&view);
    if !menu.popover().is_null() {
        unsafe { gtk_popover_popup(menu.popover()) };
    }
}

fn session_git_badge(
    s: &session::TermSession,
    local_dir: Option<&str>,
    git_map: &HashMap<String, GitRepoStatus>,
    home: &str,
) -> String {
    local_dir
        .and_then(|dir| {
            git_status::lookup_git_status(git_map, dir, home)
                .or_else(|| git_status::lookup_git_status(git_map, &s.cwd, home))
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

struct SessionViews {
    rows: Vec<sidebar::RowUpdate>,
    tabs: Vec<tab_strip::TabUpdate>,
    active_cwd: Option<String>,
    can_reconnect: bool,
    counts: [usize; 3],
}

fn prepare_session_views(
    app: &AppState,
    mi_map: &HashMap<String, ProjectMiReport>,
    git_map: &HashMap<String, GitRepoStatus>,
    home: &str,
) -> SessionViews {
    let mut out = SessionViews {
        rows: Vec::with_capacity(app.sessions.len()),
        tabs: Vec::with_capacity(app.sessions.len()),
        active_cwd: None,
        can_reconnect: false,
        counts: [0usize; 3],
    };
    for (idx, s) in app.sessions.iter().enumerate() {
        push_session_view(app, s, idx, mi_map, git_map, home, &mut out);
    }
    out
}

fn session_mi_badge(
    s: &session::TermSession,
    local_dir: Option<&str>,
    mi_map: &HashMap<String, ProjectMiReport>,
    home: &str,
) -> Option<(f64, &'static str)> {
    local_dir
        .and_then(|d| {
            mi_watch::lookup_mi(mi_map, d, home)
                .or_else(|| mi_watch::lookup_mi(mi_map, &s.cwd, home))
        })
        .map(|r| (r.avg_mi, r.grade))
}

fn accumulate_session_counts(s: &session::TermSession, is_act: bool, out: &mut SessionViews, local_dir: &Option<String>) {
    if is_act {
        out.can_reconnect = s.remote().is_some();
        out.active_cwd = local_dir.clone();
    }
    out.counts[0] += usize::from(s.state == session::WorkState::Working);
    out.counts[1] += usize::from(s.unread);
    out.counts[2] += usize::from(!s.proc_info.ssh_host.is_empty());
}

fn push_session_view(
    app: &AppState,
    s: &session::TermSession,
    idx: usize,
    mi_map: &HashMap<String, ProjectMiReport>,
    git_map: &HashMap<String, GitRepoStatus>,
    home: &str,
    out: &mut SessionViews,
) {
    let is_act = s.id == app.active_id;
    let local_dir = s.active_local_cwd();
    accumulate_session_counts(s, is_act, out, &local_dir);
    let mi_badge = session_mi_badge(s, local_dir.as_deref(), mi_map, home);
    let git_badge = session_git_badge(s, local_dir.as_deref(), git_map, home);
    out.rows.push(s.sidebar_row.prepare_update(idx, s, mi_badge, &git_badge, is_act));
    out.tabs.push(s.top_tab.prepare_update(s, is_act));
}

struct UiRefreshPlan {
    rows: Vec<sidebar::RowUpdate>,
    tabs: Vec<tab_strip::TabUpdate>,
    sidebar: sidebar::SidebarUpdate,
    btns: [*mut GtkWidget; 3],
    flags: (bool, bool),
    git: (*mut GtkWidget, bool, Option<GitRepoStatus>),
    mi: MiUiPlan,
    tokens: (telemetry::TokensMenu, telemetry::TokensViewData),
    footer: Option<(*mut GtkWidget, String)>,
}

impl UiRefreshPlan {
    fn build(app: &AppState) -> Self {
        let home = paths::home_dir();
        let local_cwds = app_state::collect_local_cwds(app);
        app.mi_scheduler.sync_targets(&app.config.mi_directories(), &local_cwds, &home);
        app.git_scheduler.sync_targets(&local_cwds, &home);
        let mi_map = app.mi_scheduler.snapshot();
        let git_map = app.git_scheduler.snapshot();
        let views = prepare_session_views(app, &mi_map, &git_map, &home);
        Self::from_views(app, views, &mi_map, &git_map, &home)
    }

    fn from_views(
        app: &AppState,
        views: SessionViews,
        mi_map: &HashMap<String, ProjectMiReport>,
        git_map: &HashMap<String, GitRepoStatus>,
        home: &str,
    ) -> Self {
        let cwd = views.active_cwd.as_deref();
        let active_git = cwd.and_then(|d| git_status::lookup_git_status(git_map, d, home)).cloned();
        let (cur_mi, self_mi) =
            mi_watch::resolve_active_mi(mi_map, cwd, app.config.mi_root.as_deref(), home);
        Self::assemble(app, views, active_git, cur_mi, self_mi)
    }

    fn assemble(
        app: &AppState,
        views: SessionViews,
        active_git: Option<GitRepoStatus>,
        cur_mi: Option<&ProjectMiReport>,
        self_mi: Option<&ProjectMiReport>,
    ) -> Self {
        let [working, ready, ssh_count] = views.counts;
        Self {
            rows: views.rows, tabs: views.tabs, sidebar: session_ops::prepare_sidebar_filter(app),
            btns: [app.reconnect_btn, app.split_vert_btn, app.split_horiz_btn],
            flags: (views.can_reconnect, !app.sessions.is_empty() && !views.can_reconnect),
            git: (app.git_menu.button(), views.active_cwd.is_some(), active_git),
            mi: MiUiPlan::from_state(app, cur_mi, self_mi),
            tokens: collect_tokens_view(app, Instant::now()),
            footer: prepare_footer_update(app, working, ready, ssh_count),
        }
    }

    fn apply(self) {
        self.rows.into_iter().for_each(sidebar::RowUpdate::apply);
        self.tabs.into_iter().for_each(tab_strip::TabUpdate::apply);
        self.sidebar.apply();
        apply_toolbar_sensitivity(self.btns, self.flags);
        git_status::update_git_button(self.git.0, self.git.1, self.git.2.as_ref());
        self.mi.apply();
        self.tokens.0.apply(&self.tokens.1);
        if let Some((lbl, text)) = self.footer {
            set_label_text(lbl, &text);
        }
    }
}

fn apply_toolbar_sensitivity(btns: [*mut GtkWidget; 3], flags: (bool, bool)) {
    let [reconnect_btn, split_vert_btn, split_horiz_btn] = btns;
    unsafe {
        gtk_widget_set_sensitive(reconnect_btn, i32::from(flags.0));
        gtk_widget_set_sensitive(split_vert_btn, i32::from(flags.1));
        gtk_widget_set_sensitive(split_horiz_btn, i32::from(flags.1));
    }
}

pub fn refresh_ui() {
    let plan = APP.with(|cell| {
        let Ok(borrow) = cell.try_borrow() else {
            return None;
        };
        borrow.as_ref().map(UiRefreshPlan::build)
    });
    if let Some(plan) = plan {
        plan.apply();
    }
}
