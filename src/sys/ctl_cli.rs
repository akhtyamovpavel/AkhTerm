//! Early CLI dispatcher (`akhterm status|focus|notify|send|open-doc|hooks`, plus `ctl` alias).

use std::path::PathBuf;
use std::process::ExitCode;

use crate::sys::agent_hooks::{install_hooks, HookAgent, HookInstallOutcome, InstallOptions};
use crate::sys::ctl_protocol::{encode_response, CtlRequest, CtlResponse, HookState, PaneStatusItem};
use crate::sys::ctl_socket::{discover_socket, send_request};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    Help,
    Request {
        socket: Option<PathBuf>,
        request: CtlRequest,
        json_output: bool,
    },
    HooksInstall {
        agent: HookAgent,
        dry_run: bool,
    },
}

pub fn try_run(args: &[String]) -> Option<ExitCode> {
    if args.len() <= 1 {
        return None;
    }
    let env_pane = std::env::var("AKHTERM_PANE_ID").ok().filter(|v| !v.is_empty());
    let cwd = std::env::current_dir().ok();
    match parse_cli_args(&args[1..], env_pane.as_deref(), cwd.as_deref()) {
        Ok(cmd) => Some(execute_cli_command(cmd)),
        Err(err) => {
            eprintln!("akhterm: {err}");
            Some(ExitCode::FAILURE)
        }
    }
}

fn dispatch_subcommand(
    sub: &str,
    tail: &[String],
    socket: Option<PathBuf>,
    env_pane: Option<&str>,
    cwd: Option<&std::path::Path>,
) -> Result<CliCommand, String> {
    match sub {
        "-h" | "--help" | "help" => Ok(CliCommand::Help),
        "status" => parse_status(tail, socket),
        "focus" => parse_focus(tail, socket),
        "notify" => parse_notify(tail, socket, env_pane),
        "send" => parse_send(tail, socket),
        "capture" => parse_capture(tail, socket, env_pane),
        "send-key" | "send_key" => parse_send_key(tail, socket),
        "open-doc" | "open_doc" => parse_open_doc(tail, socket, env_pane, cwd),
        "hooks" => parse_hooks(tail),
        other => Err(format!("unknown command '{other}' (run 'akhterm --help')")),
    }
}

pub fn parse_cli_args(
    raw: &[String],
    env_pane: Option<&str>,
    cwd: Option<&std::path::Path>,
) -> Result<CliCommand, String> {
    let (socket, rest) = peel_leading_socket(raw)?;
    let rest = if rest.first().map(String::as_str) == Some("ctl") { &rest[1..] } else { rest };
    match rest.first().map(String::as_str) {
        Some(sub) => dispatch_subcommand(sub, &rest[1..], socket, env_pane, cwd),
        None => Ok(CliCommand::Help),
    }
}

fn peel_leading_socket(args: &[String]) -> Result<(Option<PathBuf>, &[String]), String> {
    if args.first().map(String::as_str) == Some("--socket") {
        let val = args.get(1).ok_or_else(|| "--socket requires a path".to_owned())?;
        return Ok((Some(PathBuf::from(val)), &args[2..]));
    }
    Ok((None, args))
}

fn take_opt_val(args: &[String], idx: &mut usize, flag: &str) -> Result<String, String> {
    *idx += 1;
    args.get(*idx)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_status(args: &[String], mut socket: Option<PathBuf>) -> Result<CliCommand, String> {
    let mut json_output = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json_output = true,
            "--socket" => socket = Some(PathBuf::from(take_opt_val(args, &mut i, "--socket")?)),
            other => return Err(format!("unexpected argument '{other}' for status")),
        }
        i += 1;
    }
    Ok(CliCommand::Request { socket, request: CtlRequest::Status, json_output })
}

fn apply_focus_arg(args: &[String], i: &mut usize, socket: &mut Option<PathBuf>, pane_id: &mut Option<String>) -> Result<(), String> {
    match args[*i].as_str() {
        "--socket" => *socket = Some(PathBuf::from(take_opt_val(args, i, "--socket")?)),
        "--pane" => *pane_id = Some(take_opt_val(args, i, "--pane")?),
        arg if pane_id.is_none() => *pane_id = Some(arg.to_owned()),
        other => return Err(format!("unexpected argument '{other}' for focus")),
    }
    Ok(())
}

fn parse_focus(args: &[String], mut socket: Option<PathBuf>) -> Result<CliCommand, String> {
    let (mut pane_id, mut i) = (None, 0);
    while i < args.len() {
        apply_focus_arg(args, &mut i, &mut socket, &mut pane_id)?;
        i += 1;
    }
    let pane_id = pane_id.ok_or_else(|| "focus requires <pane_id>".to_owned())?;
    Ok(CliCommand::Request { socket, request: CtlRequest::Focus { pane_id }, json_output: false })
}

#[derive(Default)]
struct NotifyDraft {
    socket: Option<PathBuf>,
    pane_id: Option<String>,
    state: Option<HookState>,
    title: Option<String>,
    message: Option<String>,
    desktop: bool,
}

fn parse_notify_state(raw: &str) -> Result<HookState, String> {
    HookState::parse(raw)
        .ok_or_else(|| format!("invalid --state '{raw}' (use working|waiting|done|error)"))
}

fn apply_notify_flag(draft: &mut NotifyDraft, args: &[String], i: &mut usize) -> Result<(), String> {
    match args[*i].as_str() {
        "--socket" => draft.socket = Some(PathBuf::from(take_opt_val(args, i, "--socket")?)),
        "--pane" => draft.pane_id = Some(take_opt_val(args, i, "--pane")?),
        "--state" => draft.state = Some(parse_notify_state(&take_opt_val(args, i, "--state")?)?),
        "--title" => draft.title = Some(take_opt_val(args, i, "--title")?),
        "--message" => draft.message = Some(take_opt_val(args, i, "--message")?),
        "--desktop" => draft.desktop = true,
        other => return Err(format!("unexpected argument '{other}' for notify")),
    }
    Ok(())
}

fn finish_notify_draft(draft: NotifyDraft) -> Result<CliCommand, String> {
    let pane_id = draft.pane_id.ok_or_else(|| "notify requires --pane <id> or AKHTERM_PANE_ID".to_owned())?;
    let state = draft.state.ok_or_else(|| "notify requires --state working|waiting|done|error".to_owned())?;
    let request = CtlRequest::Notify {
        pane_id, state, title: draft.title, message: draft.message, desktop: draft.desktop,
    };
    Ok(CliCommand::Request { socket: draft.socket, request, json_output: false })
}

fn parse_notify(args: &[String], socket: Option<PathBuf>, env_pane: Option<&str>) -> Result<CliCommand, String> {
    let mut draft = NotifyDraft { socket, pane_id: env_pane.map(str::to_owned), ..Default::default() };
    let mut i = 0;
    while i < args.len() {
        apply_notify_flag(&mut draft, args, &mut i)?;
        i += 1;
    }
    finish_notify_draft(draft)
}

fn build_send_cmd(socket: Option<PathBuf>, mut pos: Vec<String>, enter: bool) -> Result<CliCommand, String> {
    if pos.len() != 2 {
        return Err("send requires <pane_id> <text> [--enter]".to_owned());
    }
    let request = CtlRequest::Send { pane_id: pos.remove(0), text: pos.remove(0), enter };
    Ok(CliCommand::Request { socket, request, json_output: false })
}

fn parse_send(args: &[String], mut socket: Option<PathBuf>) -> Result<CliCommand, String> {
    let (mut enter, mut positional, mut i) = (false, Vec::new(), 0);
    while i < args.len() {
        match args[i].as_str() {
            "--socket" => socket = Some(PathBuf::from(take_opt_val(args, &mut i, "--socket")?)),
            "--enter" => enter = true,
            arg => positional.push(arg.to_owned()),
        }
        i += 1;
    }
    build_send_cmd(socket, positional, enter)
}

fn apply_capture_arg(
    args: &[String],
    i: &mut usize,
    socket: &mut Option<PathBuf>,
    pane_id: &mut Option<String>,
    lines: &mut usize,
    json_out: &mut bool,
) -> Result<(), String> {
    match args[*i].as_str() {
        "--socket" => *socket = Some(PathBuf::from(take_opt_val(args, i, "--socket")?)),
        "--pane" => *pane_id = Some(take_opt_val(args, i, "--pane")?),
        "--lines" => *lines = take_opt_val(args, i, "--lines")?.parse().map_err(|_| "invalid --lines")?,
        "--json" => *json_out = true,
        arg if pane_id.is_none() => *pane_id = Some(arg.to_owned()),
        other => return Err(format!("unexpected argument '{other}' for capture")),
    }
    Ok(())
}

fn parse_capture(args: &[String], mut socket: Option<PathBuf>, env_pane: Option<&str>) -> Result<CliCommand, String> {
    let (mut pane_id, mut lines, mut json_output, mut i) = (env_pane.map(str::to_owned), 100usize, false, 0);
    while i < args.len() {
        apply_capture_arg(args, &mut i, &mut socket, &mut pane_id, &mut lines, &mut json_output)?;
        i += 1;
    }
    let pane_id = pane_id.ok_or_else(|| "capture requires <pane_id> or AKHTERM_PANE_ID".to_owned())?;
    Ok(CliCommand::Request { socket, request: CtlRequest::Capture { pane_id, lines }, json_output })
}

fn build_send_key_cmd(socket: Option<PathBuf>, mut pos: Vec<String>) -> Result<CliCommand, String> {
    if pos.len() != 2 {
        return Err("send-key requires <pane_id> <enter|esc|up|down|tab|ctrl_c|1|2|3>".to_owned());
    }
    let request = CtlRequest::SendKey { pane_id: pos.remove(0), key: pos.remove(0) };
    Ok(CliCommand::Request { socket, request, json_output: false })
}

fn parse_send_key(args: &[String], mut socket: Option<PathBuf>) -> Result<CliCommand, String> {
    let (mut pos, mut i) = (Vec::new(), 0);
    while i < args.len() {
        if args[i] == "--socket" {
            socket = Some(PathBuf::from(take_opt_val(args, &mut i, "--socket")?));
        } else {
            pos.push(args[i].clone());
        }
        i += 1;
    }
    build_send_key_cmd(socket, pos)
}

fn apply_open_doc_arg(
    args: &[String],
    i: &mut usize,
    socket: &mut Option<PathBuf>,
    pane_id: &mut Option<String>,
    path_raw: &mut Option<String>,
) -> Result<(), String> {
    match args[*i].as_str() {
        "--socket" => *socket = Some(PathBuf::from(take_opt_val(args, i, "--socket")?)),
        "--pane" => *pane_id = Some(take_opt_val(args, i, "--pane")?),
        arg if path_raw.is_none() => *path_raw = Some(arg.to_owned()),
        other => return Err(format!("unexpected argument '{other}' for open-doc")),
    }
    Ok(())
}

fn parse_open_doc(
    args: &[String],
    mut socket: Option<PathBuf>,
    env_pane: Option<&str>,
    cwd: Option<&std::path::Path>,
) -> Result<CliCommand, String> {
    let (mut pane_id, mut path_raw, mut i) = (env_pane.map(str::to_owned), None, 0);
    while i < args.len() {
        apply_open_doc_arg(args, &mut i, &mut socket, &mut pane_id, &mut path_raw)?;
        i += 1;
    }
    let raw = path_raw.ok_or_else(|| "open-doc requires <path>".to_owned())?;
    let request = CtlRequest::OpenDoc { pane_id, path: resolve_doc_path(&raw, cwd) };
    Ok(CliCommand::Request { socket, request, json_output: false })
}

fn resolve_doc_path(raw: &str, cwd: Option<&std::path::Path>) -> String {
    let expanded = crate::sys::paths::expand_home(raw);
    if std::path::Path::new(&expanded).is_absolute() {
        return expanded;
    }
    match cwd {
        Some(base) => base.join(&expanded).to_string_lossy().into_owned(),
        None => expanded,
    }
}

fn parse_hooks_agent(raw: &str) -> Result<HookAgent, String> {
    HookAgent::parse(raw).ok_or_else(|| format!("invalid --agent '{raw}' (use claude|codex|all)"))
}

fn apply_hooks_arg(args: &[String], i: &mut usize, agent: &mut HookAgent, dry_run: &mut bool) -> Result<(), String> {
    match args[*i].as_str() {
        "--agent" => *agent = parse_hooks_agent(&take_opt_val(args, i, "--agent")?)?,
        "--dry-run" => *dry_run = true,
        other => return Err(format!("unexpected argument '{other}' for hooks install")),
    }
    Ok(())
}

fn parse_hooks(args: &[String]) -> Result<CliCommand, String> {
    if args.first().map(String::as_str) != Some("install") {
        return Err("hooks supports 'hooks install [--agent claude|codex|all] [--dry-run]'".to_owned());
    }
    let (mut agent, mut dry_run, mut i) = (HookAgent::All, false, 1);
    while i < args.len() {
        apply_hooks_arg(args, &mut i, &mut agent, &mut dry_run)?;
        i += 1;
    }
    Ok(CliCommand::HooksInstall { agent, dry_run })
}

fn execute_cli_command(cmd: CliCommand) -> ExitCode {
    match cmd {
        CliCommand::Help => {
            print!("{}", format_cli_help());
            ExitCode::SUCCESS
        }
        CliCommand::HooksInstall { agent, dry_run } => run_hooks_install(agent, dry_run),
        CliCommand::Request { socket, request, json_output } => {
            run_socket_command(socket, &request, json_output)
        }
    }
}

fn run_hooks_install(agent: HookAgent, dry_run: bool) -> ExitCode {
    let opts = InstallOptions::for_current_env(agent, dry_run);
    match install_hooks(&opts) {
        Ok(outcomes) => {
            for item in outcomes {
                print_hook_outcome(&item);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("akhterm hooks install: {err}");
            ExitCode::FAILURE
        }
    }
}

fn print_hook_outcome(item: &HookInstallOutcome) {
    println!("[{}] {}: {} ({})", item.status.as_str(), item.agent, item.path.display(), item.summary);
    if let Some(preview) = &item.preview {
        println!("{preview}");
    }
}

fn resolve_cli_socket(socket: Option<PathBuf>) -> Result<PathBuf, String> {
    match socket {
        Some(p) => Ok(p),
        None => discover_socket(),
    }
}

fn run_socket_command(socket: Option<PathBuf>, req: &CtlRequest, json_output: bool) -> ExitCode {
    let path = match resolve_cli_socket(socket) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("akhterm: {err}");
            return ExitCode::FAILURE;
        }
    };
    match send_request(&path, req) {
        Ok(resp) => handle_response(&resp, json_output),
        Err(err) => {
            eprintln!("akhterm: {err}");
            ExitCode::FAILURE
        }
    }
}

fn format_pane_row(p: &PaneStatusItem) -> String {
    let mark = if p.focused_pane { "*" } else { " " };
    let agent = if p.agent.is_empty() { "shell" } else { p.agent.as_str() };
    let cwd = crate::sys::paths::shorten_home(&p.cwd);
    format!("{mark}{:<5} {:<4} {:<13} {:<10} {cwd}\n", p.pane_id, p.tab_id, agent, p.state)
}

fn handle_json_response(resp: &CtlResponse) -> ExitCode {
    println!("{}", encode_response(resp));
    if matches!(resp, CtlResponse::Error { .. }) { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn print_ok_response(message: Option<&str>) -> ExitCode {
    if let Some(msg) = message {
        println!("{msg}");
    }
    ExitCode::SUCCESS
}

fn print_capture_lines(lines: &[String]) -> ExitCode {
    for line in lines {
        println!("{line}");
    }
    ExitCode::SUCCESS
}

fn handle_response(resp: &CtlResponse, json_output: bool) -> ExitCode {
    if json_output {
        return handle_json_response(resp);
    }
    match resp {
        CtlResponse::Ok { message } => print_ok_response(message.as_deref()),
        CtlResponse::Status { version, pid, active_tab, panes, .. } => {
            print!("{}", format_status_table(version, *pid, *active_tab, panes));
            ExitCode::SUCCESS
        }
        CtlResponse::Capture { lines, .. } => print_capture_lines(lines),
        CtlResponse::Error { message } => {
            eprintln!("akhterm: {message}");
            ExitCode::FAILURE
        }
    }
}

pub fn format_status_table(version: &str, pid: u32, active_tab: usize, panes: &[PaneStatusItem]) -> String {
    let mut out = format!("AkhTerm {version} (pid {pid}, active tab {active_tab})\n");
    if panes.is_empty() {
        out.push_str("No open panes.\n");
        return out;
    }
    out.push_str("PANE   TAB  AGENT         STATE      CWD\n");
    for p in panes {
        out.push_str(&format_pane_row(p));
    }
    out
}

const CLI_HELP_HEADER: &str = "AkhTerm CLI control plane\nUsage:\n  akhterm                                 Launch AkhTerm GUI\n  akhterm --version | -V                  Print version information\n";
const CLI_HELP_CTL: &str = "  akhterm [ctl] status [--json]           List tabs, panes, agents, and states\n  akhterm [ctl] focus <pane_id>           Focus tab, split pane, and raise window\n  akhterm [ctl] notify --state <state>    Push hook state (working|waiting|done|error)\n      [--pane <id>] [--title <t>] [--message <m>] [--desktop]\n";
const CLI_HELP_MORE: &str = "  akhterm [ctl] send <pane_id> <text> [--enter]\n                                          Send text into a pane PTY\n  akhterm [ctl] open-doc <path> [--pane <id>]\n                                          Open Markdown preview alongside a pane\n  akhterm [ctl] hooks install [--agent claude|codex|all] [--dry-run]\n                                          Install official agent notification hooks\n";

pub fn format_cli_help() -> String {
    format!("{CLI_HELP_HEADER}{CLI_HELP_CTL}{CLI_HELP_MORE}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn sv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_direct_and_ctl_alias_subcommands() {
        let s1 = parse_cli_args(&sv(&["status", "--json"]), None, None).unwrap();
        assert_eq!(s1, CliCommand::Request { socket: None, request: CtlRequest::Status, json_output: true });
        let s2 = parse_cli_args(&sv(&["ctl", "status"]), None, None).unwrap();
        assert_eq!(s2, CliCommand::Request { socket: None, request: CtlRequest::Status, json_output: false });
        let f = parse_cli_args(&sv(&["ctl", "focus", "7"]), None, None).unwrap();
        assert_eq!(f, CliCommand::Request { socket: None, request: CtlRequest::Focus { pane_id: "7".into() }, json_output: false });
    }

    #[test]
    fn parses_send_open_doc_and_hooks() {
        let send = parse_cli_args(&sv(&["send", "3", "cargo test", "--enter"]), None, None).unwrap();
        assert_eq!(send, CliCommand::Request {
            socket: None, request: CtlRequest::Send { pane_id: "3".into(), text: "cargo test".into(), enter: true }, json_output: false,
        });
        let doc = parse_cli_args(&sv(&["open-doc", "README.md"]), Some("4"), Some(Path::new("/repo"))).unwrap();
        assert_eq!(doc, CliCommand::Request {
            socket: None, request: CtlRequest::OpenDoc { pane_id: Some("4".into()), path: "/repo/README.md".into() }, json_output: false,
        });
        let hooks = parse_cli_args(&sv(&["hooks", "install", "--agent", "claude", "--dry-run"]), None, None).unwrap();
        assert_eq!(hooks, CliCommand::HooksInstall { agent: HookAgent::Claude, dry_run: true });
    }

    #[test]
    fn parses_notify_with_env_pane_fallback() {
        let notify = parse_cli_args(
            &sv(&["notify", "--state", "waiting", "--title", "Claude", "--message", "Approve?", "--desktop"]),
            Some("11"),
            None,
        );
        let expected = CtlRequest::Notify {
            pane_id: "11".into(), state: HookState::Waiting,
            title: Some("Claude".into()), message: Some("Approve?".into()), desktop: true,
        };
        assert_eq!(notify, Ok(CliCommand::Request { socket: None, request: expected, json_output: false }));
    }

    #[test]
    fn rejects_invalid_args_and_has_no_text_arrows_in_help() {
        assert!(parse_cli_args(&sv(&["notify", "--state", "done"]), None, None).is_err());
        assert!(parse_cli_args(&sv(&["notify", "--pane", "1", "--state", "bogus"]), None, None).is_err());
        assert!(parse_cli_args(&sv(&["focus"]), None, None).is_err());
        assert!(parse_cli_args(&sv(&["hooks", "remove"]), None, None).is_err());
        let help = format_cli_help();
        assert!(!help.contains("->") && !help.contains('→') && !help.contains("=>"));
    }
}
