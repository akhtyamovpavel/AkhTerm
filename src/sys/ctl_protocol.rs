//! Lightweight zero-dependency JSON-line control protocol for AkhTerm IPC.

use crate::sys::json_lite::{format_compact, parse_json, JsonVal};

#[allow(unused_imports)]
pub use crate::sys::json_lite::{escape_json_str, quote_json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookState {
    Working,
    Waiting,
    Done,
    Error,
}

impl HookState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Error => "error",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "working" => Some(Self::Working),
            "waiting" => Some(Self::Waiting),
            "done" => Some(Self::Done),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CtlRequest {
    Status,
    Focus {
        pane_id: String,
    },
    Notify {
        pane_id: String,
        state: HookState,
        title: Option<String>,
        message: Option<String>,
        desktop: bool,
    },
    Send {
        pane_id: String,
        text: String,
        enter: bool,
    },
    OpenDoc {
        pane_id: Option<String>,
        path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneStatusItem {
    pub pane_id: String,
    pub tab_id: usize,
    pub tab_title: String,
    pub active_tab: bool,
    pub focused_pane: bool,
    pub agent: String,
    pub state: String,
    pub hook_state: Option<String>,
    pub hook_title: Option<String>,
    pub hook_message: Option<String>,
    pub cwd: String,
    pub pid: i32,
    pub ssh_host: Option<String>,
    pub unread: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CtlResponse {
    Ok {
        message: Option<String>,
    },
    Status {
        version: String,
        pid: u32,
        socket: String,
        active_tab: usize,
        panes: Vec<PaneStatusItem>,
    },
    Error {
        message: String,
    },
}

pub fn parse_pane_id_num(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    let digits = trimmed
        .strip_prefix("pane-")
        .or_else(|| trimmed.strip_prefix('p'))
        .unwrap_or(trimmed);
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok().filter(|&id| id > 0)
}

fn j_str(s: &str) -> JsonVal {
    JsonVal::Str(s.to_owned())
}

fn j_opt_str(opt: &Option<String>) -> JsonVal {
    match opt {
        Some(s) => JsonVal::Str(s.clone()),
        None => JsonVal::Null,
    }
}

fn j_num(n: impl ToString) -> JsonVal {
    JsonVal::Number(n.to_string())
}

fn field_str(obj: &JsonVal, key: &str) -> Result<String, String> {
    match obj.get(key) {
        Some(JsonVal::Str(s)) => Ok(s.clone()),
        Some(JsonVal::Number(n)) => Ok(n.clone()),
        _ => Err(format!("missing string field '{key}'")),
    }
}

fn field_opt_str(obj: &JsonVal, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(JsonVal::Str(s)) if !s.is_empty() => Some(s.clone()),
        Some(JsonVal::Number(n)) if !n.is_empty() => Some(n.clone()),
        _ => None,
    }
}

fn field_bool(obj: &JsonVal, key: &str) -> bool {
    obj.get(key).and_then(JsonVal::as_bool).unwrap_or(false)
}

fn notify_to_json(
    pane_id: &str,
    state: HookState,
    title: &Option<String>,
    message: &Option<String>,
    desktop: bool,
) -> JsonVal {
    JsonVal::Object(vec![
        ("cmd".into(), j_str("notify")), ("pane_id".into(), j_str(pane_id)),
        ("state".into(), j_str(state.as_str())), ("title".into(), j_opt_str(title)),
        ("message".into(), j_opt_str(message)), ("desktop".into(), JsonVal::Bool(desktop)),
    ])
}

fn send_to_json(pane_id: &str, text: &str, enter: bool) -> JsonVal {
    JsonVal::Object(vec![
        ("cmd".into(), j_str("send")), ("pane_id".into(), j_str(pane_id)),
        ("text".into(), j_str(text)), ("enter".into(), JsonVal::Bool(enter)),
    ])
}

fn open_doc_to_json(pane_id: &Option<String>, path: &str) -> JsonVal {
    JsonVal::Object(vec![
        ("cmd".into(), j_str("open_doc")), ("pane_id".into(), j_opt_str(pane_id)),
        ("path".into(), j_str(path)),
    ])
}

fn request_to_json(req: &CtlRequest) -> JsonVal {
    match req {
        CtlRequest::Status => JsonVal::Object(vec![("cmd".into(), j_str("status"))]),
        CtlRequest::Focus { pane_id } => {
            JsonVal::Object(vec![("cmd".into(), j_str("focus")), ("pane_id".into(), j_str(pane_id))])
        }
        CtlRequest::Notify { pane_id, state, title, message, desktop } => {
            notify_to_json(pane_id, *state, title, message, *desktop)
        }
        CtlRequest::Send { pane_id, text, enter } => send_to_json(pane_id, text, *enter),
        CtlRequest::OpenDoc { pane_id, path } => open_doc_to_json(pane_id, path),
    }
}

pub fn encode_request(req: &CtlRequest) -> String {
    format_compact(&request_to_json(req))
}

fn decode_send_req(val: &JsonVal) -> Result<CtlRequest, String> {
    Ok(CtlRequest::Send {
        pane_id: field_str(val, "pane_id")?,
        text: field_str(val, "text")?,
        enter: field_bool(val, "enter"),
    })
}

fn decode_open_doc_req(val: &JsonVal) -> Result<CtlRequest, String> {
    Ok(CtlRequest::OpenDoc { pane_id: field_opt_str(val, "pane_id"), path: field_str(val, "path")? })
}

pub fn decode_request(line: &str) -> Result<CtlRequest, String> {
    let val = parse_json(line.trim())?;
    match field_str(&val, "cmd")?.as_str() {
        "status" => Ok(CtlRequest::Status),
        "focus" => Ok(CtlRequest::Focus { pane_id: field_str(&val, "pane_id")? }),
        "notify" => decode_notify_req(&val),
        "send" => decode_send_req(&val),
        "open_doc" | "open-doc" => decode_open_doc_req(&val),
        other => Err(format!("unknown command '{other}'")),
    }
}

fn decode_notify_req(val: &JsonVal) -> Result<CtlRequest, String> {
    let raw_state = field_str(val, "state")?;
    let state = HookState::parse(&raw_state)
        .ok_or_else(|| format!("invalid hook state '{raw_state}'"))?;
    Ok(CtlRequest::Notify {
        pane_id: field_str(val, "pane_id")?,
        state,
        title: field_opt_str(val, "title"),
        message: field_opt_str(val, "message"),
        desktop: field_bool(val, "desktop"),
    })
}

fn pane_to_json(p: &PaneStatusItem) -> JsonVal {
    JsonVal::Object(vec![
        ("pane_id".into(), j_str(&p.pane_id)), ("tab_id".into(), j_num(p.tab_id)),
        ("tab_title".into(), j_str(&p.tab_title)), ("active_tab".into(), JsonVal::Bool(p.active_tab)),
        ("focused_pane".into(), JsonVal::Bool(p.focused_pane)), ("agent".into(), j_str(&p.agent)),
        ("state".into(), j_str(&p.state)), ("hook_state".into(), j_opt_str(&p.hook_state)),
        ("hook_title".into(), j_opt_str(&p.hook_title)), ("hook_message".into(), j_opt_str(&p.hook_message)),
        ("cwd".into(), j_str(&p.cwd)), ("pid".into(), j_num(p.pid)),
        ("ssh_host".into(), j_opt_str(&p.ssh_host)), ("unread".into(), JsonVal::Bool(p.unread)),
    ])
}

fn pane_from_json(val: &JsonVal) -> Result<PaneStatusItem, String> {
    Ok(PaneStatusItem {
        pane_id: field_str(val, "pane_id")?,
        tab_id: val.get("tab_id").and_then(JsonVal::as_u64).unwrap_or(0) as usize,
        tab_title: field_str(val, "tab_title").unwrap_or_default(),
        active_tab: field_bool(val, "active_tab"), focused_pane: field_bool(val, "focused_pane"),
        agent: field_str(val, "agent").unwrap_or_default(), state: field_str(val, "state").unwrap_or_default(),
        hook_state: field_opt_str(val, "hook_state"), hook_title: field_opt_str(val, "hook_title"),
        hook_message: field_opt_str(val, "hook_message"), cwd: field_str(val, "cwd").unwrap_or_default(),
        pid: val.get("pid").and_then(JsonVal::as_i64).unwrap_or(0) as i32,
        ssh_host: field_opt_str(val, "ssh_host"), unread: field_bool(val, "unread"),
    })
}

fn status_to_json(ver: &str, pid: u32, socket: &str, active_tab: usize, panes: &[PaneStatusItem]) -> JsonVal {
    JsonVal::Object(vec![
        ("ok".into(), JsonVal::Bool(true)), ("kind".into(), j_str("status")),
        ("version".into(), j_str(ver)), ("pid".into(), j_num(pid)),
        ("socket".into(), j_str(socket)), ("active_tab".into(), j_num(active_tab)),
        ("panes".into(), JsonVal::Array(panes.iter().map(pane_to_json).collect())),
    ])
}

fn response_to_json(resp: &CtlResponse) -> JsonVal {
    match resp {
        CtlResponse::Ok { message } => {
            JsonVal::Object(vec![("ok".into(), JsonVal::Bool(true)), ("message".into(), j_opt_str(message))])
        }
        CtlResponse::Status { version, pid, socket, active_tab, panes } => {
            status_to_json(version, *pid, socket, *active_tab, panes)
        }
        CtlResponse::Error { message } => {
            JsonVal::Object(vec![("ok".into(), JsonVal::Bool(false)), ("error".into(), j_str(message))])
        }
    }
}

pub fn encode_response(resp: &CtlResponse) -> String {
    format_compact(&response_to_json(resp))
}

pub fn decode_response(line: &str) -> Result<CtlResponse, String> {
    let val = parse_json(line.trim())?;
    if !field_bool(&val, "ok") {
        let message = field_str(&val, "error").unwrap_or_else(|_| "unknown error".to_owned());
        return Ok(CtlResponse::Error { message });
    }
    if val.get("kind").and_then(JsonVal::as_str) == Some("status") {
        return decode_status_resp(&val);
    }
    Ok(CtlResponse::Ok { message: field_opt_str(&val, "message") })
}

fn decode_status_resp(val: &JsonVal) -> Result<CtlResponse, String> {
    let items = val.get("panes").and_then(JsonVal::as_array).unwrap_or(&[]);
    let mut panes = Vec::with_capacity(items.len());
    for item in items {
        panes.push(pane_from_json(item)?);
    }
    Ok(CtlResponse::Status {
        version: field_str(val, "version").unwrap_or_default(),
        pid: val.get("pid").and_then(JsonVal::as_u64).unwrap_or(0) as u32,
        socket: field_str(val, "socket").unwrap_or_default(),
        active_tab: val.get("active_tab").and_then(JsonVal::as_u64).unwrap_or(0) as usize,
        panes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_notify_req() -> CtlRequest {
        CtlRequest::Notify {
            pane_id: "12".into(), state: HookState::Waiting,
            title: Some("Клод \"Агент\" \\ тест".into()),
            message: Some("Строка 1\nСтрока 2\t✓\x07".into()), desktop: true,
        }
    }

    fn sample_pane_item() -> PaneStatusItem {
        PaneStatusItem {
            pane_id: "5".into(), tab_id: 2, tab_title: "Проект \"ЦУ\"".into(),
            active_tab: true, focused_pane: true, agent: "Claude".into(), state: "waiting".into(),
            hook_state: Some("waiting".into()), hook_title: Some("Claude Code".into()),
            hook_message: Some("Approve \"git push\"?\nLine 2".into()), cwd: "/home/dev/проект".into(),
            pid: 9991, ssh_host: Some("gpu-box".into()), unread: true,
        }
    }

    #[test]
    fn round_trips_all_five_requests_and_cyrillic_escaping() {
        let requests = [
            CtlRequest::Status,
            CtlRequest::Focus { pane_id: "7".into() },
            sample_notify_req(),
            CtlRequest::Send { pane_id: "3".into(), text: "echo \"привет\\nмир\"".into(), enter: true },
            CtlRequest::OpenDoc { pane_id: Some("9".into()), path: "/tmp/отчёт \"финал\".md".into() },
        ];
        for req in requests {
            let wire = encode_request(&req);
            assert!(!wire.contains('\n'));
            assert_eq!(decode_request(&wire).expect("decode"), req);
        }
    }

    #[test]
    fn round_trips_status_ok_and_error_responses() {
        let status = CtlResponse::Status {
            version: "1.4.0".into(), pid: 4242, socket: "/run/user/1000/akhterm-4242.sock".into(),
            active_tab: 2, panes: vec![sample_pane_item()],
        };
        let wire = encode_response(&status);
        assert!(!wire.contains('\n'));
        assert_eq!(decode_response(&wire).expect("status"), status);
        let ok = CtlResponse::Ok { message: Some("focused pane 5".into()) };
        assert_eq!(decode_response(&encode_response(&ok)).expect("ok"), ok);
        let err = CtlResponse::Error { message: "pane not found: \"99\"".into() };
        assert_eq!(decode_response(&encode_response(&err)).expect("err"), err);
    }

    #[test]
    fn parses_pane_id_variants_and_rejects_invalid_payloads() {
        assert_eq!(parse_pane_id_num("42"), Some(42));
        assert_eq!(parse_pane_id_num("pane-42"), Some(42));
        assert_eq!(parse_pane_id_num("p7"), Some(7));
        assert_eq!(parse_pane_id_num("0"), None);
        assert_eq!(parse_pane_id_num("abc"), None);
        assert!(decode_request("{\"cmd\":\"focus\",\"pane_id\":42}").is_ok());
        assert!(decode_request("{\"cmd\":\"unknown\"}").is_err());
        assert!(decode_request("not json").is_err());
    }
}
