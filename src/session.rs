//! VTE-2.91 terminal widget creation, clickable link matching, and session metadata.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::time::Instant;

use crate::ffi::*;
use crate::paths::expand_home;
use crate::proc_radar::{foreground_cwd, open_external_link, ProcInfo};
use crate::sidebar::SidebarRow;
use crate::tab_strip::TabView;
use crate::tabs::{RemoteConnection, Tab};
use crate::theme::{TERM_BG, TERM_CURSOR, TERM_FG, TERM_PALETTE};

#[derive(Clone, Copy, PartialEq, Eq)]
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

    pub fn snapshot_tab(&mut self) -> Tab {
        if let Tab::Local { cwd, .. } = &mut self.tab {
            if let Some(current) = foreground_cwd(self.child_pid) {
                self.cwd.clone_from(&current);
                *cwd = current;
            } else if !self.cwd.is_empty() {
                cwd.clone_from(&self.cwd);
            }
        }
        self.tab.clone()
    }
}

const URL_PATTERNS: &[&str] = &[
    r"https?://[^\s\x22'<>()`│┃┆┊║]+",
    r"file://[^\s\x22'<>()`│┃┆┊║]+",
    r"\b(localhost|127\.0\.0\.1|0\.0\.0\.0):[0-9]{2,5}(/[^\s\x22'<>()`│┃┆┊║]*)?",
    r"(~|/home/[a-zA-Z0-9_.-]+)/[^\s\x22'<>():`│┃┆┊║]+",
];

extern "C" fn on_vte_button_press(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    _user_data: *mut c_void,
) -> c_int {
    unsafe {
        if gdk_event_get_event_type(event) != GDK_BUTTON_PRESS {
            return 0;
        }
        let mut button: u32 = 0;
        if gdk_event_get_button(event, &mut button) == 0 || button != 1 {
            return 0;
        }

        let hlink = vte_terminal_hyperlink_check_event(vte, event);
        if !hlink.is_null() {
            let s = CStr::from_ptr(hlink).to_string_lossy().into_owned();
            g_free(hlink as *mut c_void);
            open_external_link(&s);
            return 1;
        }

        let mut tag: c_int = -1;
        let matched = vte_terminal_match_check_event(vte, event, &mut tag);
        if !matched.is_null() {
            let s = CStr::from_ptr(matched).to_string_lossy().into_owned();
            g_free(matched as *mut c_void);
            open_external_link(&s);
            return 1;
        }
    }
    0
}

pub fn create_vte_widget(cwd: &str, argv_strs: &[&str]) -> (*mut GtkWidget, i32) {
    unsafe {
        let vte = vte_terminal_new();
        vte_terminal_set_scrollback_lines(vte, 15_000);
        vte_terminal_set_allow_hyperlink(vte, 1);
        vte_terminal_set_colors(vte, &TERM_FG, &TERM_BG, TERM_PALETTE.as_ptr(), 16);
        vte_terminal_set_color_cursor(vte, &TERM_CURSOR);

        if let Ok(fstr) = CString::new("DejaVu Sans Mono 10.5") {
            let desc = pango_font_description_from_string(fstr.as_ptr());
            if !desc.is_null() {
                vte_terminal_set_font(vte, desc);
                pango_font_description_free(desc);
            }
        }

        let pointer_c = CString::new("pointer").unwrap();
        for pat in URL_PATTERNS {
            if let Ok(pat_c) = CString::new(*pat) {
                let vreg = vte_regex_new_for_match(
                    pat_c.as_ptr(),
                    -1,
                    PCRE2_MULTILINE_UTF,
                    ptr::null_mut(),
                );
                if !vreg.is_null() {
                    let tag = vte_terminal_match_add_regex(vte, vreg, 0);
                    if tag >= 0 {
                        vte_terminal_match_set_cursor_name(vte, tag, pointer_c.as_ptr());
                    }
                    vte_regex_unref(vreg);
                }
            }
        }

        let sig_btn = CString::new("button-press-event").unwrap();
        g_signal_connect_data(
            vte,
            sig_btn.as_ptr(),
            on_vte_button_press as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );

        let c_cwd = CString::new(cwd).unwrap_or_else(|_| CString::new("/tmp").unwrap());
        let c_args: Vec<CString> = argv_strs
            .iter()
            .filter_map(|s| CString::new(*s).ok())
            .collect();
        let mut ptrs: Vec<*const c_char> = c_args.iter().map(|s| s.as_ptr()).collect();
        ptrs.push(ptr::null());

        let mut pid: i32 = 0;
        let spawned = vte_terminal_spawn_sync(
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
        );
        if spawned == 0 {
            pid = 0;
        }

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

pub fn refresh_smart_title(s: &mut TermSession) {
    if s.custom_title {
        return;
    }
    update_detected_agent(s);
    let current_cwd = if !s.proc_info.cwd.is_empty() {
        s.proc_info.cwd.as_str()
    } else {
        s.cwd.as_str()
    };

    match s.category {
        SessionCategory::Remote => {
            let host = s.remote().map_or("SSH", |remote| remote.host.as_str());
            if let Some((ref sess, ref win, _, ref path)) = s.remote_tmux {
                let folder = folder_name(path);
                s.title = format_remote_title(host, sess, win, folder, &s.proc_info.agent);
            }
        }
        SessionCategory::Project => {
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
}
