//! Core application state, thread-local cell, and shared session/widget helpers.

use std::cell::{Cell, RefCell};
use std::ffi::{c_void, CStr, CString};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::WorkspaceConfig;
use crate::ffi::*;
use crate::git_status::{GitMenu, GitStatusScheduler};
use crate::handoff::HandoffMenu;
use crate::mi_watch::MiScheduler;
use crate::palette;
use crate::proc_radar::{BoxTelemetry, RemoteTmuxItem, SshHostEntry};
use crate::remote_telemetry::RemoteTelemetryScheduler;
use crate::search_bar;
use crate::session::TermSession;
use crate::sidebar::Sidebar;
use crate::tab_order;
use crate::tab_strip::TabStrip;
use crate::tabs::TabStore;
use crate::telemetry::TokensMenu;
use crate::theme_mode;

pub(crate) struct AppState {
    pub(crate) config: WorkspaceConfig,
    pub(crate) win: *mut GtkWidget,
    pub(crate) is_fullscreen: bool,
    pub(crate) sidebar: Sidebar,
    pub(crate) sidebar_visible: bool,
    pub(crate) theme_mode: theme_mode::ThemeMode,
    pub(crate) theme_btn: *mut GtkWidget,
    pub(crate) box_btn: *mut GtkWidget,
    pub(crate) servers_btn: *mut GtkWidget,
    pub(crate) handoff_btn: *mut GtkWidget,
    pub(crate) agents_btn: *mut GtkWidget,
    pub(crate) prefs_btn: *mut GtkWidget,
    pub(crate) swarm_btn: *mut GtkWidget,
    pub(crate) font_scale: f64,
    pub(crate) zoom_btn: *mut GtkWidget,
    pub(crate) reconnect_btn: *mut GtkWidget,
    pub(crate) split_vert_btn: *mut GtkWidget,
    pub(crate) split_horiz_btn: *mut GtkWidget,
    pub(crate) next_id: usize,
    pub(crate) active_id: usize,
    pub(crate) sessions: Vec<TermSession>,
    pub(crate) tab_store: Option<TabStore>,
    pub(crate) stack: *mut GtkWidget,
    pub(crate) tab_strip: TabStrip,
    pub(crate) mi_btn: *mut GtkWidget,
    pub(crate) mi_pop: *mut GtkWidget,
    pub(crate) mi_pop_label: *mut GtkWidget,
    pub(crate) mi_refactor_btn: *mut GtkWidget,
    pub(crate) tokens_menu: TokensMenu,
    pub(crate) agents_pop: *mut GtkWidget,
    pub(crate) box_pop: *mut GtkWidget,
    pub(crate) box_gpu_label: *mut GtkWidget,
    pub(crate) box_tmux_pop_box: *mut GtkWidget,
    pub(crate) settings_pop: *mut GtkWidget,
    pub(crate) ssh_count_label: *mut GtkWidget,
    pub(crate) git_menu: GitMenu,
    pub(crate) handoff: HandoffMenu,
    pub(crate) footer_status: *mut GtkWidget,
    pub(crate) footer_hold_until: Cell<Option<Instant>>,
    pub(crate) ssh_hosts: Vec<SshHostEntry>,
    pub(crate) ssh_toggle_btns: Vec<*mut GtkWidget>,
    pub(crate) ssh_rows: Vec<(String, *mut GtkWidget)>,
    pub(crate) box_ssh_rows: Vec<(String, *mut GtkWidget)>,
    pub(crate) sidebar_filter: String,
    pub(crate) box_filter: String,
    pub(crate) enabled_hosts_shared: Arc<Mutex<Vec<String>>>,
    pub(crate) tmux_jump_btns: Vec<(String, String, String, *mut GtkWidget, *mut GtkWidget)>,
    pub(crate) last_tmux_keys: Vec<String>,
    pub(crate) mi_scheduler: MiScheduler,
    pub(crate) git_scheduler: GitStatusScheduler,
    pub(crate) remote_telemetry: RemoteTelemetryScheduler,
    pub(crate) box_state: Arc<Mutex<BoxTelemetry>>,
    pub(crate) palette: palette::CommandPalette,
    pub(crate) search_bar: search_bar::SearchBar,
    pub(crate) pin_pop: tab_order::TabPinPopover,
    pub(crate) swarm_pop: crate::swarm_view::SwarmPopover,
    pub(crate) swarm_view: crate::swarm_view::SwarmView,
}

thread_local! {
    pub(crate) static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
    pub(crate) static PASTE_BUSY: Cell<bool> = const { Cell::new(false) };
    pub(crate) static WT_REMOVE_BUSY: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn with_app<R>(f: impl FnOnce(&AppState) -> R) -> Option<R> {
    APP.with(|cell| {
        let state = cell.try_borrow().ok()?;
        state.as_ref().map(f)
    })
}

pub(crate) fn with_app_mut<R>(f: impl FnOnce(&mut AppState) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        state.as_mut().map(f)
    })
}

pub(crate) fn active_session_id() -> usize {
    with_app(|app| app.active_id).unwrap_or(0)
}

pub(crate) fn focus_active_vte() {
    let vte = with_app(|app| {
        app.sessions
            .iter()
            .find(|s| s.id == app.active_id)
            .map(TermSession::active_vte)
    })
    .flatten();
    if let Some(w) = vte.filter(|w| !w.is_null()) {
        unsafe { gtk_widget_grab_focus(w) };
    }
}

pub(crate) fn set_footer_message(message: &str) {
    if message.is_empty() {
        return;
    }
    let footer = with_app(|app| {
        app.footer_hold_until
            .set(Some(Instant::now() + Duration::from_secs(4)));
        app.footer_status
    });
    if let Some(lbl) = footer.filter(|w| !w.is_null()) {
        set_label_text(lbl, message);
    }
}

pub(crate) fn entry_text_lower(entry: *mut GtkWidget) -> String {
    unsafe {
        let ptr = gtk_entry_get_text(entry);
        if ptr.is_null() {
            return String::new();
        }
        CStr::from_ptr(ptr).to_string_lossy().trim().to_lowercase()
    }
}

pub(crate) fn has_live_vte(vte: *mut GtkWidget) -> bool {
    if vte.is_null() {
        return false;
    }
    with_app(|app| app.sessions.iter().any(|s| s.has_vte(vte))).unwrap_or(false)
}

pub(crate) fn record_pane_focus(sid: usize, vte: *mut GtkWidget) {
    if vte.is_null() {
        return;
    }
    with_app_mut(|app| {
        if let Some(s) = app.sessions.iter_mut().find(|s| s.id == sid) {
            s.panes.record_focus(vte);
        }
    });
}

pub(crate) fn snapshot_active_panes() -> Option<(usize, Vec<*mut GtkWidget>, *mut GtkWidget)> {
    with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        let mut vtes = Vec::new();
        s.panes.collect_vtes(s.vte, &mut vtes);
        Some((s.id, vtes, s.panes.last_focus))
    })
    .flatten()
}

pub(crate) fn pick_session_vte(s: &TermSession, preferred: *mut GtkWidget) -> *mut GtkWidget {
    if !preferred.is_null() && s.has_vte(preferred) {
        preferred
    } else {
        s.active_vte()
    }
}

pub(crate) fn pane_cwd(s: &TermSession, pane_vte: *mut GtkWidget) -> String {
    s.panes
        .live_cwd_for_vte(pane_vte, s.vte, s.child_pid, &s.cwd)
        .or_else(|| s.active_local_cwd())
        .unwrap_or_else(|| s.cwd.clone())
}

pub(crate) fn session_ssh_host(s: &TermSession) -> String {
    s.remote()
        .map_or(&s.proc_info.ssh_host, |r| &r.host)
        .clone()
}

pub(crate) fn collect_local_cwds(app: &AppState) -> Vec<String> {
    let mut cwds = Vec::new();
    if let Some(cwd) = app
        .sessions
        .iter()
        .find(|s| s.id == app.active_id)
        .and_then(TermSession::active_local_cwd)
    {
        cwds.push(cwd);
    }
    for s in &app.sessions {
        if let Some(cwd) = s.active_local_cwd().filter(|c| !cwds.contains(c)) {
            cwds.push(cwd);
        }
    }
    cwds
}

pub(crate) fn lookup_tabs_box() -> Option<*mut GtkWidget> {
    with_app(|app| app.tab_strip.tabs_box())
}

pub(crate) fn is_overlay_open() -> bool {
    with_app(|app| app.palette.is_open() || app.pin_pop.is_open()).unwrap_or(false)
}

pub(crate) fn close_open_palette() {
    let pal = with_app(|app| app.palette.is_open().then(|| app.palette.clone())).flatten();
    if let Some(pal) = pal {
        pal.close();
    }
}

pub(crate) fn snapshot_box_tmux_items(app: &AppState) -> Vec<RemoteTmuxItem> {
    app.box_state
        .lock()
        .ok()
        .map(|g| g.tmux_items.clone())
        .unwrap_or_default()
}

pub(crate) fn make_section_header(title: &str) -> *mut GtkWidget {
    let c = CString::new(title).unwrap();
    unsafe {
        let lbl = gtk_label_new(c.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-header");
        lbl
    }
}

pub fn connect_click(
    btn: *mut GtkWidget,
    cb: extern "C" fn(*mut GtkWidget, *mut c_void),
    data: *mut c_void,
) {
    let sig = CString::new("clicked").unwrap();
    unsafe {
        g_signal_connect_data(btn, sig.as_ptr(), cb as *const c_void, data, ptr::null(), 0);
    }
}

pub fn connect_entry_changed(
    entry: *mut GtkWidget,
    cb: extern "C" fn(*mut GtkWidget, *mut c_void),
) {
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

pub(crate) fn make_search_entry(placeholder: &str) -> *mut GtkWidget {
    let c = CString::new(placeholder).unwrap();
    unsafe {
        let entry = gtk_entry_new();
        gtk_entry_set_placeholder_text(entry, c.as_ptr());
        add_class(entry, "search-entry");
        entry
    }
}
