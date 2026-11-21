//! Compact Git popover Worktrees section, New Worktree dialog, and prune actions.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::fs;
use std::path::{Path, PathBuf};
use std::{ptr, thread};

use crate::app_state;
use crate::connect_click;
use crate::ffi::*;
use crate::file_drop::feed_vte_text;
use crate::git_status::resolve_git_repo;
use crate::paths::{home_dir, shorten_home};
use crate::session_ops;
use crate::ui_text as text;
use crate::worktree_mgr::{
    create_worktree, list_worktrees, prune_merged_worktrees, remove_worktree, validate_base_ref,
    validate_branch_slug, PruneReport, WorktreeEntry,
};

const GTK_WIN_POS_CENTER_ON_PARENT: c_int = 4;
const AGENT_DELAY_MS: c_uint = 250;

pub const WORKTREE_AGENTS: [(&str, Option<&str>); 4] = [
    ("Shell", None),
    ("Claude", Some("claude\n")),
    ("Codex", Some("codex\n")),
    ("AGY", Some("agy\n")),
];

extern "C" {
    fn gtk_window_set_transient_for(window: *mut GtkWidget, parent: *mut GtkWidget);
    fn gtk_window_set_modal(window: *mut GtkWidget, modal: c_int);
    fn gtk_window_set_position(window: *mut GtkWidget, position: c_int);
    fn gtk_combo_box_text_new() -> *mut GtkWidget;
    fn gtk_combo_box_text_append_text(combo: *mut GtkWidget, text: *const c_char);
    fn gtk_combo_box_get_active(combo: *mut GtkWidget) -> c_int;
    fn gtk_combo_box_set_active(combo: *mut GtkWidget, index: c_int);
    fn gtk_widget_set_margin_start(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_margin_end(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_margin_top(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_margin_bottom(widget: *mut GtkWidget, margin: c_int);
}

struct PopoverWtState {
    popover: *mut GtkWidget,
    repo_root: String,
    entries: Vec<WorktreeEntry>,
    prune_busy: bool,
}

impl PopoverWtState {
    const fn empty() -> Self {
        Self { popover: ptr::null_mut(), repo_root: String::new(), entries: Vec::new(), prune_busy: false }
    }
}

struct NewWtDialog {
    window: *mut GtkWidget,
    branch_entry: *mut GtkWidget,
    base_entry: *mut GtkWidget,
    agent_combo: *mut GtkWidget,
    err_lbl: *mut GtkWidget,
    create_btn: *mut GtkWidget,
    repo_root: String,
}

thread_local! {
    static WT_POPOVER: RefCell<PopoverWtState> = const { RefCell::new(PopoverWtState::empty()) };
    static WT_DIALOG: RefCell<Option<NewWtDialog>> = const { RefCell::new(None) };
}

pub fn format_worktree_row_label(entry: &WorktreeEntry) -> String {
    let mut tags = Vec::new();
    if entry.is_primary {
        tags.push(text::WT_PRIMARY_TAG.as_str());
    }
    if entry.is_dirty {
        tags.push(text::WT_DIRTY_TAG.as_str());
    }
    if entry.is_merged {
        tags.push(text::WT_MERGED_TAG.as_str());
    }
    let name = entry.display_name();
    if tags.is_empty() {
        name
    } else {
        format!("{name}  [{}]", tags.join(", "))
    }
}

pub fn agent_command_for_index(idx: i32) -> Option<&'static str> {
    usize::try_from(idx)
        .ok()
        .and_then(|i| WORKTREE_AGENTS.get(i))
        .and_then(|(_, cmd)| *cmd)
}

fn popdown_wt_popover() {
    let pop = WT_POPOVER.with(|cell| cell.borrow().popover);
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn find_tab_for_worktree(wt_path: &Path) -> Option<usize> {
    let home = home_dir();
    app_state::with_app(|app| {
        app.sessions.iter().find_map(|s| {
            if s.remote().is_some() {
                return None;
            }
            let cwd = s.active_local_cwd()?;
            let cwd_path = Path::new(&cwd);
            if same_dir(cwd_path, wt_path) {
                return Some(s.id);
            }
            let (root, _) = resolve_git_repo(&cwd, &home)?;
            same_dir(Path::new(&root), wt_path).then_some(s.id)
        })
    })
    .flatten()
}

pub fn open_or_focus_worktree_tab(wt_path: &Path) {
    let path_str = wt_path.to_string_lossy().into_owned();
    let short = shorten_home(&path_str);
    if let Some(sid) = find_tab_for_worktree(wt_path) {
        session_ops::select_session(sid);
        app_state::set_footer_message(&format!("{} {short}", text::WT_MSG_FOCUSED.as_str()));
        return;
    }
    let _ = session_ops::open_local_tab_in_folder(&path_str);
    app_state::set_footer_message(&format!("{} {short}", text::WT_MSG_OPENED.as_str()));
}

fn make_section_header() -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(text::WT_SECTION_HDR.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-header");
        lbl
    }
}

fn make_wt_row(entry: &WorktreeEntry, idx: usize) -> *mut GtkWidget {
    let title = format_worktree_row_label(entry);
    let tip = shorten_home(&entry.path.to_string_lossy());
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        gtk_label_set_ellipsize(lbl, PANGO_ELLIPSIZE_MIDDLE);
        gtk_label_set_max_width_chars(lbl, 38);
        set_label_text(lbl, &title);
        set_tooltip(lbl, &tip);
        let open_btn = gtk_button_new_with_label(text::WT_OPEN_BTN.as_ptr());
        set_tooltip(open_btn, &tip);
        connect_click(open_btn, on_open_wt_clicked, idx as *mut c_void);
        gtk_box_pack_start(row, lbl, 1, 1, 0);
        if !entry.is_primary {
            let rm_btn = gtk_button_new_with_label(c"×".as_ptr());
            gtk_widget_set_tooltip_text(rm_btn, text::WT_REMOVE_TIP.as_ptr());
            connect_click(rm_btn, on_remove_row_clicked, idx as *mut c_void);
            gtk_box_pack_end(row, rm_btn, 0, 0, 0);
        }
        gtk_box_pack_end(row, open_btn, 0, 0, 0);
        row
    }
}

fn wrap_wt_scroll(list: *mut GtkWidget, count: usize) -> *mut GtkWidget {
    if count <= 5 {
        return list;
    }
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, 380);
        gtk_scrolled_window_set_min_content_height(scroll, 140);
        gtk_scrolled_window_set_max_content_height(scroll, 180);
        gtk_container_add(scroll, list);
        scroll
    }
}

pub fn pack_worktree_section(vbox: *mut GtkWidget, popover: *mut GtkWidget, repo_root: &str) {
    let entries = list_worktrees(Path::new(repo_root)).unwrap_or_default();
    WT_POPOVER.with(|cell| {
        let mut st = cell.borrow_mut();
        st.popover = popover;
        st.repo_root = repo_root.to_owned();
        st.entries = entries.clone();
    });
    unsafe {
        gtk_box_pack_start(vbox, make_section_header(), 0, 0, 2);
        let list = gtk_box_new(GTK_ORIENTATION_VERTICAL, 3);
        for (idx, entry) in entries.iter().enumerate() {
            gtk_box_pack_start(list, make_wt_row(entry, idx), 0, 1, 0);
        }
        gtk_box_pack_start(vbox, wrap_wt_scroll(list, entries.len()), 0, 1, 0);
        let actions = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let new_btn = gtk_button_new_with_label(text::WT_NEW_BTN.as_ptr());
        let prune_btn = gtk_button_new_with_label(text::WT_PRUNE_BTN.as_ptr());
        connect_click(new_btn, on_new_wt_clicked, ptr::null_mut());
        connect_click(prune_btn, on_prune_wt_clicked, ptr::null_mut());
        gtk_box_pack_start(actions, new_btn, 1, 1, 0);
        gtk_box_pack_start(actions, prune_btn, 1, 1, 0);
        gtk_box_pack_start(vbox, actions, 0, 1, 2);
    }
}

fn active_local_repo_root() -> Option<String> {
    let home = home_dir();
    let cwd = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.id == app.active_id)?.active_local_cwd()
    })
    .flatten()?;
    resolve_git_repo(&cwd, &home).map(|(root, _)| root)
}

extern "C" fn on_open_wt_clicked(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let path = WT_POPOVER.with(|cell| {
        cell.borrow().entries.get(idx_ptr as usize).map(|entry| entry.path.clone())
    });
    popdown_wt_popover();
    if let Some(wt_path) = path {
        open_or_focus_worktree_tab(&wt_path);
        app_state::focus_active_vte();
    }
}

struct RemoveDone {
    repo_root: String,
    result: Result<WorktreeEntry, String>,
}

extern "C" fn on_remove_row_done(data: *mut c_void) -> c_int {
    if data.is_null() {
        return 0;
    }
    let done = unsafe { *Box::from_raw(data as *mut RemoveDone) };
    match done.result {
        Ok(entry) => {
            app_state::with_app(|app| {
                app.git_scheduler.remove_root(&entry.path.to_string_lossy());
                app.git_scheduler.request_refresh(&done.repo_root);
            });
            app_state::set_footer_message(&format!(
                "{} {}",
                text::WT_MSG_REMOVED.as_str(),
                entry.display_name()
            ));
        }
        Err(err) => app_state::set_footer_message(&format!("⚠ {err}")),
    }
    app_state::focus_active_vte();
    0
}

extern "C" fn on_remove_row_clicked(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let target: Option<(String, PathBuf)> = WT_POPOVER.with(|cell| {
        let st = cell.borrow();
        let path = st.entries.get(idx_ptr as usize)?.path.clone();
        Some((st.repo_root.clone(), path))
    });
    popdown_wt_popover();
    let Some((repo_root, wt_path)) = target else { return };
    thread::spawn(move || {
        let result = remove_worktree(Path::new(&repo_root), &wt_path);
        let payload = Box::new(RemoveDone { repo_root, result });
        unsafe { g_idle_add(on_remove_row_done, Box::into_raw(payload) as *mut c_void) };
    });
}

extern "C" fn on_new_wt_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let root = WT_POPOVER.with(|cell| cell.borrow().repo_root.clone());
    popdown_wt_popover();
    if root.is_empty() {
        open_new_worktree_dialog_for_active();
    } else {
        open_new_worktree_dialog(&root);
    }
}

extern "C" fn on_prune_wt_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let root = WT_POPOVER.with(|cell| cell.borrow().repo_root.clone());
    popdown_wt_popover();
    if root.is_empty() {
        prune_merged_for_active();
    } else {
        spawn_prune_merged(root);
    }
}

struct PruneDone {
    repo_root: String,
    result: Result<PruneReport, String>,
}

fn format_prune_summary(report: &PruneReport) -> String {
    if report.removed.is_empty() {
        text::WT_MSG_PRUNED_NONE.as_str().to_owned()
    } else {
        format!("{} {}", text::WT_MSG_PRUNED_DONE.as_str(), report.removed.len())
    }
}

extern "C" fn on_prune_done(data: *mut c_void) -> c_int {
    WT_POPOVER.with(|cell| cell.borrow_mut().prune_busy = false);
    if data.is_null() {
        return 0;
    }
    let done = unsafe { *Box::from_raw(data as *mut PruneDone) };
    match done.result {
        Ok(report) => {
            app_state::with_app(|app| {
                for removed in &report.removed {
                    app.git_scheduler.remove_root(&removed.path.to_string_lossy());
                }
                app.git_scheduler.request_refresh(&done.repo_root);
            });
            app_state::set_footer_message(&format_prune_summary(&report));
        }
        Err(err) => app_state::set_footer_message(&format!("⚠ {err}")),
    }
    app_state::focus_active_vte();
    0
}

fn spawn_prune_merged(repo_root: String) {
    let already_busy = WT_POPOVER.with(|cell| {
        let mut st = cell.borrow_mut();
        let prev = st.prune_busy;
        st.prune_busy = true;
        prev
    });
    if already_busy {
        return;
    }
    thread::spawn(move || {
        let result = prune_merged_worktrees(Path::new(&repo_root));
        let payload = Box::new(PruneDone { repo_root, result });
        unsafe { g_idle_add(on_prune_done, Box::into_raw(payload) as *mut c_void) };
    });
}

pub fn prune_merged_for_active() {
    match active_local_repo_root() {
        Some(root) => spawn_prune_merged(root),
        None => app_state::set_footer_message(text::WT_LOCAL_ONLY.as_str()),
    }
}

struct PendingAgentCmd {
    sid: usize,
    cmd: &'static str,
}

extern "C" fn on_delayed_agent_cmd(data: *mut c_void) -> c_int {
    if data.is_null() {
        return 0;
    }
    let pending = unsafe { *Box::from_raw(data as *mut PendingAgentCmd) };
    let vte = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.id == pending.sid).map(|s| s.active_vte())
    })
    .flatten();
    if let Some(widget) = vte.filter(|w| !w.is_null()) {
        feed_vte_text(widget, pending.cmd, false);
    }
    0
}

fn schedule_agent_launch(sid: usize, cmd: &'static str) {
    let boxed = Box::new(PendingAgentCmd { sid, cmd });
    unsafe { g_timeout_add(AGENT_DELAY_MS, on_delayed_agent_cmd, Box::into_raw(boxed) as *mut c_void) };
}

fn close_wt_dialog() {
    let win = WT_DIALOG.with(|cell| cell.borrow_mut().take().map(|d| d.window));
    if let Some(w) = win.filter(|p| !p.is_null()) {
        unsafe { gtk_widget_destroy(w) };
    }
}

fn set_dialog_error(msg: &str) {
    WT_DIALOG.with(|cell| {
        if let Some(ref dlg) = *cell.borrow() {
            set_label_text(dlg.err_lbl, msg);
            unsafe { gtk_widget_set_sensitive(dlg.create_btn, 1) };
        }
    });
}

fn read_entry_text(entry: *mut GtkWidget) -> String {
    unsafe {
        let ptr = gtk_entry_get_text(entry);
        if ptr.is_null() { String::new() } else { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
    }
}

struct CreateDone {
    repo_root: String,
    agent_cmd: Option<&'static str>,
    result: Result<WorktreeEntry, String>,
}

extern "C" fn on_create_done(data: *mut c_void) -> c_int {
    if data.is_null() {
        return 0;
    }
    let done = unsafe { *Box::from_raw(data as *mut CreateDone) };
    match done.result {
        Ok(entry) => {
            close_wt_dialog();
            let wt_str = entry.path.to_string_lossy().into_owned();
            let sid = session_ops::open_local_tab_in_folder(&wt_str);
            if let Some(cmd) = done.agent_cmd {
                schedule_agent_launch(sid, cmd);
            }
            app_state::with_app(|app| app.git_scheduler.request_refresh(&done.repo_root));
            app_state::set_footer_message(&format!(
                "{} {}",
                text::WT_MSG_CREATED.as_str(),
                entry.display_name()
            ));
        }
        Err(err) => {
            set_dialog_error(&err);
            app_state::set_footer_message(&format!("⚠ {err}"));
        }
    }
    0
}

extern "C" fn on_dialog_submit(_widget: *mut GtkWidget, _data: *mut c_void) {
    let params = WT_DIALOG.with(|cell| {
        let dlg = cell.borrow();
        let d = dlg.as_ref()?;
        let slug = read_entry_text(d.branch_entry);
        let base = read_entry_text(d.base_entry);
        let agent_idx = unsafe { gtk_combo_box_get_active(d.agent_combo) };
        Some((d.repo_root.clone(), slug, base, agent_command_for_index(agent_idx), d.create_btn))
    });
    let Some((repo_root, slug_raw, base_raw, agent_cmd, create_btn)) = params else {
        return;
    };
    let slug = match validate_branch_slug(&slug_raw) {
        Ok(v) => v,
        Err(err) => return set_dialog_error(&err),
    };
    let base = match validate_base_ref(&base_raw) {
        Ok(v) => v,
        Err(err) => return set_dialog_error(&err),
    };
    set_dialog_error("");
    unsafe { gtk_widget_set_sensitive(create_btn, 0) };
    thread::spawn(move || {
        let result = create_worktree(Path::new(&repo_root), &slug, &base);
        let payload = Box::new(CreateDone { repo_root, agent_cmd, result });
        unsafe { g_idle_add(on_create_done, Box::into_raw(payload) as *mut c_void) };
    });
}

const GDK_KEY_ESCAPE: c_uint = 0xff1b;

extern "C" fn on_dialog_cancel(_btn: *mut GtkWidget, _data: *mut c_void) {
    close_wt_dialog();
}

extern "C" fn on_dialog_destroy(_win: *mut GtkWidget, _data: *mut c_void) {
    WT_DIALOG.with(|cell| {
        let _ = cell.borrow_mut().take();
    });
}

extern "C" fn on_dialog_key(_win: *mut GtkWidget, event: *mut GdkEvent, _data: *mut c_void) -> c_int {
    let mut keyval: c_uint = 0;
    if !event.is_null() && unsafe { gdk_event_get_keyval(event, &mut keyval) } != 0 && keyval == GDK_KEY_ESCAPE {
        close_wt_dialog();
        return 1;
    }
    0
}

fn make_field_label(lbl_text: crate::i18n::UiText) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(lbl_text.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-header");
        lbl
    }
}

fn build_agent_combo() -> *mut GtkWidget {
    unsafe {
        let combo = gtk_combo_box_text_new();
        for (name, _) in WORKTREE_AGENTS {
            if let Ok(c) = std::ffi::CString::new(name) {
                gtk_combo_box_text_append_text(combo, c.as_ptr());
            }
        }
        gtk_combo_box_set_active(combo, 0);
        combo
    }
}

fn wire_dialog_signals(win: *mut GtkWidget, branch: *mut GtkWidget, base: *mut GtkWidget, cancel: *mut GtkWidget, create: *mut GtkWidget) {
    connect_click(cancel, on_dialog_cancel, ptr::null_mut());
    connect_click(create, on_dialog_submit, ptr::null_mut());
    unsafe {
        for entry in [branch, base] {
            g_signal_connect_data(entry, c"activate".as_ptr(), on_dialog_submit as *const c_void, ptr::null_mut(), ptr::null(), 0);
        }
        g_signal_connect_data(win, c"key-press-event".as_ptr(), on_dialog_key as *const c_void, ptr::null_mut(), ptr::null(), 0);
        g_signal_connect_data(win, c"destroy".as_ptr(), on_dialog_destroy as *const c_void, ptr::null_mut(), ptr::null(), 0);
    }
}

fn make_dialog_window() -> *mut GtkWidget {
    unsafe {
        let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        gtk_window_set_title(win as *mut GtkWindow, text::WT_DLG_TITLE.as_ptr());
        gtk_window_set_default_size(win as *mut GtkWindow, 400, 220);
        gtk_window_set_resizable(win as *mut GtkWindow, 0);
        gtk_window_set_modal(win, 1);
        if let Some(parent) = app_state::with_app(|app| app.win).filter(|w| !w.is_null()) {
            gtk_window_set_transient_for(win, parent);
            gtk_window_set_position(win, GTK_WIN_POS_CENTER_ON_PARENT);
        }
        win
    }
}

fn make_dialog_buttons() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let btns = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        let cancel_btn = gtk_button_new_with_label(text::WT_DLG_CANCEL.as_ptr());
        let create_btn = gtk_button_new_with_label(text::WT_DLG_CREATE.as_ptr());
        add_class(create_btn, "btn-mi-a");
        gtk_box_pack_end(btns, create_btn, 0, 0, 0);
        gtk_box_pack_end(btns, cancel_btn, 0, 0, 0);
        (btns, cancel_btn, create_btn)
    }
}

fn make_padded_vbox() -> *mut GtkWidget {
    unsafe {
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
        for setter in [gtk_widget_set_margin_start, gtk_widget_set_margin_end, gtk_widget_set_margin_top, gtk_widget_set_margin_bottom] {
            setter(vbox, 14);
        }
        vbox
    }
}

fn build_wt_dialog(repo_root: &str) -> NewWtDialog {
    unsafe {
        let win = make_dialog_window();
        let vbox = make_padded_vbox();
        let branch_entry = gtk_entry_new();
        gtk_entry_set_placeholder_text(branch_entry, text::WT_DLG_BRANCH_PH.as_ptr());
        let base_entry = gtk_entry_new();
        gtk_entry_set_text(base_entry, c"HEAD".as_ptr());
        let agent_combo = build_agent_combo();
        let err_lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(err_lbl, 0.0);
        add_class(err_lbl, "ssh-count");
        let (btns, cancel_btn, create_btn) = make_dialog_buttons();
        for w in [
            make_field_label(text::WT_DLG_BRANCH_LBL), branch_entry,
            make_field_label(text::WT_DLG_BASE_LBL), base_entry,
            make_field_label(text::WT_DLG_AGENT_LBL), agent_combo, err_lbl, btns,
        ] {
            gtk_box_pack_start(vbox, w, 0, 1, 0);
        }
        gtk_container_add(win, vbox);
        wire_dialog_signals(win, branch_entry, base_entry, cancel_btn, create_btn);
        gtk_widget_show_all(win);
        gtk_widget_grab_focus(branch_entry);
        NewWtDialog { window: win, branch_entry, base_entry, agent_combo, err_lbl, create_btn, repo_root: repo_root.to_owned() }
    }
}

pub fn open_new_worktree_dialog(repo_root: &str) {
    close_wt_dialog();
    let dlg = build_wt_dialog(repo_root);
    WT_DIALOG.with(|cell| *cell.borrow_mut() = Some(dlg));
}

pub fn open_new_worktree_dialog_for_active() {
    match active_local_repo_root() {
        Some(root) => open_new_worktree_dialog(&root),
        None => app_state::set_footer_message(text::WT_LOCAL_ONLY.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc_radar::tests::with_headless_gtk;

    #[test]
    fn maps_agent_indices_and_formats_worktree_rows() {
        assert_eq!(agent_command_for_index(0), None);
        assert_eq!(agent_command_for_index(1), Some("claude\n"));
        assert_eq!(agent_command_for_index(2), Some("codex\n"));
        assert_eq!(agent_command_for_index(3), Some("agy\n"));
        assert_eq!(agent_command_for_index(-1), None);

        let entry = WorktreeEntry {
            path: PathBuf::from("/tmp/repo"),
            branch: Some("main".to_owned()),
            head_sha: "abcdef1234567".to_owned(),
            is_primary: true,
            is_detached: false,
            is_dirty: true,
            is_merged: false,
        };
        let label = format_worktree_row_label(&entry);
        assert!(label.contains("main"));
        assert!(!label.contains("->") && !label.contains('→'));
    }

    #[test]
    fn headless_packs_worktree_section_and_opens_dialog() {
        with_headless_gtk(|| {
            let vbox = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 4) };
            pack_worktree_section(vbox, ptr::null_mut(), ".");
            open_new_worktree_dialog(".");
            close_wt_dialog();
            unsafe { gtk_widget_destroy(vbox) };
        });
    }

    #[test]
    fn palette_exposes_bilingual_worktree_actions() {
        use crate::i18n::UiLocale;
        use crate::palette::{collect_items_for_locale, PaletteAction, PaletteSource};
        let src = PaletteSource {
            sessions: &[], tmux_items: &[], profiles: &[], ssh_hosts: &[],
            briefs_dir: None, extra_handoffs: &[], active_git: None, active_is_worktree: false,
        };
        let ru = collect_items_for_locale(&src, UiLocale::Ru);
        let en = collect_items_for_locale(&src, UiLocale::En);
        assert!(ru.iter().any(|i| i.action == PaletteAction::NewGitWorktree && i.title == "Создать Git Worktree"));
        assert!(en.iter().any(|i| i.action == PaletteAction::NewGitWorktree && i.title == "New Git Worktree"));
        assert!(ru.iter().any(|i| i.action == PaletteAction::PruneMergedWorktrees && i.title == "Очистить слитые Worktree"));
        assert!(en.iter().any(|i| i.action == PaletteAction::PruneMergedWorktrees && i.title == "Prune Merged Worktrees"));
    }
}
