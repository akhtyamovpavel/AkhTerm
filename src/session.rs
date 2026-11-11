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
use crate::tabs::{RemoteConnection, Tab};
use crate::theme::{apply_vte_colors, current_theme_mode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCategory {
    Project,
    Remote,
    Daemon,
}

impl SessionCategory {
    pub fn icon_name(self) -> &'static CStr {
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
    pub custom_title: bool,
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
    pub last_notified: Option<Instant>,
    pub proc_info: ProcInfo,
    pub tab: Tab,
    pub remote_tmux: Option<(String, String, String, String)>,
    pub tmux_title_from_vte: bool,
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

    pub fn has_vte(&self, vte: *mut GtkWidget) -> bool {
        self.vte == vte || self.panes.contains_vte(vte)
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
    r"file://[^\s\x22'<>()`│┃┆┊║]+",
    r"\b(localhost|127\.0\.0\.1|0\.0\.0\.0):[0-9]{2,5}(/[^\s\x22'<>()`│┃┆┊║]*)?",
    r"(~|/home/[a-zA-Z0-9_.-]+)/[^\s\x22'<>():`│┃┆┊║]+(?::[0-9]+){0,2}",
    r"(?<![^\s\x22'<>(\[{=`│┃┆┊║])(?:(?:[^\s\x22'<>():`│┃┆┊║/\[\]{};,]+/)+[^\s\x22'<>():`│┃┆┊║/\[\]{};,]+\.[a-zA-Z][a-zA-Z0-9]{0,9}(?::[0-9]+){0,2}|(?:[^\s\x22'<>():`│┃┆┊║/\[\]{};,]*[^\s\x22'<>():`│┃┆┊║/\[\]{};,.])\.[a-zA-Z][a-zA-Z0-9]{0,9}:[0-9]+(?::[0-9]+)?)",
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

thread_local! {
    static LINK_CWD_FN: Cell<Option<fn(*mut GtkWidget) -> LinkCwd>> = const { Cell::new(None) };
}

pub fn register_link_cwd_lookup(lookup: fn(*mut GtkWidget) -> LinkCwd) {
    LINK_CWD_FN.with(|slot| slot.set(Some(lookup)));
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

extern "C" fn on_vte_button_press(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    user_data: *mut c_void,
) -> c_int {
    unsafe {
        let mut button: u32 = 0;
        if gdk_event_get_event_type(event) != GDK_BUTTON_PRESS
            || gdk_event_get_button(event, &mut button) == 0
            || button != 1
        {
            return 0;
        }
        let cwd = resolve_vte_cwd(vte, user_data);
        if let Some(hlink) = take_c_string(vte_terminal_hyperlink_check_event(vte, event)) {
            if let Some(target) = resolve_link_target(&hlink, cwd.as_deref()) {
                open_external_link(&target);
                return 1;
            }
        }
        let mut tag: c_int = -1;
        if let Some(matched) = take_c_string(vte_terminal_match_check_event(vte, event, &mut tag)) {
            if let Some(target) = resolve_link_target(&matched, cwd.as_deref()) {
                open_external_link(&target);
                return 1;
            }
        }
    }
    0
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
    if s.custom_title {
        return;
    }
    update_detected_agent(s);
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
        SessionCategory::Daemon => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_panes() -> TabPanes {
        TabPanes { root: ptr::null_mut(), extra: Vec::new(), last_focus: ptr::null_mut() }
    }

    fn sample_local_session(base_title: &str, dir: &str) -> TermSession {
        let now = Instant::now();
        let cat = SessionCategory::Project;
        let tab = Tab::Local { profile: "work".to_owned(), cwd: dir.to_owned() };
        TermSession {
            id: 1, base_title: base_title.to_owned(), profile_cwd: dir.to_owned(),
            title: base_title.to_owned(), osc_title: String::new(), custom_title: false,
            cwd: dir.to_owned(), category: cat, vte: ptr::null_mut(), scroll_wrap: ptr::null_mut(),
            panes: empty_panes(), sidebar_row: SidebarRow::empty_for_test(cat),
            top_tab: TabView::empty_for_test(cat), child_pid: 0, state: WorkState::Idle,
            unread: false, last_output: now, work_started: now, last_notified: None,
            proc_info: ProcInfo::default(), tab, remote_tmux: None, tmux_title_from_vte: false,
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

    #[test]
    fn all_vte_url_patterns_compile_with_pcre2_multiline_utf() {
        extern "C" {
            fn vte_regex_substitute(
                regex: *mut VteRegex,
                subject: *const c_char,
                replacement: *const c_char,
                flags: u32,
                error: *mut *mut c_void,
            ) -> *mut c_char;
        }
        for pat in URL_PATTERNS {
            let pat_c = CString::new(*pat).unwrap();
            let vreg = unsafe {
                vte_regex_new_for_match(pat_c.as_ptr(), -1, PCRE2_MULTILINE_UTF, ptr::null_mut())
            };
            assert!(!vreg.is_null(), "failed to compile VTE regex: {pat}");
            unsafe { vte_regex_unref(vreg) };
        }
        let rel_pat = CString::new(*URL_PATTERNS.last().unwrap()).unwrap();
        let vreg = unsafe {
            vte_regex_new_for_match(rel_pat.as_ptr(), -1, PCRE2_MULTILINE_UTF, ptr::null_mut())
        };
        for (line, expected) in [
            ("  --> src/session.rs:241:5", "  --> <src/session.rs:241:5>"),
            ("at (./app/main.py:42:5)", "at (<./app/main.py:42:5>)"),
            ("see crates/core/lib.rs now", "see <crates/core/lib.rs> now"),
            ("ошибка в учёба/модуль.py:18:3!", "ошибка в <учёба/модуль.py:18:3>!"),
            ("version v0.2.1 on 127.0.0.1", "version v0.2.1 on 127.0.0.1"),
        ] {
            let subj = CString::new(line).unwrap();
            let out = unsafe {
                take_c_string(vte_regex_substitute(vreg, subj.as_ptr(), c"<$0>".as_ptr(), 0, ptr::null_mut()))
            };
            assert_eq!(out.as_deref(), Some(expected));
        }
        unsafe { vte_regex_unref(vreg) };
    }
}
