//! Keyboard shortcut dispatch, command palette routing, and scrollback search hooks.

use std::ffi::{c_int, c_uint, c_void};
use std::ptr;

use crate::app_state::{self, AppState, APP};
use crate::ffi::*;
use crate::{approval, context_menu, git_status, palette, pane_actions, paste_route};
use crate::{paths, search_bar, session, session_ops, split_pane, ssh_ui, tab_order};
use crate::{tabs::Tab, theme_mode, topbar, ui_text as text};

fn extract_key_event(event: *const GdkEvent) -> Option<(c_uint, c_uint, u16)> {
    let mut keyval: c_uint = 0;
    let mut state: c_uint = 0;
    let mut keycode: u16 = 0;
    unsafe {
        if gdk_event_get_keyval(event, &mut keyval) == 0 {
            return None;
        }
        gdk_event_get_state(event, &mut state);
        gdk_event_get_keycode(event, &mut keycode);
    }
    Some((keyval, state, keycode))
}

fn is_zoom_in_key(keyval: c_uint, keycode: u16) -> bool {
    keyval == b'+' as u32 || keyval == b'=' as u32 || keyval == 0xffab || keycode == 21 || keycode == 86
}

fn is_zoom_out_key(keyval: c_uint, keycode: u16) -> bool {
    keyval == b'-' as u32 || keyval == b'_' as u32 || keyval == 0xffad || keycode == 20 || keycode == 82
}

fn is_zoom_reset_key(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    (state & GDK_SHIFT_MASK) == 0 && (keyval == b'0' as u32 || keyval == 0xffb0 || keycode == 19)
}

fn handle_zoom_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if is_zoom_in_key(keyval, keycode) {
        topbar::adjust_zoom(0.1);
        return true;
    }
    if is_zoom_out_key(keyval, keycode) {
        topbar::adjust_zoom(-0.1);
        return true;
    }
    if is_zoom_reset_key(keyval, state, keycode) {
        topbar::set_zoom(1.0);
        return true;
    }
    false
}

fn handle_alt_tab_shortcut(keyval: c_uint, state: c_uint) -> bool {
    let Some(idx) = tab_order::alt_tab_index(keyval, state) else {
        return false;
    };
    let target_sid = app_state::with_app(|app| app.sessions.get(idx).map(|s| s.id)).flatten();
    if let Some(sid) = target_sid {
        session_ops::select_session(sid);
        return true;
    }
    false
}

fn handle_plain_ctrl_clipboard(keyval: c_uint, state: c_uint, keycode: u16) -> Option<c_int> {
    if !paste_route::is_plain_ctrl_shortcut(state) {
        return None;
    }
    if paste_route::is_c_key(keyval, keycode) {
        return Some(c_int::from(pane_actions::copy_active_selection(true)));
    }
    if paste_route::is_v_key(keyval, keycode) && pane_actions::paste_plain_ctrl_v() {
        return Some(1);
    }
    None
}

fn handle_ctrl_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> Option<c_int> {
    if (state & GDK_CONTROL_MASK) == 0 {
        return None;
    }
    if crate::settings_dialog::is_settings_shortcut(keyval, state, keycode) {
        topbar::open_settings_for_app();
        return Some(1);
    }
    if handle_zoom_shortcut(keyval, state, keycode) {
        return Some(1);
    }
    handle_plain_ctrl_clipboard(keyval, state, keycode)
}

fn handle_ctrl_shift_clipboard(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if !paste_route::is_ctrl_shift_shortcut(state) {
        return false;
    }
    if paste_route::is_c_key(keyval, keycode) {
        let _ = pane_actions::copy_active_selection(false);
        return true;
    }
    if paste_route::is_v_key(keyval, keycode) {
        let _ = pane_actions::paste_smart(false);
        return true;
    }
    false
}

fn handle_ctrl_shift_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if (state & GDK_CONTROL_MASK) == 0 || (state & GDK_SHIFT_MASK) == 0 {
        return false;
    }
    handle_tab_shortcut(keyval, state, keycode) || handle_ctrl_shift_clipboard(keyval, state, keycode)
}

pub(crate) fn window_key_press_cb() -> extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int {
    on_window_key_press
}

fn alt_arrow_direction(keyval: c_uint) -> Option<split_pane::FocusDir> {
    match keyval {
        0xff51 => Some(split_pane::FocusDir::Left),
        0xff52 => Some(split_pane::FocusDir::Up),
        0xff53 => Some(split_pane::FocusDir::Right),
        0xff54 => Some(split_pane::FocusDir::Down),
        _ => None,
    }
}

fn handle_alt_arrow_focus(keyval: c_uint, state: c_uint) -> bool {
    let mods = GDK_CONTROL_MASK | GDK_SHIFT_MASK | GDK_MOD1_MASK;
    if (state & mods) != GDK_MOD1_MASK {
        return false;
    }
    let Some(dir) = alt_arrow_direction(keyval) else {
        return false;
    };
    session_ops::focus_pane_direction(dir)
}

fn dispatch_global_key(keyval: c_uint, state: c_uint, keycode: u16) -> c_int {
    if keyval == 0xffc8 {
        topbar::toggle_window_square();
        return 1;
    }
    if handle_alt_tab_shortcut(keyval, state) || handle_alt_arrow_focus(keyval, state) {
        return 1;
    }
    if let Some(handled) = handle_ctrl_shortcut(keyval, state, keycode) {
        return handled;
    }
    c_int::from(handle_ctrl_shift_shortcut(keyval, state, keycode))
}

pub(crate) fn route_swarm_escape(keyval: c_uint) -> bool {
    let handled = app_state::with_app(|app| {
        app.swarm_view.handle_key_press(app.stack.cast(), Some(app.active_id), keyval)
    })
    .unwrap_or(false);
    if handled {
        topbar::refresh_ui();
    }
    handled
}

extern "C" fn on_window_key_press(
    _win: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    let Some((keyval, state, keycode)) = extract_key_event(event) else {
        return 0;
    };
    if route_swarm_escape(keyval) {
        return 1;
    }
    if let Some(handled) = route_palette_key(keyval, state, keycode) {
        return handled;
    }
    if let Some(handled) = route_search_key(keyval, state, keycode) {
        return handled;
    }
    dispatch_global_key(keyval, state, keycode)
}

pub(crate) fn jump_next_attention() {
    let target = app_state::with_app(|app| {
        let pairs: Vec<(&crate::session::TermSession, Option<&crate::pane_registry::PaneRegistry>)> =
            app.sessions.iter().map(|s| (s, (!s.pane_registry.is_empty()).then_some(&s.pane_registry))).collect();
        let active_vte = app.sessions.iter().find(|s| s.id == app.active_id).map_or(ptr::null_mut(), |s| s.active_vte());
        approval::find_next_pane_attention(&pairs, app.active_id, active_vte)
    })
    .flatten();
    if let Some(hit) = target {
        session_ops::select_session(hit.session_id);
        app_state::record_pane_focus(hit.session_id, hit.vte);
        if !hit.vte.is_null() {
            unsafe { gtk_widget_grab_focus(hit.vte) };
        }
        topbar::refresh_ui();
    }
}

fn dispatch_tab_split_or_zoom(keyval: c_uint, keycode: u16) -> bool {
    match (char::from_u32(keyval), keycode) {
        (Some('D' | 'd'), _) | (_, 40) => session_ops::split_active_tab(GTK_ORIENTATION_HORIZONTAL),
        (Some('E' | 'e'), _) | (_, 26) => session_ops::split_active_tab(GTK_ORIENTATION_VERTICAL),
        (Some('Z' | 'z'), _) | (_, 52) => session_ops::toggle_active_pane_zoom(),
        _ => return false,
    }
    true
}

fn dispatch_tab_action_key(keyval: c_uint, keycode: u16) -> bool {
    match (char::from_u32(keyval), keycode) {
        (Some('T' | 't'), _) | (_, 28) => session_ops::open_default_tab(),
        (Some('W' | 'w'), _) | (_, 25) => session_ops::close_active_pane_or_tab(),
        (Some('R' | 'r'), _) | (_, 27) => session_ops::reconnect_session(app_state::active_session_id()),
        _ => return dispatch_tab_split_or_zoom(keyval, keycode),
    }
    true
}

fn dispatch_workspace_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if approval::is_jump_attention_shortcut(keyval, keycode) {
        jump_next_attention();
        return true;
    }
    if crate::task_dispatch::is_dispatch_shortcut(keyval, state, keycode) {
        crate::task_dispatch::open_for_app();
        return true;
    }
    if crate::swarm_view::is_swarm_shortcut(keyval, state, keycode) {
        topbar::toggle_swarm_view();
        return true;
    }
    false
}

fn handle_tab_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if let Some(delta) = tab_order::tab_reorder_delta(keyval, state, keycode) {
        session_ops::move_active_tab(delta);
        return true;
    }
    dispatch_workspace_shortcut(keyval, state, keycode) || dispatch_tab_action_key(keyval, keycode)
}

pub(crate) fn execute_palette_action(action: palette::PaletteAction) {
    match action {
        palette::PaletteAction::SelectTab(sid) => session_ops::select_session(sid),
        palette::PaletteAction::JumpTmux { host, session } => ssh_ui::jump_to_remote_tmux(&host, &session),
        palette::PaletteAction::OpenProfile(key) => {
            session_ops::open_tab(Tab::local(&key));
        }
        palette::PaletteAction::OpenSsh { host, tmux_session } => {
            session_ops::add_remote_session(&host, tmux_session.as_deref());
        }
        palette::PaletteAction::RunAgent(code) => run_palette_agent(code),
        palette::PaletteAction::InjectHandoff(path) => {
            pane_actions::handoff_file_to_session(app_state::active_session_id(), &path);
        }
        action => execute_palette_tool_action(action),
    }
}

fn execute_palette_tool_action(action: palette::PaletteAction) {
    match action {
        palette::PaletteAction::ToggleTheme => topbar::toggle_theme(),
        palette::PaletteAction::OpenSearch => open_search_bar(),
        palette::PaletteAction::OpenGitMenu => pane_actions::populate_git_popover(true, true),
        palette::PaletteAction::GitFile { repo_root, path, untracked } => {
            pane_actions::open_git_file_action(&repo_root, &path, untracked);
        }
        palette::PaletteAction::RemoveCleanWorktree => pane_actions::start_remove_clean_worktree(),
        palette::PaletteAction::OpenTaskDispatch => crate::task_dispatch::open_for_app(),
        palette::PaletteAction::ToggleSwarmDashboard => topbar::toggle_swarm_view(),
        palette::PaletteAction::OpenSettings => topbar::open_settings_for_app(),
        palette::PaletteAction::OpenAbout => topbar::open_about_for_app(),
        palette::PaletteAction::SwitchLocale => topbar::toggle_locale_for_app(),
        _ => {}
    }
}

fn run_palette_agent(code: usize) {
    if let Some((_, command)) = text::AGENT_ACTIONS.get(code) {
        session_ops::send_to_active(command);
    }
}

pub(crate) fn pick_focused_vte(vtes: &[*mut GtkWidget], last_focus: *mut GtkWidget) -> *mut GtkWidget {
    vtes.iter()
        .copied()
        .find(|&w| split_pane::is_widget_focused(w))
        .or_else(|| vtes.contains(&last_focus).then_some(last_focus))
        .or_else(|| vtes.first().copied())
        .unwrap_or(ptr::null_mut())
}

pub(crate) fn active_search_vte() -> *mut GtkWidget {
    let Some((sid, vtes, last_focus)) = app_state::snapshot_active_panes() else {
        return ptr::null_mut();
    };
    let chosen = pick_focused_vte(&vtes, last_focus);
    app_state::record_pane_focus(sid, chosen);
    chosen
}

pub(crate) fn collect_close_search_vtes() -> (Vec<*mut GtkWidget>, *mut GtkWidget) {
    let focus_vte = active_search_vte();
    let mut all = Vec::new();
    app_state::with_app(|app| {
        for s in &app.sessions {
            s.panes.collect_vtes(s.vte, &mut all);
        }
    });
    (all, focus_vte)
}

fn clone_search_bar() -> Option<search_bar::SearchBar> {
    app_state::with_app(|app| app.search_bar.clone())
}

fn open_search_bar() {
    if let Some(bar) = clone_search_bar() {
        bar.open();
    }
}

fn toggle_search_bar() {
    context_menu::dismiss_rename_popover();
    pane_actions::dismiss_git_popover();
    let triple = app_state::with_app(|app| {
        (app.palette.clone(), app.pin_pop.clone(), app.search_bar.clone())
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

fn active_palette_git_state(app: &AppState) -> (Option<git_status::GitRepoStatus>, bool) {
    let cwd = app
        .sessions
        .iter()
        .find(|s| s.id == app.active_id)
        .and_then(session::TermSession::active_local_cwd);
    let Some((root, head)) = cwd.and_then(|d| git_status::resolve_git_repo(&d, &paths::home_dir()))
    else {
        return (None, false);
    };
    let status = app.git_scheduler.snapshot().remove(&root);
    if status.is_none() {
        app.git_scheduler.request_refresh(&root);
    }
    (status, head.is_linked_worktree)
}

fn build_palette_items(app: &AppState) -> Vec<palette::PaletteItem> {
    let tmux_items = app_state::snapshot_box_tmux_items(app);
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
    palette::collect_items(&src)
}

fn toggle_palette() {
    let snapshot = app_state::with_app_mut(|app| {
        let pal = app.palette.clone();
        let pin_pop = app.pin_pop.clone();
        let items = (!pal.is_open()).then(|| {
            crate::ssh_ui::refresh_ssh_hosts_if_changed(app);
            build_palette_items(app)
        });
        (pal, pin_pop, items)
    });
    if let Some((pal, pin_pop, maybe_items)) = snapshot {
        apply_palette_toggle(pal, pin_pop, maybe_items);
    }
}

#[cfg(test)]
pub(crate) fn toggle_palette_for_test() -> Option<Vec<String>> {
    toggle_palette();
    app_state::with_app(|app| {
        app.palette
            .is_open()
            .then(|| build_palette_items(app).into_iter().map(|i| i.title).collect())
    })
    .flatten()
}

fn apply_palette_toggle(
    pal: palette::CommandPalette,
    pin_pop: tab_order::TabPinPopover,
    maybe_items: Option<Vec<palette::PaletteItem>>,
) {
    match maybe_items {
        Some(items) => {
            context_menu::dismiss_rename_popover();
            pin_pop.detach();
            pal.open(items);
        }
        None => pal.close(),
    }
}

fn dismiss_overlays_for_git() {
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
}

fn route_overlay_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> Option<c_int> {
    if palette::is_palette_shortcut(keyval, state, keycode) {
        pane_actions::dismiss_git_popover();
        toggle_palette();
        return Some(1);
    }
    if theme_mode::is_theme_toggle_shortcut(keyval, state, keycode) {
        topbar::toggle_theme();
        return Some(1);
    }
    if search_bar::is_search_shortcut(keyval, state, keycode) {
        toggle_search_bar();
        return Some(1);
    }
    None
}

fn route_palette_key(keyval: c_uint, state: c_uint, keycode: u16) -> Option<c_int> {
    if context_menu::is_rename_open() {
        return Some(0);
    }
    if let Some(handled) = route_overlay_shortcut(keyval, state, keycode) {
        return Some(handled);
    }
    if git_status::is_git_popover_shortcut(keyval, state, keycode) {
        dismiss_overlays_for_git();
        pane_actions::toggle_git_popover();
        return Some(1);
    }
    app_state::is_overlay_open().then_some(0)
}
