//! VTE-2.91 terminal widget creation, clickable link matching, and session metadata.

use std::cell::Cell;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::time::Instant;

use crate::ffi::*;
use crate::paths::expand_home;
use crate::proc_radar::{foreground_cwd, open_external_link, resolve_link_target, ProcInfo};
use crate::sidebar::SidebarRow;
use crate::split_pane::TabPanes;
use crate::tab_strip::TabView;
use crate::tabs::{RemoteConnection, Tab, TabMeta};
use crate::telemetry::SessionTelemetry;
use crate::theme::{apply_vte_colors, current_theme_mode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCategory {
    Project,
    Remote,
    Daemon,
}

impl SessionCategory {
    pub fn default_icon_name(self) -> &'static CStr {
        match self {
            Self::Project => c"utilities-terminal-symbolic",
            Self::Remote => c"network-server-symbolic",
            Self::Daemon => c"computer-symbolic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkState {
    Idle,
    Working,
    Ready,
    NeedsApproval,
}

impl WorkState {
    pub fn needs_approval(self) -> bool {
        matches!(self, Self::NeedsApproval)
    }
}

pub struct TermSession {
    pub id: usize,
    pub base_title: String,
    pub profile_cwd: String,
    pub title: String,
    pub osc_title: String,
    pub pinned_title: Option<String>,
    pub cwd: String,
    pub category: SessionCategory,
    pub vte: *mut GtkWidget,
    pub scroll_wrap: *mut GtkWidget,
    pub panes: TabPanes,
    pub sidebar_row: SidebarRow,
    pub top_tab: TabView,
    pub child_pid: i32,
    pub state: WorkState,
    pub unread: bool,
    pub last_output: Instant,
    pub work_started: Instant,
    pub work_started_at: Option<Instant>,
    pub last_turn_secs: Option<u64>,
    pub last_notified: Option<Instant>,
    pub proc_info: ProcInfo,
    pub tab: Tab,
    pub remote_tmux: Option<(String, String, String, String)>,
    pub tmux_title_from_vte: bool,
    pub telemetry: SessionTelemetry,
}

impl TermSession {
    pub fn remote(&self) -> Option<&RemoteConnection> {
        match &self.tab {
            Tab::Remote(remote) => Some(remote),
            Tab::Local { .. } => None,
        }
    }

    pub fn is_dynamic_ssh(&self) -> bool {
        self.category == SessionCategory::Project && !self.proc_info.ssh_host.is_empty()
    }

    pub fn effective_category(&self) -> SessionCategory {
        if self.is_dynamic_ssh() {
            SessionCategory::Remote
        } else {
            self.category
        }
    }

    pub fn active_vte(&self) -> *mut GtkWidget {
        self.panes.active_vte(self.vte)
    }

    pub fn active_pid(&self) -> i32 {
        self.panes.active_pid(self.vte, self.child_pid)
    }

    pub fn pid_for_vte(&self, vte: *mut GtkWidget) -> i32 {
        self.panes.pid_for_vte(vte, self.vte, self.child_pid)
    }

    pub fn has_vte(&self, vte: *mut GtkWidget) -> bool {
        self.vte == vte || self.panes.contains_vte(vte)
    }

    #[cfg(test)]
    pub fn apply_rename(&mut self, raw_title: &str) {
        self.set_pinned_title(Some(raw_title));
    }

    pub fn active_local_cwd(&self) -> Option<String> {
        if self.effective_category() == SessionCategory::Remote {
            return None;
        }
        let fallback = if !self.proc_info.cwd.is_empty() && self.active_vte() == self.vte {
            expand_home(&self.proc_info.cwd)
        } else {
            self.cwd.clone()
        };
        let cwd = self.panes.active_cwd(self.vte, self.child_pid, &fallback);
        (!cwd.is_empty()).then_some(cwd)
    }

    pub fn apply_proc_snapshot(&mut self, proc_info: ProcInfo) {
        let prev_host = std::mem::take(&mut self.proc_info.ssh_host);
        self.proc_info = proc_info;
        if self.category == SessionCategory::Project && prev_host != self.proc_info.ssh_host {
            self.osc_title.clear();
            self.remote_tmux = None;
            self.tmux_title_from_vte = false;
        }
        if self.effective_category() != SessionCategory::Remote
            && !self.proc_info.cwd.is_empty()
            && self.active_vte() == self.vte
        {
            self.cwd = expand_home(&self.proc_info.cwd);
        }
    }

    pub fn apply_vte_title(&mut self, vte: *mut GtkWidget, title_str: &str) {
        if let Some(rest) = title_str.strip_prefix("AKHTERM_TMUX:") {
            apply_tmux_vte_title(self, vte, rest);
            return;
        }
        if vte != self.active_vte() {
            return;
        }
        let clean = title_str.trim();
        self.osc_title = if self.effective_category() == SessionCategory::Remote {
            let host = self.remote().map_or(self.proc_info.ssh_host.as_str(), |r| r.host.as_str());
            clean_remote_osc_title(clean, host)
        } else {
            clean_local_osc_title(clean)
        };
    }

    pub fn live_turn_secs(&self, now: Instant) -> Option<u64> {
        if self.category == SessionCategory::Daemon
            || self.state != WorkState::Working
            || self.active_pid() <= 0
        {
            return None;
        }
        let started = self.work_started_at.unwrap_or(self.work_started);
        Some(now.saturating_duration_since(started).as_secs())
    }

    pub fn settled_turn_secs(&self) -> Option<u64> {
        resolve_settled_turn_secs(
            self.category,
            self.state,
            self.active_pid(),
            self.last_turn_secs,
        )
    }

    pub fn effective_agent(&self) -> String {
        if !self.proc_info.agent.is_empty() {
            return self.proc_info.agent.clone();
        }
        detect_agent_name(&self.title)
            .or_else(|| detect_agent_name(&self.osc_title))
            .or_else(|| self.telemetry.detected_agent_label())
            .unwrap_or("")
            .to_owned()
    }

    pub fn snapshot_tab(&mut self) -> Tab {
        let dyn_ssh = self.is_dynamic_ssh();
        if let Tab::Local { cwd, .. } = &mut self.tab {
            if !dyn_ssh {
                if let Some(current) = foreground_cwd(self.child_pid) {
                    self.cwd.clone_from(&current);
                    *cwd = current;
                } else if !self.cwd.is_empty() {
                    cwd.clone_from(&self.cwd);
                }
            } else if !self.cwd.is_empty() {
                cwd.clone_from(&self.cwd);
            }
        }
        self.tab.clone()
    }

    pub fn snapshot_meta(&mut self) -> TabMeta {
        let splits = if self.category == SessionCategory::Project && self.remote().is_none() {
            self.panes.snapshot_splits(self.scroll_wrap)
        } else {
            None
        };
        TabMeta {
            pinned_title: self.pinned_title.clone(),
            splits,
        }
    }

    pub fn set_pinned_title(&mut self, raw: Option<&str>) {
        self.pinned_title = sanitize_pinned_title(raw);
        refresh_smart_title(self);
    }
}

pub fn sanitize_pinned_title(raw: Option<&str>) -> Option<String> {
    let clean = raw?.replace('\0', "");
    let trimmed = clean.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn apply_tmux_vte_title(s: &mut TermSession, vte: *mut GtkWidget, rest: &str) {
    if s.active_vte() != vte {
        return;
    }
    let parts: Vec<&str> = rest.splitn(4, '|').collect();
    if parts.len() == 4 && !parts[0].is_empty() {
        if let Tab::Remote(remote) = &mut s.tab {
            if let Some(target) = &mut remote.tmux_session {
                *target = parts[0].to_owned();
            }
        }
        s.remote_tmux = Some((
            parts[0].to_owned(),
            parts[1].to_owned(),
            parts[2].to_owned(),
            parts[3].to_owned(),
        ));
        s.tmux_title_from_vte = true;
    }
}

fn clean_remote_osc_title(clean: &str, host: &str) -> String {
    if clean.is_empty() || matches!(clean, "zsh" | "bash" | "sh") || clean == host {
        return String::new();
    }
    let extracted = match clean.split_once(':') {
        Some((lhs, rhs)) if lhs.contains('@') => rhs.trim(),
        _ if clean.contains('@') => "",
        _ => clean,
    };
    extracted.chars().take(28).collect()
}

fn clean_local_osc_title(clean: &str) -> String {
    let keep = !clean.is_empty()
        && !clean.contains('@')
        && !clean.starts_with('~')
        && !clean.starts_with('/')
        && !matches!(clean, "zsh" | "bash");
    if keep { clean.chars().take(28).collect() } else { String::new() }
}

const URL_PATTERNS: &[&str] = &[
    r"https?://[^\s\x22'<>()`│┃┆┊║]+",
    r"file://[^\s\x22'<>()\]`│┃┆┊║]+(?:\([^\s\x22'<>()\]`│┃┆┊║]*\)[^\s\x22'<>()\]`│┃┆┊║]*)*",
    r"\b(localhost|127\.0\.0\.1|0\.0\.0\.0):[0-9]{2,5}(/[^\s\x22'<>()`│┃┆┊║]*)?",
    r"(?:~|/home/[a-zA-Z0-9_.-]+)/[^\s\x22'<>()\]`│┃┆┊║]+(?:\([^\s\x22'<>()\]`│┃┆┊║]*\)[^\s\x22'<>()\]`│┃┆┊║]*)*",
    r"(?<![^\s\x22'<>(\[{=`│┃┆┊║])(?:(?:[^\s\x22'<>():~`│┃┆┊║/\[\]{};,]+/)+[^\s\x22'<>():~`│┃┆┊║/\[\]{};,]+\.[a-zA-Z][a-zA-Z0-9]{0,9}(?::[0-9]+){0,2}|(?:[^\s\x22'<>():~`│┃┆┊║/\[\]{};,]*[^\s\x22'<>():~`│┃┆┊║/\[\]{};,.])\.[a-zA-Z][a-zA-Z0-9]{0,9}:[0-9]+(?::[0-9]+)?)",
];

pub enum LinkCwd {
    Unmanaged,
    Remote,
    Local(String),
}

struct VteLinkCtx {
    pid: i32,
    fallback_cwd: String,
}

pub type VteMenuHook = fn(*mut GtkWidget, *const GdkEvent, Option<String>);

thread_local! {
    static LINK_CWD_FN: Cell<Option<fn(*mut GtkWidget) -> LinkCwd>> = const { Cell::new(None) };
    static VTE_MENU_FN: Cell<Option<VteMenuHook>> = const { Cell::new(None) };
}

pub fn register_link_cwd_lookup(lookup: fn(*mut GtkWidget) -> LinkCwd) {
    LINK_CWD_FN.with(|slot| slot.set(Some(lookup)));
}

pub fn register_vte_menu_hook(hook: VteMenuHook) {
    VTE_MENU_FN.with(|slot| slot.set(Some(hook)));
}

extern "C" fn drop_vte_link_ctx(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data.cast::<VteLinkCtx>())) };
    }
}

fn resolve_vte_cwd(vte: *mut GtkWidget, user_data: *mut c_void) -> Option<String> {
    if let Some(lookup) = LINK_CWD_FN.with(Cell::get) {
        match lookup(vte) {
            LinkCwd::Local(dir) => return Some(dir),
            LinkCwd::Remote => return None,
            LinkCwd::Unmanaged => {}
        }
    }
    if user_data.is_null() {
        return None;
    }
    let ctx = unsafe { &*user_data.cast::<VteLinkCtx>() };
    foreground_cwd(ctx.pid)
        .or_else(|| (!ctx.fallback_cwd.is_empty()).then(|| ctx.fallback_cwd.clone()))
}

fn take_c_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    unsafe { g_free(ptr as *mut c_void) };
    Some(text)
}

pub fn check_vte_event_link(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    cwd: Option<&str>,
) -> Option<String> {
    if vte.is_null() || event.is_null() {
        return None;
    }
    unsafe {
        if let Some(hlink) = take_c_string(vte_terminal_hyperlink_check_event(vte, event)) {
            if let Some(target) = resolve_link_target(&hlink, cwd) {
                return Some(target);
            }
        }
        let mut tag: c_int = -1;
        let matched = take_c_string(vte_terminal_match_check_event(vte, event, &mut tag))?;
        resolve_link_target(&matched, cwd)
    }
}

fn handle_vte_right_click(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    user_data: *mut c_void,
) -> c_int {
    let mut state: u32 = 0;
    unsafe { gdk_event_get_state(event, &mut state) };
    if crate::context_menu::should_passthrough_vte_click(3, state) {
        return 0;
    }
    let Some(hook) = VTE_MENU_FN.with(Cell::get) else {
        return 0;
    };
    let cwd = resolve_vte_cwd(vte, user_data);
    let link = check_vte_event_link(vte, event, cwd.as_deref());
    hook(vte, event, link);
    1
}

pub fn should_open_vte_link(button: u32, state: u32) -> bool {
    if button != 1 {
        return false;
    }
    let ctrl_held = (state & GDK_CONTROL_MASK) != 0;
    let no_alt = (state & GDK_MOD1_MASK) == 0;
    ctrl_held && no_alt
}

fn handle_vte_left_click(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    state: u32,
    user_data: *mut c_void,
) -> c_int {
    if !should_open_vte_link(1, state) {
        return 0;
    }
    let cwd = resolve_vte_cwd(vte, user_data);
    if let Some(target) = check_vte_event_link(vte, event, cwd.as_deref()) {
        open_external_link(&target);
        return 1;
    }
    0
}

extern "C" fn on_vte_button_press(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    user_data: *mut c_void,
) -> c_int {
    let mut button: u32 = 0;
    let mut state: u32 = 0;
    let is_press = unsafe {
        !event.is_null()
            && gdk_event_get_event_type(event) == GDK_BUTTON_PRESS
            && gdk_event_get_button(event, &mut button) != 0
    };
    if !is_press {
        return 0;
    }
    unsafe { gdk_event_get_state(event, &mut state) };
    match button {
        1 => handle_vte_left_click(vte, event, state, user_data),
        3 => handle_vte_right_click(vte, event, user_data),
        _ => 0,
    }
}

fn apply_vte_style(vte: *mut GtkWidget) {
    unsafe {
        vte_terminal_set_scrollback_lines(vte, 15_000);
        vte_terminal_set_allow_hyperlink(vte, 1);
        apply_vte_colors(vte, current_theme_mode());
        let desc = pango_font_description_from_string(c"DejaVu Sans Mono 10.5".as_ptr());
        if !desc.is_null() {
            vte_terminal_set_font(vte, desc);
            pango_font_description_free(desc);
        }
    }
}

fn register_vte_url_matches(vte: *mut GtkWidget) {
    unsafe {
        for pat in URL_PATTERNS {
            let Ok(pat_c) = CString::new(*pat) else { continue };
            let vreg = vte_regex_new_for_match(pat_c.as_ptr(), -1, PCRE2_MULTILINE_UTF, ptr::null_mut());
            if !vreg.is_null() {
                let tag = vte_terminal_match_add_regex(vte, vreg, 0);
                if tag >= 0 {
                    vte_terminal_match_set_cursor_name(vte, tag, c"pointer".as_ptr());
                }
                vte_regex_unref(vreg);
            }
        }
    }
}

fn spawn_vte_command(vte: *mut GtkWidget, cwd: &str, argv_strs: &[&str]) -> i32 {
    let c_cwd = CString::new(cwd).unwrap_or_else(|_| CString::new("/tmp").unwrap());
    let c_args: Vec<CString> = argv_strs.iter().filter_map(|s| CString::new(*s).ok()).collect();
    let mut ptrs: Vec<*const c_char> = c_args.iter().map(|s| s.as_ptr()).collect();
    ptrs.push(ptr::null());
    let mut pid: i32 = 0;
    let spawned = unsafe {
        vte_terminal_spawn_sync(
            vte,
            0,
            c_cwd.as_ptr(),
            ptrs.as_ptr(),
            ptr::null(),
            G_SPAWN_SEARCH_PATH,
            ptr::null(),
            ptr::null_mut(),
            &mut pid,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if spawned == 0 { 0 } else { pid }
}

pub fn create_vte_widget(cwd: &str, argv_strs: &[&str]) -> (*mut GtkWidget, i32) {
    unsafe {
        let vte = vte_terminal_new();
        apply_vte_style(vte);
        register_vte_url_matches(vte);
        crate::file_drop::setup_vte_file_drop(vte);
        let pid = spawn_vte_command(vte, cwd, argv_strs);
        let ctx = Box::into_raw(Box::new(VteLinkCtx {
            pid,
            fallback_cwd: expand_home(cwd),
        }));
        g_signal_connect_data(
            vte,
            c"button-press-event".as_ptr(),
            on_vte_button_press as *const c_void,
            ctx.cast(),
            drop_vte_link_ctx as *const c_void,
            0,
        );
        (vte, pid)
    }
}

pub fn resolve_settled_turn_secs(
    category: SessionCategory,
    state: WorkState,
    active_pid: i32,
    last_turn_secs: Option<u64>,
) -> Option<u64> {
    (category != SessionCategory::Daemon && state != WorkState::Working && active_pid > 0)
        .then_some(last_turn_secs)
        .flatten()
}

pub fn detect_agent_name(raw: &str) -> Option<&'static str> {
    let low = raw.to_lowercase();
    let first_word = low.split_whitespace().next().map(|word| word.trim_end_matches(':'));
    if matches!(first_word, Some("deepseek" | "deepseek-cli")) {
        Some("DeepSeek")
    } else if low.contains("agy") || low.contains("antigravity") {
        Some("AGY")
    } else if low.contains("codex") {
        Some("Codex")
    } else if low.contains("dsh") {
        Some("DSH")
    } else if low.contains("kimi") {
        Some("Kimi")
    } else if low.contains("claude") {
        Some("Claude")
    } else {
        None
    }
}

fn folder_name(path: &str) -> &str {
    if path == "/" {
        return "/";
    }
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

fn normalize_dir(path: &str) -> String {
    let expanded = expand_home(path);
    if expanded.len() > 1 {
        expanded.trim_end_matches('/').to_owned()
    } else {
        expanded
    }
}

fn moved_from_profile(current_cwd: &str, profile_cwd: &str) -> bool {
    !current_cwd.is_empty()
        && !profile_cwd.is_empty()
        && normalize_dir(current_cwd) != normalize_dir(profile_cwd)
}

fn format_project_title(
    base_title: &str,
    profile_cwd: &str,
    current_cwd: &str,
    agent: &str,
    osc_title: &str,
) -> String {
    let folder = folder_name(current_cwd);
    let moved = moved_from_profile(current_cwd, profile_cwd) && !folder.is_empty();
    let short_base = if moved {
        folder
    } else {
        base_title.split(" · ").next().unwrap_or(base_title)
    };
    let osc = (!osc_title.is_empty() && osc_title != short_base).then_some(osc_title);
    match (agent.is_empty(), osc) {
        (false, Some(osc)) => format!("{short_base} · {agent} · {osc}"),
        (false, None) => format!("{short_base} · {agent}"),
        (true, Some(osc)) => format!("{short_base} · {osc}"),
        (true, None) if moved => folder.to_owned(),
        (true, None) => base_title.to_owned(),
    }
}

fn format_remote_title(host: &str, sess: &str, win: &str, folder: &str, agent: &str) -> String {
    let clean_folder = if !folder.is_empty() && folder != "~" {
        format!(" · {folder}")
    } else {
        String::new()
    };
    if !agent.is_empty() {
        format!("{host}{clean_folder} · {agent} (tmux:{sess})")
    } else {
        format!("{host}{clean_folder} · tmux:{sess}/{win}")
    }
}

fn update_detected_agent(s: &mut TermSession) {
    if !s.proc_info.agent.is_empty() {
        return;
    }
    if let Some((_, ref win, ref cmd, _)) = s.remote_tmux {
        if let Some(a) = detect_agent_name(cmd).or_else(|| detect_agent_name(win)) {
            s.proc_info.agent = a.to_string();
            return;
        }
    }
    if !s.osc_title.is_empty() {
        if let Some(a) = detect_agent_name(&s.osc_title) {
            s.proc_info.agent = a.to_string();
        }
    }
}

fn format_ssh_plain_title(host: &str, agent: &str, osc_title: &str) -> String {
    let base = if host.is_empty() { "SSH".to_owned() } else { format!("ssh {host}") };
    let osc = (!osc_title.is_empty() && osc_title != host).then_some(osc_title);
    match (agent.is_empty(), osc) {
        (false, Some(osc)) => format!("{base} · {agent} · {osc}"),
        (false, None) => format!("{base} · {agent}"),
        (true, Some(osc)) => format!("{base} · {osc}"),
        (true, None) => base,
    }
}

fn refresh_remote_session_title(s: &mut TermSession) {
    let host = s
        .remote()
        .map(|r| r.host.as_str())
        .filter(|h| !h.is_empty())
        .unwrap_or(s.proc_info.ssh_host.as_str());
    if let Some((ref sess, ref win, _, ref path)) = s.remote_tmux {
        s.title = format_remote_title(host, sess, win, folder_name(path), &s.proc_info.agent);
    } else if let Some(remote) = s.remote().filter(|r| r.tmux_session.is_some()) {
        s.title = remote.title();
    } else {
        s.title = format_ssh_plain_title(host, &s.proc_info.agent, &s.osc_title);
    }
}

pub fn refresh_smart_title(s: &mut TermSession) {
    update_detected_agent(s);
    if let Some(ref pinned) = s.pinned_title {
        s.title.clone_from(pinned);
        return;
    }
    match s.effective_category() {
        SessionCategory::Remote => refresh_remote_session_title(s),
        SessionCategory::Project => {
            let current_cwd = if !s.proc_info.cwd.is_empty() {
                s.proc_info.cwd.as_str()
            } else {
                s.cwd.as_str()
            };
            s.title = format_project_title(
                &s.base_title,
                &s.profile_cwd,
                current_cwd,
                &s.proc_info.agent,
                &s.osc_title,
            );
        }
        SessionCategory::Daemon => {
            s.title.clone_from(&s.base_title);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_panes() -> TabPanes {
        TabPanes::empty_for_test()
    }

    fn sample_local_session(base_title: &str, dir: &str) -> TermSession {
        let now = Instant::now();
        let cat = SessionCategory::Project;
        let tab = Tab::Local { profile: "work".to_owned(), cwd: dir.to_owned() };
        TermSession {
            id: 1, base_title: base_title.to_owned(), profile_cwd: dir.to_owned(),
            title: base_title.to_owned(), osc_title: String::new(), pinned_title: None,
            cwd: dir.to_owned(), category: cat, vte: ptr::null_mut(), scroll_wrap: ptr::null_mut(),
            panes: empty_panes(), sidebar_row: SidebarRow::empty_for_test(cat),
            top_tab: TabView::empty_for_test(cat), child_pid: 0, state: WorkState::Idle,
            unread: false, last_output: now, work_started: now, work_started_at: None,
            last_turn_secs: None, last_notified: None, proc_info: ProcInfo::default(), tab,
            remote_tmux: None, tmux_title_from_vte: false, telemetry: SessionTelemetry::default(),
        }
    }

    fn assert_dynamic_ssh_active(s: &mut TermSession) {
        s.apply_proc_snapshot(ProcInfo { ssh_host: "ubuntu@prod-01".into(), ..Default::default() });
        refresh_smart_title(s);
        assert_eq!(s.effective_category(), SessionCategory::Remote);
        assert_eq!(s.active_local_cwd(), None);
        assert_eq!(s.title, "ssh ubuntu@prod-01");
        s.apply_vte_title(ptr::null_mut(), "ubuntu@prod-01: ~/backend");
        refresh_smart_title(s);
        assert_eq!(s.title, "ssh ubuntu@prod-01 · ~/backend");
    }

    #[test]
    fn dynamic_ssh_switches_category_and_restores_local_state_on_exit() {
        let mut s = sample_local_session("Productivity · Новая", "/tmp/proj");
        assert_eq!(s.effective_category(), SessionCategory::Project);
        assert_eq!(s.active_local_cwd().as_deref(), Some("/tmp/proj"));
        assert_dynamic_ssh_active(&mut s);

        s.apply_vte_title(ptr::null_mut(), "AKHTERM_TMUX:main|0|claude|/srv/app");
        refresh_smart_title(&mut s);
        assert!(s.tmux_title_from_vte);
        assert_eq!(s.title, "ubuntu@prod-01 · app · Claude (tmux:main)");
        assert_eq!(s.snapshot_tab(), Tab::Local { profile: "work".into(), cwd: "/tmp/proj".into() });

        s.apply_proc_snapshot(ProcInfo { cwd: "/tmp/proj".into(), ..Default::default() });
        refresh_smart_title(&mut s);
        assert_eq!(s.effective_category(), SessionCategory::Project);
        assert!(!s.tmux_title_from_vte && s.remote_tmux.is_none() && s.osc_title.is_empty());
        assert_eq!(s.title, "Productivity · Новая");
        assert_eq!(s.active_local_cwd().as_deref(), Some("/tmp/proj"));
    }

    #[test]
    fn settled_turn_secs_persists_on_idle_ready_and_approval_tabs() {
        assert_eq!(
            resolve_settled_turn_secs(SessionCategory::Project, WorkState::Idle, 42, Some(134)),
            Some(134)
        );
        assert_eq!(
            resolve_settled_turn_secs(SessionCategory::Project, WorkState::Ready, 42, Some(134)),
            Some(134)
        );
        assert_eq!(
            resolve_settled_turn_secs(SessionCategory::Remote, WorkState::NeedsApproval, 42, Some(18)),
            Some(18)
        );
        assert_eq!(
            resolve_settled_turn_secs(SessionCategory::Project, WorkState::Working, 42, Some(134)),
            None
        );
        assert_eq!(
            resolve_settled_turn_secs(SessionCategory::Daemon, WorkState::Idle, 42, Some(134)),
            None
        );
        assert_eq!(
            resolve_settled_turn_secs(SessionCategory::Project, WorkState::Idle, 0, Some(134)),
            None
        );
    }

    #[test]
    fn project_title_tracks_directory_changes_and_restores_profile_name() {
        let base = "Productivity · Новая";
        let prof = "/home/user/Productivity";
        assert_eq!(format_project_title(base, prof, prof, "", ""), "Productivity · Новая");
        assert_eq!(
            format_project_title(base, prof, "/home/user/Development/VoiceDetector", "", ""),
            "VoiceDetector"
        );
        assert_eq!(
            format_project_title(base, prof, "/home/user/Учёба/Курс ЦУ/", "Claude", ""),
            "Курс ЦУ · Claude"
        );
        assert_eq!(
            format_project_title(base, prof, "/home/user/Productivity/", "", ""),
            "Productivity · Новая"
        );
    }

    fn assert_regex_substitutions(pat: &str, cases: &[(&str, &str)]) {
        extern "C" {
            fn vte_regex_substitute(
                regex: *mut VteRegex,
                subject: *const c_char,
                replacement: *const c_char,
                flags: u32,
                error: *mut *mut c_void,
            ) -> *mut c_char;
        }
        let pat_c = CString::new(pat).unwrap();
        let vreg = unsafe {
            vte_regex_new_for_match(pat_c.as_ptr(), -1, PCRE2_MULTILINE_UTF, ptr::null_mut())
        };
        assert!(!vreg.is_null(), "failed to compile VTE regex: {pat}");
        for &(line, expected) in cases {
            let subj = CString::new(line).unwrap();
            let out = unsafe {
                take_c_string(vte_regex_substitute(vreg, subj.as_ptr(), c"<$0>".as_ptr(), 0, ptr::null_mut()))
            };
            assert_eq!(out.as_deref(), Some(expected), "pattern={pat} line={line}");
        }
        unsafe { vte_regex_unref(vreg) };
    }

    #[test]
    fn all_vte_url_patterns_compile_with_pcre2_multiline_utf() {
        for pat in URL_PATTERNS {
            assert_regex_substitutions(pat, &[]);
        }
        assert_regex_substitutions(
            URL_PATTERNS[3],
            &[
                (
                    "отзыв.docx~/Development/ЦБО/отзыв_финал.docx:42",
                    "отзыв.docx<~/Development/ЦБО/отзыв_финал.docx:42>",
                ),
                (
                    "[конспект](~/Development/Курс_(ЦУ)/неделя_1.md#L10-L20)",
                    "[конспект](<~/Development/Курс_(ЦУ)/неделя_1.md#L10-L20>)",
                ),
                (
                    "[~/Development/файл.md]хвост",
                    "[<~/Development/файл.md>]хвост",
                ),
                (
                    "ссылка/home/user/Development/Отзывы/рецензия.pdf'",
                    "ссылка</home/user/Development/Отзывы/рецензия.pdf>'",
                ),
            ],
        );
        assert_regex_substitutions(
            URL_PATTERNS[4],
            &[
                ("  --> src/session.rs:241:5", "  --> <src/session.rs:241:5>"),
                ("at (./app/main.py:42:5)", "at (<./app/main.py:42:5>)"),
                ("see crates/core/lib.rs now", "see <crates/core/lib.rs> now"),
                ("ошибка в учёба/модуль.py:18:3!", "ошибка в <учёба/модуль.py:18:3>!"),
                ("version v0.2.1 on 127.0.0.1", "version v0.2.1 on 127.0.0.1"),
                (
                    "ссылка~/Development/ЦБО/файл.md",
                    "ссылка~/Development/ЦБО/файл.md",
                ),
            ],
        );
    }

    #[repr(C)]
    struct TestButtonEvent {
        type_: c_int,
        window: *mut c_void,
        send_event: i8,
        time: u32,
        x: f64,
        y: f64,
        axes: *mut f64,
        state: std::ffi::c_uint,
        button: std::ffi::c_uint,
        device: *mut c_void,
        x_root: f64,
        y_root: f64,
    }

    #[repr(C)]
    struct RawGList {
        data: *mut c_void,
        next: *const RawGList,
        prev: *const RawGList,
    }

    fn find_vte_event_win(vte: *mut GtkWidget, cw: f64, ch: f64) -> *mut c_void {
        extern "C" {
            fn gdk_event_new(type_: c_int) -> *mut GdkEvent;
            fn gdk_event_free(event: *mut GdkEvent);
            fn gtk_widget_get_window(widget: *mut GtkWidget) -> *mut c_void;
            fn gdk_window_peek_children(window: *mut c_void) -> *const RawGList;
            fn g_object_ref(obj: *mut c_void) -> *mut c_void;
        }
        unsafe {
            let gwin = gtk_widget_get_window(vte);
            let mut node = if gwin.is_null() { ptr::null() } else { gdk_window_peek_children(gwin) };
            while !node.is_null() {
                let cand = (*node).data;
                node = (*node).next;
                if cand.is_null() {
                    continue;
                }
                let ev = gdk_event_new(GDK_BUTTON_PRESS);
                (*(ev as *mut TestButtonEvent)).window = g_object_ref(cand);
                (*(ev as *mut TestButtonEvent)).button = 1;
                (*(ev as *mut TestButtonEvent)).x = cw * 36.5 + 2.0;
                (*(ev as *mut TestButtonEvent)).y = ch * 0.5 + 2.0;
                let matched = check_vte_event_link(vte, ev, None).is_some();
                gdk_event_free(ev);
                if matched {
                    return cand;
                }
            }
            ptr::null_mut()
        }
    }

    fn probe_vte_cell(vte: *mut GtkWidget, ev_win: *mut c_void, col: f64, row: f64) -> Option<String> {
        extern "C" {
            fn gdk_event_new(type_: c_int) -> *mut GdkEvent;
            fn gdk_event_free(event: *mut GdkEvent);
            fn g_object_ref(obj: *mut c_void) -> *mut c_void;
            fn vte_terminal_get_char_width(vte: *mut GtkWidget) -> std::ffi::c_long;
            fn vte_terminal_get_char_height(vte: *mut GtkWidget) -> std::ffi::c_long;
        }
        unsafe {
            let cw = vte_terminal_get_char_width(vte).max(8) as f64;
            let ch = vte_terminal_get_char_height(vte).max(14) as f64;
            let ev = gdk_event_new(GDK_BUTTON_PRESS);
            (*(ev as *mut TestButtonEvent)).window = g_object_ref(ev_win);
            (*(ev as *mut TestButtonEvent)).button = 1;
            (*(ev as *mut TestButtonEvent)).x = cw * (col + 0.5) + 2.0;
            (*(ev as *mut TestButtonEvent)).y = ch * (row + 0.5) + 2.0;
            let out = check_vte_event_link(vte, ev, None);
            gdk_event_free(ev);
            out
        }
    }

    #[test]
    fn headless_vte_matches_glued_tilde_cyrillic_tail_cells() {
        extern "C" {
            fn vte_terminal_get_char_width(vte: *mut GtkWidget) -> std::ffi::c_long;
            fn vte_terminal_get_char_height(vte: *mut GtkWidget) -> std::ffi::c_long;
        }
        crate::proc_radar::tests::with_headless_gtk(|| unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            gtk_window_set_default_size(win, 900, 300);
            let vte = vte_terminal_new();
            apply_vte_style(vte);
            register_vte_url_matches(vte);
            gtk_container_add(win, vte);
            gtk_widget_show_all(win);
            crate::proc_radar::tests::pump_gtk_events();
            let feed = "отзыв.docx~/Development/ЦБО/отзыв_Иванов.docx\r\n[конспект](~/Development/Курс_(ЦУ)/неделя_1.md)\r\n";
            vte_terminal_feed(vte, feed.as_ptr() as *const c_char, feed.len() as isize);
            std::thread::sleep(std::time::Duration::from_millis(40));
            crate::proc_radar::tests::pump_gtk_events();

            let cw = vte_terminal_get_char_width(vte).max(8) as f64;
            let ch = vte_terminal_get_char_height(vte).max(14) as f64;
            let ev_win = find_vte_event_win(vte, cw, ch);
            assert!(!ev_win.is_null(), "expected VTE input child GdkWindow");
            let home = crate::paths::home_dir();
            assert_eq!(probe_vte_cell(vte, ev_win, 38.0, 0.0).as_deref(), Some(format!("{home}/Development/ЦБО/отзыв_Иванов.docx").as_str()));
            assert_eq!(probe_vte_cell(vte, ev_win, 2.0, 0.0), None);
            assert_eq!(probe_vte_cell(vte, ev_win, 38.0, 1.0).as_deref(), Some(format!("{home}/Development/Курс_(ЦУ)/неделя_1.md").as_str()));
            gtk_widget_destroy(win);
        });
    }

    extern "C" fn noop_click(_w: *mut GtkWidget, _d: *mut c_void) {}
    extern "C" fn noop_ctx(_w: *mut GtkWidget, _e: *const GdkEvent, _d: *mut c_void) -> c_int {
        0
    }

    fn assert_custom_title_preserves_agent_detection(s: &mut TermSession) {
        s.osc_title = "claude code".to_owned();
        s.proc_info.agent.clear();
        refresh_smart_title(s);
        assert_eq!(s.proc_info.agent, "Claude");
        assert_eq!(s.title, "Сборка релиза ЦУ");
        s.osc_title.clear();
        s.proc_info.agent.clear();
        s.remote_tmux = Some(("main".into(), "0".into(), "codex".into(), "/srv/app".into()));
        refresh_smart_title(s);
        assert_eq!(s.proc_info.agent, "Codex");
        assert_eq!(s.title, "Сборка релиза ЦУ");
        s.remote_tmux = None;
        s.proc_info.agent.clear();
    }

    fn build_headless_rename_session(root: *mut GtkWidget) -> TermSession {
        let sidebar = crate::sidebar::Sidebar::new(root, noop_click);
        let strip = crate::tab_strip::TabStrip::new(root, noop_click);
        let vte = unsafe { vte_terminal_new() };
        let scroll = crate::split_pane::wrap_vte_scroll(vte);
        let panes = TabPanes::new(scroll, vte);
        unsafe { gtk_box_pack_start(root, panes.root, 1, 1, 0) };
        new_headless_project_session(vte, scroll, panes, &sidebar, &strip)
    }

    fn new_headless_project_session(
        vte: *mut GtkWidget,
        scroll: *mut GtkWidget,
        panes: TabPanes,
        sidebar: &crate::sidebar::Sidebar,
        strip: &crate::tab_strip::TabStrip,
    ) -> TermSession {
        let now = Instant::now();
        let handlers = crate::tab_strip::TabHandlers {
            on_select: noop_click,
            on_close: noop_click,
            on_press: noop_ctx,
            on_motion: noop_ctx,
            on_release: noop_ctx,
        };
        TermSession {
            id: 1,
            base_title: "Проект · Новая".to_owned(),
            profile_cwd: "/home/user/Проект".to_owned(),
            title: "Проект · Новая".to_owned(),
            osc_title: String::new(),
            pinned_title: None,
            cwd: "/home/user/ДругаяПапка".to_owned(),
            category: SessionCategory::Project,
            vte,
            scroll_wrap: scroll,
            panes,
            sidebar_row: sidebar.append_tab(SessionCategory::Project, 1, noop_click, noop_ctx),
            top_tab: strip.append(1, SessionCategory::Project, handlers),
            child_pid: 0,
            state: WorkState::Idle,
            unread: false,
            last_output: now,
            work_started: now,
            work_started_at: None,
            last_turn_secs: None,
            last_notified: None,
            proc_info: ProcInfo::default(),
            tab: Tab::local("default"),
            remote_tmux: None,
            tmux_title_from_vte: false,
            telemetry: SessionTelemetry::default(),
        }
    }

    #[test]
    fn custom_rename_overrides_smart_title_and_resets_when_blank() {
        crate::proc_radar::tests::with_headless_gtk(|| unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            let root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
            gtk_container_add(win, root);
            let mut s = build_headless_rename_session(root);
            s.apply_rename("  Сборка релиза ЦУ  ");
            assert_eq!(s.pinned_title.as_deref(), Some("Сборка релиза ЦУ"));
            assert_eq!(s.title, "Сборка релиза ЦУ");
            assert_custom_title_preserves_agent_detection(&mut s);
            s.apply_rename("   ");
            assert_eq!(s.pinned_title, None);
            assert_eq!(s.title, "ДругаяПапка");
            gtk_widget_destroy(win);
        });
    }

    fn check_left_click_modifiers() {
        const LOCK_AND_MOD2: u32 = (1 << 1) | (1 << 4);
        for state in [0, LOCK_AND_MOD2, GDK_SHIFT_MASK, GDK_MOD1_MASK, GDK_CONTROL_MASK | GDK_MOD1_MASK] {
            assert!(!should_open_vte_link(1, state));
        }
        for state in [GDK_CONTROL_MASK, GDK_CONTROL_MASK | LOCK_AND_MOD2, GDK_CONTROL_MASK | GDK_SHIFT_MASK] {
            assert!(should_open_vte_link(1, state));
        }
        assert!(!should_open_vte_link(2, GDK_CONTROL_MASK));
        assert!(!should_open_vte_link(3, GDK_CONTROL_MASK));
    }

    #[test]
    fn left_click_link_predicate_requires_ctrl_and_keeps_drag_selection_free() {
        check_left_click_modifiers();
        crate::proc_radar::tests::with_headless_gtk(|| unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            let vte = vte_terminal_new();
            gtk_container_add(win, vte);
            assert_eq!(handle_vte_left_click(vte, ptr::null(), 0, ptr::null_mut()), 0);
            assert_eq!(handle_vte_left_click(vte, ptr::null(), GDK_CONTROL_MASK, ptr::null_mut()), 0);
            assert_eq!(on_vte_button_press(vte, ptr::null(), ptr::null_mut()), 0);
            gtk_widget_destroy(win);
        });
    }

    #[test]
    fn session_module_meets_grade_b_mi_target() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("session.rs"));
        assert!(mi >= 58.0, "session.rs MI {mi:.2} < 58.0");
    }
}
