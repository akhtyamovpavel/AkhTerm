//! Agent approval prompt detection, next-attention tab cycling, and unfocused notifications.

use std::ffi::{c_uint, c_void, CStr};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::ffi::*;
use crate::session::{SessionCategory, TermSession, WorkState};
use crate::ui_text as text;

pub const RECENT_PROMPT_LINES: usize = 6;
pub const QUIET_SETTLE_DELAY: Duration = Duration::from_millis(2200);
pub const LONG_WORK_NOTIFY_MIN: Duration = Duration::from_secs(12);
pub const NOTIFY_COOLDOWN: Duration = Duration::from_secs(10);
pub const KEYCODE_U: u16 = 30;

const LOWER_PROMPTS: &[&str] = &[
    "[y/n]",
    "(y/n)",
    "[n/y]",
    "(n/y)",
    "[yes/no]",
    "(yes/no)",
    "do you want to proceed",
    "do you want to allow",
    "would you like to proceed",
    "yes, allow",
    "run command?",
    "run this command?",
    "waiting for approval",
    "ждёт апрува",
    "разрешить выполнение",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabAttention {
    pub id: usize,
    pub state: WorkState,
    pub unread: bool,
}

pub fn read_vte_visible_text(vte: *mut GtkWidget) -> String {
    if vte.is_null() {
        return String::new();
    }
    let raw = unsafe { vte_terminal_get_text_format(vte, VTE_FORMAT_TEXT) };
    if raw.is_null() {
        return String::new();
    }
    let out = unsafe { CStr::from_ptr(raw).to_string_lossy().into_owned() };
    unsafe { g_free(raw as *mut c_void) };
    out
}

fn is_frame_or_space(ch: char) -> bool {
    ch.is_whitespace() || "│┃┆┊║╭╮╰╯┌┐└┘├┤┬┴┼─━═".contains(ch)
}

fn clean_terminal_line(line: &str) -> &str {
    line.trim_matches(is_frame_or_space)
}

fn has_approve_word(line: &str) -> bool {
    line.match_indices("Approve").any(|(idx, word)| {
        let rest = &line[idx + word.len()..];
        !rest.starts_with(|c: char| c.is_ascii_alphabetic())
    })
}

fn line_needs_approval(line: &str) -> bool {
    if line.contains("Allow ") || has_approve_word(line) {
        return true;
    }
    let lower = line.to_lowercase();
    LOWER_PROMPTS.iter().any(|pat| lower.contains(pat))
}

pub fn classify_terminal_text(text: &str) -> WorkState {
    let needs = text
        .lines()
        .rev()
        .map(clean_terminal_line)
        .filter(|line| !line.is_empty())
        .take(RECENT_PROMPT_LINES)
        .any(line_needs_approval);
    if needs {
        WorkState::NeedsApproval
    } else {
        WorkState::Ready
    }
}

pub fn classify_vte_state(vte: *mut GtkWidget) -> WorkState {
    let visible = read_vte_visible_text(vte);
    classify_terminal_text(&visible)
}

pub fn is_jump_attention_shortcut(keyval: c_uint, keycode: u16) -> bool {
    matches!(char::from_u32(keyval), Some('U' | 'u')) || keycode == KEYCODE_U
}

fn find_cyclic_match(
    tabs: &[TabAttention],
    start: usize,
    active_id: usize,
    pred: fn(&TabAttention) -> bool,
) -> Option<usize> {
    let len = tabs.len();
    (0..len)
        .map(|off| &tabs[(start + off) % len])
        .find(|tab| tab.id != active_id && pred(tab))
        .map(|tab| tab.id)
}

fn is_approval_tab(tab: &TabAttention) -> bool {
    tab.state == WorkState::NeedsApproval
}

fn is_ready_or_unread_tab(tab: &TabAttention) -> bool {
    tab.unread || tab.state == WorkState::Ready
}

pub fn next_attention_tab(tabs: &[TabAttention], active_id: usize) -> Option<usize> {
    if tabs.is_empty() {
        return None;
    }
    let start = tabs
        .iter()
        .position(|tab| tab.id == active_id)
        .map_or(0, |idx| (idx + 1) % tabs.len());
    find_cyclic_match(tabs, start, active_id, is_approval_tab)
        .or_else(|| find_cyclic_match(tabs, start, active_id, is_ready_or_unread_tab))
}

pub fn find_next_session_attention(sessions: &[TermSession], active_id: usize) -> Option<usize> {
    let items: Vec<TabAttention> = sessions
        .iter()
        .map(|s| TabAttention { id: s.id, state: s.state, unread: s.unread })
        .collect();
    next_attention_tab(&items, active_id)
}

pub fn should_notify_background(
    window_active: bool,
    settled_state: WorkState,
    work_span: Duration,
) -> bool {
    !window_active
        && (settled_state == WorkState::NeedsApproval || work_span >= LONG_WORK_NOTIFY_MIN)
}

pub fn can_notify_session(last_notified: Option<Instant>, now: Instant) -> bool {
    last_notified.is_none_or(|prev| now.saturating_duration_since(prev) >= NOTIFY_COOLDOWN)
}

pub fn run_notify_command(program: &str, summary: &str, body: &str) -> bool {
    Command::new(program)
        .args(["-a", "AkhTerm", "-i", "utilities-terminal-symbolic", "--", summary, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|st| st.success())
}

pub fn spawn_desktop_notification(needs_approval: bool, tab_title: &str) {
    let summary_c = if needs_approval {
        text::NOTIFY_APPROVAL_TITLE
    } else {
        text::NOTIFY_READY_TITLE
    };
    let summary = summary_c.to_string_lossy().into_owned();
    let body = tab_title.to_owned();
    thread::spawn(move || {
        let _ = run_notify_command("notify-send", &summary, &body);
    });
}

pub fn sync_window_focus_urgency(win: *mut GtkWidget) -> bool {
    if win.is_null() {
        return true;
    }
    let active = unsafe { gtk_window_is_active(win) } != 0;
    if active && unsafe { gtk_window_get_urgency_hint(win) } != 0 {
        unsafe { gtk_window_set_urgency_hint(win, 0) };
    }
    active
}

fn apply_settled_state(session: &mut TermSession, classified: WorkState, active_id: usize) {
    if classified == WorkState::NeedsApproval {
        session.state = WorkState::NeedsApproval;
        session.unread = session.id != active_id;
    } else if session.id != active_id {
        session.state = WorkState::Ready;
        session.unread = true;
    } else {
        session.state = WorkState::Idle;
        session.unread = false;
    }
}

fn maybe_notify_settled(
    session: &mut TermSession,
    now: Instant,
    classified: WorkState,
    work_span: Duration,
    win: *mut GtkWidget,
    window_active: bool,
) {
    if !should_notify_background(window_active, classified, work_span) {
        return;
    }
    if !win.is_null() {
        unsafe { gtk_window_set_urgency_hint(win, 1) };
    }
    if can_notify_session(session.last_notified, now) {
        session.last_notified = Some(now);
        spawn_desktop_notification(classified.needs_approval(), &session.title);
    }
}

fn record_settled_turn(session: &mut TermSession, work_span: Duration) {
    let secs = work_span.as_secs();
    if secs >= 1 {
        session.last_turn_secs = Some(secs);
    } else if matches!(session.state, WorkState::Ready | WorkState::NeedsApproval)
        && session.last_turn_secs.is_none()
    {
        session.last_turn_secs = Some(1);
    }
}

pub fn settle_working_session(
    session: &mut TermSession,
    now: Instant,
    active_id: usize,
    win: *mut GtkWidget,
    window_active: bool,
) {
    if session.category == SessionCategory::Daemon || session.state != WorkState::Working {
        return;
    }
    if now.saturating_duration_since(session.last_output) <= QUIET_SETTLE_DELAY {
        return;
    }
    let started = session.work_started_at.take().unwrap_or(session.work_started);
    let work_span = session.last_output.saturating_duration_since(started);
    let classified = classify_vte_state(session.active_vte());
    apply_settled_state(session, classified, active_id);
    record_settled_turn(session, work_span);
    maybe_notify_settled(session, now, classified, work_span, win, window_active);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_char;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Child;
    use std::ptr;

    #[test]
    fn classifies_approval_prompts_across_agents_and_cli_tools() {
        let samples = [
            "Continue installation? [Y/n]",
            "Overwrite config? [y/N]",
            "Proceed with changes? [y/n]",
            "Delete temporary branch? (y/n)",
            "╭────────────────────────────╮\n│ Do you want to proceed?    │\n│ ❯ 1. Yes                   │\n│   2. Yes, allow all edits  │\n╰────────────────────────────╯",
            "Allow Bash(cargo test --offline)?",
            "Tool call requires review\n[1] Approve  [2] Reject",
            "Agent wants to edit src/main.rs\nRun command?",
        ];
        for sample in samples {
            assert_eq!(
                classify_terminal_text(sample),
                WorkState::NeedsApproval,
                "expected NeedsApproval for: {sample}"
            );
        }
    }

    #[test]
    fn ignores_ordinary_output_false_positives_and_scrolled_prompts() {
        let ready_samples = [
            "",
            "   \n\t\n",
            "test result: ok. 12 passed; 0 failed; finished in 1.10s\nuser@host:~/proj$",
            "On branch feat/issue-1-approvals\nnothing to commit, working tree clean",
            "warning: #[allow(dead_code)] on module\nPR Approved by reviewer",
            "Continue? [y/n]\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\ndone$",
        ];
        for sample in ready_samples {
            assert_eq!(
                classify_terminal_text(sample),
                WorkState::Ready,
                "expected Ready for: {sample}"
            );
        }
    }

    #[test]
    fn cycles_needs_approval_tabs_first_then_ready_tabs() {
        let tabs = [
            TabAttention { id: 10, state: WorkState::Idle, unread: false },
            TabAttention { id: 20, state: WorkState::Ready, unread: true },
            TabAttention { id: 30, state: WorkState::NeedsApproval, unread: true },
            TabAttention { id: 40, state: WorkState::NeedsApproval, unread: false },
        ];
        assert_eq!(next_attention_tab(&tabs, 10), Some(30));
        assert_eq!(next_attention_tab(&tabs, 30), Some(40));
        assert_eq!(next_attention_tab(&tabs, 40), Some(30));

        let ready_only = [
            TabAttention { id: 10, state: WorkState::Ready, unread: true },
            TabAttention { id: 20, state: WorkState::Idle, unread: false },
            TabAttention { id: 30, state: WorkState::Idle, unread: true },
        ];
        assert_eq!(next_attention_tab(&ready_only, 20), Some(30));
        assert_eq!(next_attention_tab(&ready_only, 30), Some(10));
    }

    #[test]
    fn returns_none_when_no_other_tab_needs_attention() {
        let idle = [
            TabAttention { id: 1, state: WorkState::Idle, unread: false },
            TabAttention { id: 2, state: WorkState::Working, unread: false },
        ];
        assert_eq!(next_attention_tab(&[], 1), None);
        assert_eq!(next_attention_tab(&idle, 1), None);
        let single = [TabAttention { id: 7, state: WorkState::NeedsApproval, unread: true }];
        assert_eq!(next_attention_tab(&single, 7), None);
    }

    #[test]
    fn matches_ctrl_shift_u_on_latin_and_cyrillic_keycodes() {
        assert!(is_jump_attention_shortcut(u32::from(b'U'), 0));
        assert!(is_jump_attention_shortcut(u32::from(b'u'), 0));
        assert!(is_jump_attention_shortcut(0x06e7, KEYCODE_U));
        assert!(!is_jump_attention_shortcut(u32::from(b'T'), 28));
        assert!(!is_jump_attention_shortcut(u32::from(b'W'), 25));
    }

    #[test]
    fn background_notification_checks_focus_duration_and_cooldown() {
        let short = Duration::from_secs(3);
        let long = Duration::from_secs(14);
        assert!(should_notify_background(false, WorkState::NeedsApproval, short));
        assert!(should_notify_background(false, WorkState::Ready, long));
        assert!(!should_notify_background(false, WorkState::Ready, short));
        assert!(!should_notify_background(true, WorkState::NeedsApproval, long));

        let t0 = Instant::now();
        assert!(can_notify_session(None, t0));
        assert!(!can_notify_session(Some(t0), t0 + Duration::from_secs(4)));
        assert!(can_notify_session(Some(t0), t0 + Duration::from_secs(11)));
        assert!(!run_notify_command("/nonexistent/akhterm-missing-notify-bin", "t", "b"));
    }

    #[test]
    fn maintainability_index_meets_module_target() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("approval.rs"));
        assert!(mi >= 58.0, "approval.rs MI {mi:.2} must stay >= 58.0");
    }

    fn clean_broadway_dir(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
        let _ = fs::remove_dir(dir);
    }

    fn spawn_isolated_broadway(dir: &Path) -> Option<Child> {
        let http_sock = dir.join("http.sock");
        Command::new("broadwayd")
            .arg("-u")
            .arg(&http_sock)
            .arg(":11")
            .env("XDG_RUNTIME_DIR", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }

    fn pump_events() {
        for _ in 0..60 {
            if unsafe { gtk_events_pending() } == 0 {
                break;
            }
            unsafe { gtk_main_iteration_do(0) };
        }
    }

    fn feed_and_pump(vte: *mut GtkWidget, payload: &str) {
        unsafe {
            vte_terminal_feed(vte, payload.as_ptr() as *const c_char, payload.len() as isize);
        }
        thread::sleep(Duration::from_millis(30));
        pump_events();
    }

    fn run_broadway_vte_scenario(dir: &Path) {
        std::env::set_var("XDG_RUNTIME_DIR", dir);
        std::env::set_var("GDK_BACKEND", "broadway");
        std::env::set_var("BROADWAY_DISPLAY", ":11");
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == 0 {
            return;
        }
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let vte = unsafe { vte_terminal_new() };
        unsafe {
            gtk_window_set_default_size(win, 800, 480);
            gtk_container_add(win, vte);
            gtk_widget_show_all(win);
        }
        pump_events();
        feed_and_pump(vte, "Проверка пакета ЦУ\r\nDo you want to proceed? [Y/n]");
        let text = read_vte_visible_text(vte);
        assert!(text.contains("Проверка пакета ЦУ"), "visible text: {text:?}");
        assert_eq!(classify_vte_state(vte), WorkState::NeedsApproval);
        feed_and_pump(vte, "\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\nГотово!\r\n");
        assert_eq!(classify_vte_state(vte), WorkState::Ready);
        unsafe { gtk_widget_destroy(win) };
    }

    fn execute_broadway_child() {
        let dir: PathBuf = std::env::temp_dir().join(format!("akhterm_app_bway_{}", std::process::id()));
        clean_broadway_dir(&dir);
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Some(mut child) = spawn_isolated_broadway(&dir) {
            let sock = dir.join("broadway12.socket");
            let deadline = Instant::now() + Duration::from_secs(2);
            while !sock.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            run_broadway_vte_scenario(&dir);
            let _ = child.kill();
            let _ = child.wait();
        }
        clean_broadway_dir(&dir);
    }

    #[test]
    fn headless_broadway_vte_reads_and_classifies_prompts() {
        if std::env::var_os("AKHTERM_APPROVAL_BWAY_CHILD").is_some() {
            execute_broadway_child();
            return;
        }
        let exe = std::env::current_exe().expect("test binary path");
        let status = Command::new(exe)
            .args(["--exact", "approval::tests::headless_broadway_vte_reads_and_classifies_prompts", "--nocapture"])
            .env("AKHTERM_APPROVAL_BWAY_CHILD", "1")
            .status()
            .expect("spawn isolated broadway test");
        assert!(status.success());
    }
}
