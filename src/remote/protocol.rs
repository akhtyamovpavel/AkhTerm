//! Frozen Remote API v1 DTOs, JSON serializers, approval/TUI-key byte mapping, and QR URI builder.

use crate::sys::json_lite::{format_compact, JsonVal};

pub const API_VERSION: u32 = 1;
pub const MAX_TAIL_LINES: usize = 200;
pub const DEFAULT_TAIL_LINES: usize = 100;
pub const MAX_SEND_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalKind {
    YesNo,
    TuiMenu,
    EnterConfirm,
}

impl ApprovalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::YesNo => "yes_no",
            Self::TuiMenu => "tui_menu",
            Self::EnterConfirm => "enter_confirm",
        }
    }

    pub fn detect(approval_line: Option<&str>) -> Self {
        let lower = approval_line.unwrap_or("").to_ascii_lowercase();
        if has_any(&lower, &["y/n", "n/y", "yes/no", "[y]"]) {
            Self::YesNo
        } else if has_any(&lower, &["yes", "allow", "approve", "confirm"]) {
            Self::TuiMenu
        } else {
            Self::EnterConfirm
        }
    }
}

fn has_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApproveAction {
    Yes,
    YesSave,
    No,
    Enter,
}

impl ApproveAction {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "yes" => Some(Self::Yes),
            "yes_save" => Some(Self::YesSave),
            "no" => Some(Self::No),
            "enter" => Some(Self::Enter),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::YesSave => "yes_save",
            Self::No => "no",
            Self::Enter => "enter",
        }
    }

    pub fn to_bytes(self, kind: Option<ApprovalKind>) -> &'static [u8] {
        let is_yn = kind == Some(ApprovalKind::YesNo);
        match self {
            Self::Enter => b"\r",
            Self::Yes => if is_yn { b"y\r" } else { b"\r" },
            Self::YesSave => b"\x1b[B\r",
            Self::No => if is_yn { b"n\r" } else { b"\x1b" },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteKey {
    Enter,
    Esc,
    Up,
    Down,
    Tab,
    CtrlC,
    Digit1,
    Digit2,
    Digit3,
}

impl RemoteKey {
    pub fn parse(raw: &str) -> Option<Self> {
        const MAP: &[(&str, RemoteKey)] = &[
            ("enter", RemoteKey::Enter), ("esc", RemoteKey::Esc), ("up", RemoteKey::Up),
            ("down", RemoteKey::Down), ("tab", RemoteKey::Tab), ("ctrl_c", RemoteKey::CtrlC),
            ("1", RemoteKey::Digit1), ("2", RemoteKey::Digit2), ("3", RemoteKey::Digit3),
        ];
        let key = raw.trim().to_ascii_lowercase();
        MAP.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enter => "enter", Self::Esc => "esc", Self::Up => "up",
            Self::Down => "down", Self::Tab => "tab", Self::CtrlC => "ctrl_c",
            Self::Digit1 => "1", Self::Digit2 => "2", Self::Digit3 => "3",
        }
    }

    pub fn to_bytes(self) -> &'static [u8] {
        match self {
            Self::Enter => b"\r", Self::Esc => b"\x1b", Self::Up => b"\x1b[A",
            Self::Down => b"\x1b[B", Self::Tab => b"\t", Self::CtrlC => b"\x03",
            Self::Digit1 => b"1", Self::Digit2 => b"2", Self::Digit3 => b"3",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePaneSummary {
    pub pane_id: String,
    pub tab_id: usize,
    pub tab_title: String,
    pub active_tab: bool,
    pub focused_pane: bool,
    pub agent: String,
    pub repo_name: String,
    pub branch_badge: String,
    pub is_worktree: bool,
    pub ssh_host: Option<String>,
    pub state: String,
    pub elapsed_secs: Option<u64>,
    pub context_badge: Option<String>,
    pub unread: bool,
    pub preview_lines: Vec<String>,
    pub approval_snippet: Option<String>,
    pub approval_kind: Option<ApprovalKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteStatusResponse {
    pub version: String,
    pub host_name: String,
    pub desktop_focused: bool,
    pub mute_when_focused: bool,
    pub active_tab_id: usize,
    pub panes: Vec<RemotePaneSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneTailResponse {
    pub pane_id: String,
    pub seq: u64,
    pub state: String,
    pub approval_snippet: Option<String>,
    pub approval_kind: Option<ApprovalKind>,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertEvent {
    pub event_id: String,
    pub pane_id: String,
    pub tab_title: String,
    pub agent: String,
    pub repo_name: String,
    pub state: String,
    pub title: String,
    pub body: String,
    pub approval_kind: Option<ApprovalKind>,
    pub desktop_focused: bool,
    pub should_notify: bool,
}

fn j_str(s: &str) -> JsonVal {
    JsonVal::Str(s.to_owned())
}

fn j_opt_str(opt: &Option<String>) -> JsonVal {
    opt.as_deref().map_or(JsonVal::Null, j_str)
}

fn j_opt_u64(opt: Option<u64>) -> JsonVal {
    opt.map_or(JsonVal::Null, |n| JsonVal::Number(n.to_string()))
}

fn j_opt_kind(k: Option<ApprovalKind>) -> JsonVal {
    k.map_or(JsonVal::Null, |v| j_str(v.as_str()))
}

fn j_str_array(items: &[String]) -> JsonVal {
    JsonVal::Array(items.iter().map(|s| j_str(s)).collect())
}

fn push_pane_identity(out: &mut Vec<(String, JsonVal)>, p: &RemotePaneSummary) {
    out.push(("pane_id".into(), j_str(&p.pane_id)));
    out.push(("tab_id".into(), JsonVal::Number(p.tab_id.to_string())));
    out.push(("tab_title".into(), j_str(&p.tab_title)));
    out.push(("active_tab".into(), JsonVal::Bool(p.active_tab)));
    out.push(("focused_pane".into(), JsonVal::Bool(p.focused_pane)));
    out.push(("agent".into(), j_str(&p.agent)));
    out.push(("repo_name".into(), j_str(&p.repo_name)));
    out.push(("branch_badge".into(), j_str(&p.branch_badge)));
    out.push(("is_worktree".into(), JsonVal::Bool(p.is_worktree)));
}

fn push_pane_runtime(out: &mut Vec<(String, JsonVal)>, p: &RemotePaneSummary) {
    out.push(("ssh_host".into(), j_opt_str(&p.ssh_host)));
    out.push(("state".into(), j_str(&p.state)));
    out.push(("elapsed_secs".into(), j_opt_u64(p.elapsed_secs)));
    out.push(("context_badge".into(), j_opt_str(&p.context_badge)));
    out.push(("unread".into(), JsonVal::Bool(p.unread)));
    out.push(("preview_lines".into(), j_str_array(&p.preview_lines)));
    out.push(("approval_snippet".into(), j_opt_str(&p.approval_snippet)));
    out.push(("approval_kind".into(), j_opt_kind(p.approval_kind)));
}

pub fn pane_summary_to_json(p: &RemotePaneSummary) -> JsonVal {
    let mut fields = Vec::with_capacity(17);
    push_pane_identity(&mut fields, p);
    push_pane_runtime(&mut fields, p);
    JsonVal::Object(fields)
}

pub fn encode_status_json(s: &RemoteStatusResponse) -> String {
    let panes = JsonVal::Array(s.panes.iter().map(pane_summary_to_json).collect());
    let obj = JsonVal::Object(vec![
        ("api_version".into(), JsonVal::Number(API_VERSION.to_string())),
        ("version".into(), j_str(&s.version)),
        ("host_name".into(), j_str(&s.host_name)),
        ("desktop_focused".into(), JsonVal::Bool(s.desktop_focused)),
        ("mute_when_focused".into(), JsonVal::Bool(s.mute_when_focused)),
        ("active_tab_id".into(), JsonVal::Number(s.active_tab_id.to_string())),
        ("panes".into(), panes),
    ]);
    format_compact(&obj)
}

pub fn encode_tail_json(t: &PaneTailResponse) -> String {
    let obj = JsonVal::Object(vec![
        ("pane_id".into(), j_str(&t.pane_id)),
        ("seq".into(), JsonVal::Number(t.seq.to_string())),
        ("state".into(), j_str(&t.state)),
        ("approval_snippet".into(), j_opt_str(&t.approval_snippet)),
        ("approval_kind".into(), j_opt_kind(t.approval_kind)),
        ("lines".into(), j_str_array(&t.lines)),
    ]);
    format_compact(&obj)
}

pub fn encode_alert_json(a: &AlertEvent) -> String {
    let obj = JsonVal::Object(vec![
        ("event_id".into(), j_str(&a.event_id)),
        ("pane_id".into(), j_str(&a.pane_id)),
        ("tab_title".into(), j_str(&a.tab_title)),
        ("agent".into(), j_str(&a.agent)),
        ("repo_name".into(), j_str(&a.repo_name)),
        ("state".into(), j_str(&a.state)),
        ("title".into(), j_str(&a.title)),
        ("body".into(), j_str(&a.body)),
        ("approval_kind".into(), j_opt_kind(a.approval_kind)),
        ("desktop_focused".into(), JsonVal::Bool(a.desktop_focused)),
        ("should_notify".into(), JsonVal::Bool(a.should_notify)),
    ]);
    format_compact(&obj)
}

pub fn encode_error_json(msg: &str) -> String {
    format_compact(&JsonVal::Object(vec![("error".into(), j_str(msg))]))
}

pub fn percent_encode_query(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for &b in raw.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn build_qr_pair_url(bind_ip: &str, port: u16, code: &str, host_name: &str) -> String {
    format!(
        "http://{bind_ip}:{port}/?pair={code}&host={}&v={API_VERSION}",
        percent_encode_query(host_name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::json_lite::parse_json;

    fn sample_pane() -> RemotePaneSummary {
        RemotePaneSummary {
            pane_id: "1".into(), tab_id: 1, tab_title: "Проект".into(), active_tab: true,
            focused_pane: true, agent: "Claude Code".into(), repo_name: "akhterm-rs".into(),
            branch_badge: "main".into(), is_worktree: false, ssh_host: None, state: "waiting".into(),
            elapsed_secs: Some(42), context_badge: Some("ctx 40k · cache 80%".into()),
            unread: true, preview_lines: vec!["Run test?".into()],
            approval_snippet: Some("Run test?".into()), approval_kind: Some(ApprovalKind::TuiMenu),
        }
    }

    #[test]
    fn status_and_tail_encode_match_api_v1_contract() {
        let status = RemoteStatusResponse {
            version: "1.5.1".into(), host_name: "pavel-laptop".into(), desktop_focused: false,
            mute_when_focused: true, active_tab_id: 1, panes: vec![sample_pane()],
        };
        let parsed = parse_json(&encode_status_json(&status)).unwrap();
        assert_eq!(parsed.get("api_version").and_then(JsonVal::as_u64), Some(1));
        let pane = &parsed.get("panes").unwrap().as_array().unwrap()[0];
        assert_eq!(pane.get("tab_title").and_then(JsonVal::as_str), Some("Проект"));
        assert_eq!(pane.get("approval_kind").and_then(JsonVal::as_str), Some("tui_menu"));
    }

    #[test]
    fn key_and_approve_byte_mappings_are_exact() {
        assert_eq!(RemoteKey::parse("ctrl_c").unwrap().to_bytes(), b"\x03");
        assert_eq!(RemoteKey::parse("up").unwrap().to_bytes(), b"\x1b[A");
        assert_eq!(RemoteKey::parse("down").unwrap().to_bytes(), b"\x1b[B");
        assert_eq!(ApproveAction::Yes.to_bytes(Some(ApprovalKind::YesNo)), b"y\r");
        assert_eq!(ApproveAction::Yes.to_bytes(Some(ApprovalKind::TuiMenu)), b"\r");
        assert_eq!(ApproveAction::No.to_bytes(Some(ApprovalKind::YesNo)), b"n\r");
        assert_eq!(ApproveAction::No.to_bytes(Some(ApprovalKind::TuiMenu)), b"\x1b");
    }

    #[test]
    fn qr_pair_url_encodes_cyrillic_hostname() {
        let u = build_qr_pair_url("192.168.1.10", 7474, "abcd", "ноут павла");
        assert!(u.starts_with("http://192.168.1.10:7474/?pair=abcd&host="));
        assert!(u.ends_with("&v=1") && !u.contains(' '));
    }
}
