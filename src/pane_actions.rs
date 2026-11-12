//! Pane and session actions: file drop, smart paste, Git/Handoff/Agent popovers, and context menus.

use std::ffi::{c_int, c_uint, c_void, CString};
use std::path::{Path, PathBuf};
use std::{ptr, rc::Rc, thread};

use crate::app_state::{self, AppState, APP, PASTE_BUSY, WT_REMOVE_BUSY};
use crate::context_menu::{self, ContextAction, DispatchFn, SessionTarget};
use crate::ffi::*;
use crate::{file_drop, git_status, git_wt, handoff, paste_route, paths};
use crate::{proc_radar, session, session_ops, split_pane, tabs::Tab, topbar, ui_text as text};

fn dismiss_agents_popover() {
    let pop = app_state::with_app(|app| app.agents_pop);
    if let Some(pop) = pop.filter(|p| !p.is_null()) {
        unsafe { gtk_popover_popdown(pop) };
    }
}

pub(crate) fn agent_cmd_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_agent_cmd
}

extern "C" fn on_agent_cmd(_btn: *mut GtkWidget, code: *mut c_void) {
    let Some((_, command)) = text::AGENT_ACTIONS.get(code as usize) else {
        return;
    };
    dismiss_agents_popover();
    session_ops::send_to_active(command);
}

fn active_worktree_context() -> Result<(String, String), String> {
    dismiss_agents_popover();
    APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref().ok_or_else(String::new)?;
        let err = || text::WORKTREE_LOCAL_ONLY.to_string_lossy().into_owned();
        let s = app.sessions.iter().find(|s| s.id == app.active_id).ok_or_else(err)?;
        let Tab::Local { ref profile, .. } = s.tab else {
            return Err(err());
        };
        let key = resolve_worktree_profile_key(app, profile);
        let cwd = s.active_local_cwd().ok_or_else(err)?;
        Ok((key, cwd))
    })
}

fn resolve_worktree_profile_key(app: &AppState, profile: &str) -> String {
    if app.config.profile(profile).monitor {
        app.config.profile("shell").key
    } else {
        profile.to_owned()
    }
}

pub(crate) fn agent_worktree_cmd_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_agent_worktree_cmd
}

extern "C" fn on_agent_worktree_cmd(_btn: *mut GtkWidget, code: *mut c_void) {
    let Some((_, command)) = text::AGENT_ACTIONS.get(code as usize) else {
        return;
    };
    let (profile, cwd) = match active_worktree_context() {
        Ok(ctx) => ctx,
        Err(msg) => return app_state::set_footer_message(&msg),
    };
    let slug = git_wt::agent_slug_from_cmd(command);
    match git_wt::create_agent_worktree(Path::new(&cwd), &slug) {
        Ok(wt_path) => launch_worktree_session(&wt_path, profile, command, &slug),
        Err(err) => app_state::set_footer_message(&format!("⚠ {err}")),
    }
}

fn launch_worktree_session(wt_path: &Path, profile: String, command: &str, slug: &str) {
    let wt_str = wt_path.to_string_lossy().into_owned();
    let sid = session_ops::open_local_tab(profile, &wt_str);
    session_ops::send_to_session(sid, command);
    app_state::set_footer_message(&format!(
        "⎇ Worktree {} · запущен {slug}",
        paths::shorten_home(&wt_str)
    ));
}

pub(crate) fn find_live_drop_vte(
    sessions: &[session::TermSession],
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

fn snapshot_drop_target(s: &session::TermSession, vte: *mut GtkWidget) -> PendingDropTarget {
    if let Some(remote) = s.remote() {
        return PendingDropTarget::Remote { sid: s.id, host: remote.host.clone() };
    }
    let is_remote_vte =
        vte == s.active_vte() && s.effective_category() == session::SessionCategory::Remote;
    let cached_host = is_remote_vte
        .then(|| s.proc_info.ssh_host.clone())
        .filter(|h| !h.is_empty());
    PendingDropTarget::CheckLive {
        sid: s.id,
        pid: Some(s.pid_for_vte(vte)).filter(|&p| p > 0),
        cached_host,
    }
}

pub(crate) fn lookup_vte_drop_target(vte: *mut GtkWidget) -> file_drop::DropTarget {
    let pending = APP.with(|cell| {
        let Ok(state) = cell.try_borrow() else {
            return PendingDropTarget::Ignore;
        };
        let Some(app) = state.as_ref() else {
            return PendingDropTarget::CheckLive { sid: 0, pid: None, cached_host: None };
        };
        app.sessions
            .iter()
            .find(|s| s.has_vte(vte))
            .map_or(PendingDropTarget::Ignore, |s| snapshot_drop_target(s, vte))
    });
    resolve_pending_drop(pending)
}

fn resolve_pending_drop(pending: PendingDropTarget) -> file_drop::DropTarget {
    match pending {
        PendingDropTarget::Ignore => file_drop::DropTarget::Ignore,
        PendingDropTarget::Remote { sid, host } => file_drop::DropTarget::Remote { sid, host },
        PendingDropTarget::CheckLive { sid, pid, cached_host } => {
            let live =
                pid.map(proc_radar::inspect_proc_tree).map(|p| p.ssh_host).filter(|h| !h.is_empty());
            match live.or(cached_host) {
                Some(host) if sid > 0 => file_drop::DropTarget::Remote { sid, host },
                _ => file_drop::DropTarget::Local,
            }
        }
    }
}

pub(crate) fn apply_vte_drop_upload(done: file_drop::UploadCompletion) {
    let target_vte = done.vte_addr as *mut GtkWidget;
    let resolved = app_state::with_app(|app| {
        let live = find_live_drop_vte(&app.sessions, app.active_id, done.sid, target_vte);
        (app.footer_status, live)
    });
    let Some((footer, live)) = resolved else {
        return;
    };
    if !footer.is_null() && !done.footer_msg.is_empty() {
        set_label_text(footer, &done.footer_msg);
    }
    if let (Some((vte, focus)), Some(text)) = (live, done.injected_text.as_deref()) {
        file_drop::feed_vte_text(vte, text, focus);
    }
}

pub(crate) fn install_file_drop_hooks() {
    file_drop::register_file_drop_hooks(file_drop::FileDropHooks {
        resolve_target: lookup_vte_drop_target,
        set_status: app_state::set_footer_message,
        complete_upload: apply_vte_drop_upload,
    });
}

fn active_capture_context(
    app: &AppState,
) -> Option<(*mut GtkWidget, String, String, String, Option<PathBuf>)> {
    let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
    let vte = s.active_vte();
    let fallback = if !s.proc_info.cwd.is_empty() {
        paths::expand_home(&s.proc_info.cwd)
    } else {
        s.cwd.clone()
    };
    let cwd = s.active_local_cwd().unwrap_or(fallback);
    let host = app_state::session_ssh_host(s);
    Some((vte, s.title.clone(), cwd, host, app.config.briefs_dir.clone()))
}

fn capture_active_to_file() -> Option<PathBuf> {
    let (vte, title, cwd, ssh_host, briefs_dir) =
        app_state::with_app(active_capture_context).flatten()?;
    if vte.is_null() {
        return None;
    }
    let meta = handoff::CaptureMeta {
        title: &title,
        cwd: &cwd,
        ssh_host: &ssh_host,
        timestamp_secs: handoff::current_unix_secs(),
    };
    handoff::save_vte_capture(vte, &meta, briefs_dir.as_deref())
        .map_err(|err| app_state::set_footer_message(&format!("⚠ Ошибка сохранения снимка: {err}")))
        .ok()
}

pub(crate) fn handoff_file_to_session(target_sid: usize, local_path: &Path) {
    let target_host = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.id == target_sid).map(app_state::session_ssh_host)
    })
    .flatten();
    let Some(host) = target_host else {
        return;
    };
    match handoff::resolve_handoff_delivery(local_path, &host) {
        Ok((prompt, status)) => {
            session_ops::select_session(target_sid);
            session_ops::send_to_session(target_sid, &prompt);
            app_state::set_footer_message(&status);
        }
        Err(err) => app_state::set_footer_message(&err),
    }
}

fn collect_handoff_targets(app: &AppState) -> Vec<handoff::HandoffTarget> {
    let has_regular = app.sessions.iter().any(|s| s.category != session::SessionCategory::Daemon);
    app.sessions
        .iter()
        .filter(|s| !has_regular || s.category != session::SessionCategory::Daemon)
        .map(|s| handoff::HandoffTarget {
            sid: s.id,
            title: s.title.clone(),
            ssh_host: app_state::session_ssh_host(s),
            is_active: s.id == app.active_id,
        })
        .collect()
}

pub(crate) fn handoff_open_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_handoff_open
}

fn apply_handoff_rebuild(
    widgets: handoff::HandoffWidgets,
    briefs_dir: Option<PathBuf>,
    targets: Vec<handoff::HandoffTarget>,
) {
    let files = handoff::list_handoff_files(briefs_dir.as_deref());
    let target_ids: Vec<usize> = targets.iter().map(|t| t.sid).collect();
    let cbs = handoff::HandoffCallbacks {
        on_capture: on_handoff_capture,
        on_forward: on_handoff_forward,
        on_inject: on_handoff_inject,
    };
    let new_inner = handoff::rebuild_handoff_widgets(widgets, &files, &targets, cbs);
    app_state::with_app_mut(|app| app.handoff.save_snapshot(new_inner, files, target_ids));
    if !widgets.popover.is_null() {
        unsafe { gtk_popover_popup(widgets.popover) };
    }
}

extern "C" fn on_handoff_open(_btn: *mut GtkWidget, _data: *mut c_void) {
    let prep = app_state::with_app_mut(|app| {
        (app.handoff.take_widgets(), app.config.briefs_dir.clone(), collect_handoff_targets(app))
    });
    if let Some((widgets, briefs_dir, targets)) = prep {
        apply_handoff_rebuild(widgets, briefs_dir, targets);
    }
}

extern "C" fn on_handoff_capture(_btn: *mut GtkWidget, _data: *mut c_void) {
    if let Some(pop) = app_state::with_app(|app| app.handoff.popover()).filter(|p| !p.is_null()) {
        unsafe { gtk_popover_popdown(pop) };
    }
    if let Some(path) = capture_active_to_file() {
        let shown = paths::shorten_home(&path.display().to_string());
        app_state::set_footer_message(&format!("📋 Вывод сохранён в {shown}"));
    }
}

extern "C" fn on_handoff_forward(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let action = app_state::with_app(|app| {
        (app.handoff.popover(), app.handoff.target_at(idx_ptr as usize))
    });
    let Some((pop, target_sid)) = action else {
        return;
    };
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
    if let (Some(sid), Some(saved_path)) = (target_sid, capture_active_to_file()) {
        handoff_file_to_session(sid, &saved_path);
    }
}

extern "C" fn on_handoff_inject(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let action = app_state::with_app(|app| {
        (app.handoff.popover(), app.active_id, app.handoff.file_at(idx_ptr as usize))
    });
    let Some((pop, sid, chosen)) = action else {
        return;
    };
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
    if let Some(path) = chosen {
        handoff_file_to_session(sid, &path);
    }
}

pub(crate) fn dismiss_git_popover() {
    let pop = app_state::with_app(|app| app.git_menu.popover());
    if let Some(pop) = pop.filter(|p| !p.is_null()) {
        unsafe { gtk_popover_popdown(pop) };
    }
}

type GitPopoverPrep = (
    git_status::GitMenuWidgets,
    Option<(String, git_wt::GitHeadInfo)>,
    Option<git_status::GitRepoStatus>,
);

fn prepare_git_popover_state(force_refresh: bool) -> Result<GitPopoverPrep, String> {
    APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut().ok_or_else(String::new)?;
        let local_err = || text::GIT_LOCAL_ONLY.to_string_lossy().into_owned();
        let active = app.sessions.iter().find(|s| s.id == app.active_id).ok_or_else(local_err)?;
        let cwd = active.active_local_cwd().ok_or_else(local_err)?;
        let repo = git_status::resolve_git_repo(&cwd, &paths::home_dir());
        let status = repo.as_ref().and_then(|(root, _)| read_or_queue_git(app, root, force_refresh));
        Ok((app.git_menu.take_widgets(), repo, status))
    })
}

fn read_or_queue_git(
    app: &AppState,
    root: &str,
    force_refresh: bool,
) -> Option<git_status::GitRepoStatus> {
    let cached = app.git_scheduler.snapshot().remove(root);
    if force_refresh || cached.is_none() {
        app.git_scheduler.request_refresh(root);
    }
    cached
}

fn finish_git_popover_popup(widgets: git_status::GitMenuWidgets, pin_pop: Option<crate::tab_order::TabPinPopover>) {
    if let Some(pin_pop) = pin_pop {
        pin_pop.detach();
    }
    context_menu::dismiss_rename_popover();
    if !widgets.popover.is_null() {
        unsafe { gtk_popover_popup(widgets.popover) };
    }
}

pub(crate) fn populate_git_popover(force_refresh: bool, popup_after: bool) {
    let (widgets, repo, status) = match prepare_git_popover_state(force_refresh) {
        Ok(prep) => prep,
        Err(msg) => {
            app_state::set_footer_message(&msg);
            app_state::focus_active_vte();
            return;
        }
    };
    let repo_ref = repo.as_ref().map(|(r, h)| (r.as_str(), h));
    let cbs = git_status::GitMenuCallbacks { on_file: on_git_file_clicked, on_remove_wt: on_git_remove_worktree };
    let (inner, entries) = git_status::rebuild_git_widgets(widgets, repo_ref, status.as_ref(), cbs);
    let pin_pop = app_state::with_app_mut(|app| {
        app.git_menu.save_snapshot(inner, entries);
        popup_after.then(|| app.pin_pop.clone())
    })
    .flatten();
    if popup_after {
        finish_git_popover_popup(widgets, pin_pop);
    }
}

fn is_git_popover_open() -> Option<*mut GtkWidget> {
    let pop = app_state::with_app(|app| app.git_menu.popover())?;
    if pop.is_null() || unsafe { gtk_widget_get_visible(pop) == 0 } {
        return None;
    }
    Some(pop)
}

fn refresh_open_git_popover() {
    if is_git_popover_open().is_some() {
        populate_git_popover(false, false);
    }
}

pub(crate) fn toggle_git_popover() {
    if let Some(pop) = is_git_popover_open() {
        unsafe { gtk_popover_popdown(pop) };
        app_state::focus_active_vte();
    } else {
        populate_git_popover(true, true);
    }
}

pub(crate) fn git_open_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_git_open
}

extern "C" fn on_git_open(_btn: *mut GtkWidget, _data: *mut c_void) {
    populate_git_popover(true, true);
}

pub(crate) fn git_status_ready_cb() -> extern "C" fn(*mut c_void) -> c_int {
    on_git_status_ready
}

extern "C" fn on_git_status_ready(_data: *mut c_void) -> c_int {
    topbar::refresh_ui();
    refresh_open_git_popover();
    0
}

pub(crate) fn git_key_cb() -> extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int {
    on_git_popover_key_press
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
    if keyval == 0xff1b || git_status::is_git_popover_shortcut(keyval, state, keycode) {
        dismiss_git_popover();
        app_state::focus_active_vte();
        return 1;
    }
    0
}

fn prepare_diff_split(
    sid: usize,
    new_scroll: *mut GtkWidget,
    new_vte: *mut GtkWidget,
    new_pid: i32,
    repo_root: &str,
) -> Option<split_pane::SplitOp> {
    app_state::with_app_mut(|app| {
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        Some(s.panes.prepare_split(
            s.scroll_wrap, s.vte, new_scroll, new_vte, new_pid, repo_root.to_owned(), GTK_ORIENTATION_HORIZONTAL,
        ))
    })
    .flatten()
}

fn open_git_diff_split(repo_root: &str, rel_path: &str) {
    let local_ok = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.id == app.active_id)?.active_local_cwd()
    })
    .flatten();
    let Some((sid, _, _, shell, scale)) =
        local_ok.and_then(|_| session_ops::local_split_context(None, ptr::null_mut()))
    else {
        return app_state::set_footer_message(&text::GIT_LOCAL_ONLY.to_string_lossy());
    };
    let argv = git_status::build_diff_pane_argv(repo_root, rel_path, &shell);
    let (new_vte, new_pid) = session_ops::create_terminal(repo_root, &argv, sid, scale);
    let new_scroll = split_pane::wrap_vte_scroll(new_vte);
    let split_op = prepare_diff_split(sid, new_scroll, new_vte, new_pid, repo_root);
    split_pane::finish_split(split_op, new_scroll, new_vte);
    topbar::refresh_ui();
    app_state::set_footer_message(&format!("⎇ Diff: {rel_path}"));
}

pub(crate) fn open_git_file_action(repo_root: &str, rel_path: &str, untracked: bool) {
    if untracked {
        let full = Path::new(repo_root).join(rel_path);
        proc_radar::open_external_link(&full.to_string_lossy());
        app_state::set_footer_message(&format!("📄 Открыт файл {rel_path}"));
        app_state::focus_active_vte();
    } else {
        open_git_diff_split(repo_root, rel_path);
    }
}

extern "C" fn on_git_file_clicked(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let chosen = app_state::with_app(|app| {
        (app.git_menu.popover(), app.git_menu.entry_at(idx_ptr as usize))
    });
    let Some((pop, entry)) = chosen else {
        return;
    };
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
    outcome: Result<git_status::WorktreeRemovalOutcome, String>,
}

fn request_git_refresh_for_cwd(cwd: &str) {
    let Some((root, _)) = git_status::resolve_git_repo(cwd, &paths::home_dir()) else {
        return;
    };
    app_state::with_app(|app| app.git_scheduler.request_refresh(&root));
}

fn find_main_repo_tab(
    sessions: &[session::TermSession],
    skip_sid: usize,
    main_root: &str,
    home: &str,
) -> Option<usize> {
    sessions.iter().find_map(|s| {
        if s.id == skip_sid || s.remote().is_some() {
            return None;
        }
        let cwd = s.active_local_cwd()?;
        let (root, _) = git_status::resolve_git_repo(&cwd, home)?;
        (root == main_root).then_some(s.id)
    })
}

fn finish_worktree_removal(
    sid: usize,
    profile: &str,
    outcome: git_status::WorktreeRemovalOutcome,
) {
    let wt_str = outcome.wt_root.to_string_lossy().into_owned();
    let main_str = outcome.main_repo.to_string_lossy().into_owned();
    let nav = app_state::with_app(|app| {
        app.git_scheduler.remove_root(&wt_str);
        app.git_scheduler.request_refresh(&main_str);
        let has_tab = app.sessions.iter().any(|s| s.id == sid);
        let sole_tab = has_tab && app.sessions.len() == 1;
        let main_sid = find_main_repo_tab(&app.sessions, sid, &main_str, &paths::home_dir());
        (has_tab, sole_tab, main_sid)
    });
    if let Some((has_tab, sole_tab, main_sid)) = nav {
        apply_post_worktree_nav(sid, profile, &main_str, has_tab, sole_tab, main_sid);
    }
    app_state::set_footer_message(&outcome.success_message());
}

fn apply_post_worktree_nav(
    sid: usize,
    profile: &str,
    main_str: &str,
    has_tab: bool,
    sole_tab: bool,
    main_sid: Option<usize>,
) {
    if sole_tab {
        session_ops::open_local_tab(profile.to_owned(), main_str);
        session_ops::close_session(sid);
    } else if has_tab {
        session_ops::close_session(sid);
        if let Some(target_sid) = main_sid {
            session_ops::select_session(target_sid);
        }
    }
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
            app_state::set_footer_message(&err);
            app_state::focus_active_vte();
        }
    }
    0
}

fn active_wt_removal_target() -> Option<(usize, String, String)> {
    app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        let Tab::Local { ref profile, .. } = s.tab else {
            return None;
        };
        let key = resolve_worktree_profile_key(app, profile);
        Some((s.id, key, s.active_local_cwd()?))
    })
    .flatten()
}

pub(crate) fn start_remove_clean_worktree() {
    dismiss_git_popover();
    if WT_REMOVE_BUSY.with(|b| b.replace(true)) {
        return;
    }
    let Some((sid, profile, cwd)) = active_wt_removal_target() else {
        WT_REMOVE_BUSY.with(|b| b.set(false));
        app_state::set_footer_message(&text::GIT_LOCAL_ONLY.to_string_lossy());
        app_state::focus_active_vte();
        return;
    };
    spawn_worktree_removal(sid, profile, cwd);
}

fn spawn_worktree_removal(sid: usize, profile: String, cwd: String) {
    app_state::focus_active_vte();
    thread::spawn(move || {
        let outcome = git_status::remove_clean_worktree(Path::new(&cwd));
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

fn resolve_session_agent(s: &mut session::TermSession, target_vte: *mut GtkWidget) -> String {
    if s.effective_category() == session::SessionCategory::Remote {
        return s.proc_info.agent.clone();
    }
    let prev = s.proc_info.agent.clone();
    let live = proc_radar::detect_local_agent(s.pid_for_vte(target_vte));
    session_ops::sync_session_proc(s, None);
    let fallback = if s.proc_info.agent.is_empty() { &prev } else { &s.proc_info.agent };
    paste_route::resolve_pane_agent(live.as_deref(), target_vte == s.vte, fallback)
}

fn pane_paste_context(
    sid_opt: Option<usize>,
    preferred_vte: *mut GtkWidget,
) -> Option<(usize, *mut GtkWidget, String, String)> {
    app_state::with_app_mut(|app| {
        let target_id = sid_opt.unwrap_or(app.active_id);
        let s = app.sessions.iter_mut().find(|s| s.id == target_id)?;
        let target_vte = app_state::pick_session_vte(s, preferred_vte);
        s.panes.record_focus(target_vte);
        let agent = resolve_session_agent(s, target_vte);
        Some((s.id, target_vte, app_state::session_ssh_host(s), agent))
    })
    .flatten()
}

fn session_paste_widgets(
    sid: usize,
    preferred_vte: *mut GtkWidget,
) -> Option<(*mut GtkWidget, *mut GtkWidget)> {
    app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        let vte = app_state::pick_session_vte(s, preferred_vte);
        (!vte.is_null()).then_some((vte, app.footer_status))
    })
    .flatten()
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

fn run_paste_route(
    sid_opt: Option<usize>,
    preferred_vte: *mut GtkWidget,
    plain_ctrl_v: bool,
    force_image: bool,
) -> bool {
    let Some((sid, target_vte, host, agent)) = pane_paste_context(sid_opt, preferred_vte) else {
        return false;
    };
    let has_image = plain_ctrl_v && proc_radar::clipboard_has_direct_image();
    if paste_route::choose_paste_route(plain_ctrl_v, has_image, &host, &agent)
        == paste_route::PasteRoute::PassThroughToPty
    {
        return false;
    }
    deliver_paste(sid, target_vte, &host, force_image)
}

fn deliver_paste(
    sid: usize,
    target_vte: *mut GtkWidget,
    host: &str,
    force_image: bool,
) -> bool {
    let image = proc_radar::try_paste_clipboard_image(host, force_image);
    if image.is_none() && force_image {
        return false;
    }
    let Some((vte, footer)) = session_paste_widgets(sid, target_vte) else {
        return false;
    };
    finish_paste(vte, footer, image);
    true
}

fn paste_routed_to(
    sid_opt: Option<usize>,
    preferred_vte: *mut GtkWidget,
    plain_ctrl_v: bool,
    force_image: bool,
) -> bool {
    if PASTE_BUSY.with(|busy| busy.replace(true)) {
        return !force_image;
    }
    let _guard = PasteGuard;
    run_paste_route(sid_opt, preferred_vte, plain_ctrl_v, force_image)
}

pub(crate) fn paste_smart(force_image: bool) -> bool {
    paste_routed_to(None, ptr::null_mut(), false, force_image)
}

pub(crate) fn paste_plain_ctrl_v() -> bool {
    paste_routed_to(Some(app_state::active_session_id()), ptr::null_mut(), true, false)
}

pub(crate) fn paste_image_clicked_cb() -> extern "C" fn(*mut GtkWidget, *mut c_void) {
    on_paste_image_clicked
}

extern "C" fn on_paste_image_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let _ = paste_smart(true);
}

fn collect_forward_targets(app: &AppState, source_sid: usize) -> Vec<SessionTarget> {
    let has_regular = app.sessions.iter().any(|s| s.category != session::SessionCategory::Daemon);
    app.sessions
        .iter()
        .filter(|s| !has_regular || s.category != session::SessionCategory::Daemon)
        .map(|s| context_menu::format_session_target(s.id, &s.title, &app_state::session_ssh_host(s), source_sid))
        .collect()
}

fn forward_pane_to_tab(
    source_sid: usize,
    source_vte: *mut GtkWidget,
    target_sid: usize,
    selected_text: Option<String>,
) {
    let ctx = app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == source_sid)?;
        let pane_vte = app_state::pick_session_vte(s, source_vte);
        let cwd = app_state::pane_cwd(s, pane_vte);
        let host = app_state::session_ssh_host(s);
        Some((pane_vte, s.title.clone(), cwd, host, app.config.briefs_dir.clone()))
    })
    .flatten();
    let Some((pane_vte, title, cwd, ssh_host, briefs_dir)) = ctx else {
        return;
    };
    match context_menu::save_forward_capture(
        briefs_dir.as_deref(), pane_vte, &title, &cwd, &ssh_host, selected_text,
    ) {
        Ok(path) => handoff_file_to_session(target_sid, &path),
        Err(msg) => app_state::set_footer_message(&msg),
    }
}

pub(crate) fn close_pane_by_vte(target_vte: *mut GtkWidget) {
    let sid = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.has_vte(target_vte)).map(|s| s.id)
    })
    .flatten();
    if let Some(sid) = sid {
        close_pane_in_session(sid, target_vte);
    }
}

fn close_pane_in_session(sid: usize, target_vte: *mut GtkWidget) {
    let outcome = app_state::with_app_mut(|app| {
        let active_id = app.active_id;
        let s = app.sessions.iter_mut().find(|s| s.id == sid && s.has_vte(target_vte))?;
        if !s.panes.has_splits() {
            return Some((false, None));
        }
        let op = s.panes.prepare_close_vte(target_vte, &mut s.scroll_wrap, &mut s.vte, &mut s.child_pid, &mut s.cwd)?;
        session_ops::sync_session_proc(s, None);
        Some((s.id == active_id, Some(op)))
    })
    .flatten();
    if let Some((is_active, maybe_op)) = outcome {
        apply_close_pane_outcome(sid, is_active, maybe_op);
    }
}

fn apply_close_pane_outcome(sid: usize, is_active: bool, maybe_op: Option<split_pane::CloseOp>) {
    match maybe_op {
        Some(op) => {
            op.apply_focus(is_active);
            topbar::refresh_ui();
            session_ops::save_tabs();
        }
        None => session_ops::close_session(sid),
    }
}

fn duplicate_session_tab(sid: usize) {
    let dup = app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        Some(match &s.tab {
            Tab::Local { profile, .. } => {
                let cwd = s.active_local_cwd().unwrap_or_else(|| s.cwd.clone());
                Tab::Local { profile: profile.clone(), cwd }
            }
            Tab::Remote(remote) => Tab::Remote(remote.clone()),
        })
    })
    .flatten();
    if let Some(tab) = dup {
        session_ops::open_tab(tab);
    }
}

pub(crate) fn commit_tab_rename(sid: usize, new_title: String) {
    session_ops::apply_pinned_tab_title(sid, session::sanitize_pinned_title(Some(&new_title)));
}

fn prepare_rename_target(
    sid: usize,
    anchor: *mut GtkWidget,
) -> Option<(Option<crate::palette::CommandPalette>, crate::tab_order::TabPinPopover, *mut GtkWidget, String)> {
    app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        let row = s.sidebar_row.row_widget();
        let valid_anchor = if anchor == row { row } else { s.top_tab.select_button() };
        let initial = s.pinned_title.as_deref().unwrap_or(&s.title).to_owned();
        let pal = app.palette.is_open().then(|| app.palette.clone());
        Some((pal, app.pin_pop.clone(), valid_anchor, initial))
    })
    .flatten()
}

fn open_rename_for_tab(sid: usize, anchor: *mut GtkWidget) {
    dismiss_git_popover();
    let Some((pal, pin_pop, valid_anchor, current_title)) = prepare_rename_target(sid, anchor)
    else {
        return;
    };
    if let Some(pal) = pal {
        pal.close();
    }
    pin_pop.detach();
    context_menu::popup_rename_popover(
        valid_anchor, sid, &current_title, commit_tab_rename, app_state::focus_active_vte,
    );
}

fn clear_vte_buffer(vte: *mut GtkWidget) {
    if !app_state::has_live_vte(vte) {
        return;
    }
    unsafe {
        vte_terminal_reset(vte, 0, 1);
        gtk_widget_grab_focus(vte);
    }
}

fn copy_vte_selection(vte: *mut GtkWidget) {
    if !app_state::has_live_vte(vte) {
        return;
    }
    let _ = context_menu::copy_vte_selection_to_clipboard(vte, false);
    unsafe { gtk_widget_grab_focus(vte) };
}

fn execute_vte_action(action: ContextAction) -> Result<(), ContextAction> {
    match action {
        ContextAction::CopySelection(vte) => copy_vte_selection(vte),
        ContextAction::PasteSmart { sid, vte } => {
            let _ = paste_routed_to(Some(sid), vte, false, false);
        }
        ContextAction::OpenLink(url) => proc_radar::open_external_link(&url),
        ContextAction::CopyText(value) => context_menu::copy_text_to_clipboard(&value),
        ContextAction::ForwardToTab { source_sid, source_vte, target_sid, selected_text } => {
            forward_pane_to_tab(source_sid, source_vte, target_sid, selected_text);
        }
        other => return execute_vte_split_action(other),
    }
    Ok(())
}

fn execute_vte_split_action(action: ContextAction) -> Result<(), ContextAction> {
    match action {
        ContextAction::SplitRight { sid, vte } => {
            session_ops::split_pane_in_session(Some(sid), vte, GTK_ORIENTATION_HORIZONTAL);
        }
        ContextAction::SplitDown { sid, vte } => {
            session_ops::split_pane_in_session(Some(sid), vte, GTK_ORIENTATION_VERTICAL);
        }
        ContextAction::ClosePane { sid, vte } => close_pane_in_session(sid, vte),
        ContextAction::ClearBuffer(vte) => clear_vte_buffer(vte),
        other => return Err(other),
    }
    Ok(())
}

fn execute_context_action(action: ContextAction) {
    let Err(tab_action) = execute_vte_action(action) else {
        return;
    };
    match tab_action {
        ContextAction::RenameTab { sid, anchor } => open_rename_for_tab(sid, anchor),
        ContextAction::DuplicateTab(sid) => duplicate_session_tab(sid),
        ContextAction::ReconnectTab(sid) => session_ops::reconnect_session(sid),
        ContextAction::CloseTab(sid) => session_ops::close_session(sid),
        ContextAction::CloseOtherTabs(sid) => session_ops::close_other_sessions(sid),
        _ => {}
    }
}

fn make_context_dispatcher() -> DispatchFn {
    Rc::new(execute_context_action)
}

fn build_vte_menu_spec(
    vte: *mut GtkWidget,
    link_target: Option<String>,
) -> Option<context_menu::VteMenuSpec> {
    let has_selection = !vte.is_null() && unsafe { vte_terminal_get_has_selection(vte) != 0 };
    let selected_text = has_selection.then(|| context_menu::read_vte_selection(vte)).flatten();
    app_state::with_app_mut(|app| {
        let idx = app.sessions.iter().position(|s| s.has_vte(vte))?;
        app.sessions[idx].panes.record_focus(vte);
        let sid = app.sessions[idx].id;
        let is_local = true;
        let targets = collect_forward_targets(app, sid);
        Some(context_menu::VteMenuSpec {
            sid, vte, has_selection, selected_text, link_target, is_local, targets,
        })
    })
    .flatten()
}

pub(crate) fn open_vte_context_menu(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    link_target: Option<String>,
) {
    if let Some(spec) = build_vte_menu_spec(vte, link_target) {
        context_menu::popup_vte_menu(&spec, event, make_context_dispatcher());
    }
}

pub(crate) fn tab_context_menu_cb() -> extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int {
    on_tab_context_menu
}

fn build_tab_menu_spec(widget: *mut GtkWidget, sid: usize) -> Option<context_menu::TabMenuSpec> {
    app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == sid)?;
        Some(context_menu::TabMenuSpec {
            sid,
            anchor: widget,
            local_cwd: s.active_local_cwd(),
            is_remote: s.remote().is_some(),
            can_close_others: app.sessions.len() > 1,
        })
    })
    .flatten()
}

extern "C" fn on_tab_context_menu(
    widget: *mut GtkWidget,
    event: *const GdkEvent,
    data: *mut c_void,
) -> c_int {
    if !context_menu::is_right_button_press(event) {
        return 0;
    }
    let Some(spec) = build_tab_menu_spec(widget, data as usize) else {
        return 0;
    };
    context_menu::popup_tab_menu(&spec, event, make_context_dispatcher());
    1
}

pub(crate) fn copy_active_selection(clear_after: bool) -> bool {
    let vte = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.id == app.active_id).map(session::TermSession::active_vte)
    })
    .flatten();
    vte.is_some_and(|w| context_menu::copy_vte_selection_to_clipboard(w, clear_after))
}
