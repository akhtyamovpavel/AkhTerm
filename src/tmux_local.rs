//! Local tmux client resolution, shared VTE title parsing, and Codex FD lookup.

use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::agent_detect::detect_title_agent;
use crate::pane_radar::PanePollJob;
use crate::pane_registry::PaneRecord;
use crate::paths::{expand_home, shorten_home};
use crate::proc_radar::ProcInfo;
use crate::remote_telemetry::RemoteTailer;
use crate::session::TermSession;
use crate::telemetry::{SessionPollJob, SessionTelemetry};

pub const TMUX_TITLE_FORMAT: &str =
    "AKHTERM_TMUX:#{session_name}|#{window_index}:#{window_name}|#{pane_current_command}|#{pane_current_path}";
const LOCAL_CLIENT_FMT: &str =
    "#{client_pid}|#{session_name}|#{window_index}:#{window_name}|#{pane_pid}|#{pane_current_command}|#{pane_current_path}";
const CACHE_TTL: Duration = Duration::from_millis(400);

type CacheKey = (Option<OsString>, Option<OsString>, Option<OsString>);
type CacheEntry = (Instant, CacheKey, Vec<LocalTmuxClient>);

static CLIENT_CACHE: Mutex<Option<CacheEntry>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTmuxClient {
    pub client_pid: i32,
    pub session_name: String,
    pub window_id: String,
    pub pane_pid: i32,
    pub pane_cmd: String,
    pub pane_path: String,
}

pub fn parse_tmux_vte_title(rest: &str) -> Option<(String, String, String, String)> {
    let mut parts = rest.splitn(4, '|');
    let sess = parts.next()?.trim();
    if sess.is_empty() {
        return None;
    }
    let win = parts.next()?.trim().to_owned();
    let cmd = parts.next()?.trim().to_owned();
    let path = parts.next()?.trim().to_owned();
    Some((sess.to_owned(), win, cmd, path))
}

pub fn apply_pane_tmux_title(pane: &mut PaneRecord, rest: &str) {
    let Some((sess, win, cmd, path)) = parse_tmux_vte_title(rest) else {
        return;
    };
    let switched = pane
        .remote_tmux
        .as_ref()
        .is_some_and(|(ps, pw, ..)| (ps.as_str(), pw.as_str()) != (sess.as_str(), win.as_str()));
    if switched {
        pane.proc_info.agent.clear();
        pane.osc_title.clear();
        pane.telemetry = SessionTelemetry::default();
    }
    pane.proc_info.tmux_session.clone_from(&sess);
    pane.proc_info.tmux_window.clone_from(&win);
    pane.remote_tmux = Some((sess, win, cmd, path));
}

pub fn clear_stale_pane_osc(pane: &mut PaneRecord, proc_info: &ProcInfo) {
    let prev = pane.proc_info.tmux_window.as_str();
    if !prev.is_empty() && prev != proc_info.tmux_window {
        pane.osc_title.clear();
    }
}

pub fn is_local_tmux_pane(proc_info: &ProcInfo) -> bool {
    !proc_info.has_tmux.is_empty() && proc_info.active_pane_pid > 0 && proc_info.ssh_host.is_empty()
}

pub fn pane_probe_pid(pane: &PaneRecord) -> i32 {
    if pane.proc_info.active_pane_pid > 0 {
        pane.proc_info.active_pane_pid
    } else {
        pane.child_pid
    }
}

pub fn matching_remote_tailer(
    remote_map: &HashMap<usize, RemoteTailer>,
    sid: usize,
    tmux_window: &Option<String>,
) -> Option<RemoteTailer> {
    let rt = remote_map.get(&sid)?;
    if tmux_window.is_none() || &rt.tmux_window == tmux_window {
        Some(rt.clone())
    } else {
        None
    }
}

pub fn pane_poll_job_from_pane(
    sid: usize,
    pane: &PaneRecord,
    is_daemon: bool,
    is_remote: bool,
) -> PanePollJob {
    let local_cwd = (!is_remote && pane.proc_info.ssh_host.is_empty() && !pane.cwd.is_empty())
        .then(|| expand_home(&pane.cwd));
    PanePollJob {
        sid, pane_id: pane.id, root_pid: pane_probe_pid(pane), vte: pane.vte,
        local_cwd,
        has_ssh: is_remote || !pane.proc_info.ssh_host.is_empty(),
        has_tmux: !pane.proc_info.has_tmux.is_empty() || pane.remote_tmux.is_some(),
        tmux_window: pane_tmux_window(pane),
        agent: pane.effective_agent(), is_daemon, telemetry: pane.telemetry.clone(),
    }
}

pub fn pane_into_session_job(job: PanePollJob) -> SessionPollJob {
    SessionPollJob {
        sid: job.sid, root_pid: job.root_pid, vte: job.vte, local_cwd: job.local_cwd,
        has_ssh: job.has_ssh, has_tmux: job.has_tmux, tmux_window: job.tmux_window,
        agent: job.agent, is_daemon: job.is_daemon, telemetry: job.telemetry,
    }
}

pub fn session_tmux_window(s: &TermSession) -> Option<String> {
    if let Some((_, w, ..)) = s.remote_tmux.as_ref() {
        let trimmed = w.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    let w = s.proc_info.tmux_window.trim();
    (!w.is_empty()).then(|| w.to_owned())
}

pub fn pane_tmux_window(pane: &PaneRecord) -> Option<String> {
    if let Some((_, w, ..)) = pane.remote_tmux.as_ref() {
        let trimmed = w.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    let w = pane.proc_info.tmux_window.trim();
    (!w.is_empty()).then(|| w.to_owned())
}

#[cfg(test)]
pub fn split_tmux_window_id(win: &str) -> (Option<&str>, &str) {
    match win.split_once(':') {
        Some((idx, name)) if !idx.is_empty() && idx.bytes().all(|b| b.is_ascii_digit()) => {
            (Some(idx), name)
        }
        _ => (None, win),
    }
}

pub fn parse_client_line(line: &str) -> Option<LocalTmuxClient> {
    let mut parts = line.splitn(6, '|');
    let client_pid = parts.next()?.trim().parse::<i32>().ok().filter(|&p| p > 0)?;
    let session_name = parts.next()?.to_owned();
    let window_id = parts.next()?.to_owned();
    let pane_pid = parts.next()?.trim().parse::<i32>().ok().filter(|&p| p > 0)?;
    let pane_cmd = parts.next()?.to_owned();
    let pane_path = parts.next()?.to_owned();
    Some(LocalTmuxClient { client_pid, session_name, window_id, pane_pid, pane_cmd, pane_path })
}

pub fn parse_clients_output(raw: &str) -> Vec<LocalTmuxClient> {
    raw.lines().filter_map(parse_client_line).collect()
}

fn current_cache_key() -> CacheKey {
    (env::var_os("TMUX"), env::var_os("TMUX_TMPDIR"), env::var_os("PATH"))
}

#[cfg(test)]
pub fn invalidate_client_cache() {
    if let Ok(mut g) = CLIENT_CACHE.lock() {
        *g = None;
    }
}

fn run_tmux_list_clients() -> Vec<LocalTmuxClient> {
    let out = Command::new("tmux")
        .args(["list-clients", "-F", LOCAL_CLIENT_FMT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => parse_clients_output(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

pub fn query_local_tmux_clients() -> Vec<LocalTmuxClient> {
    let key = current_cache_key();
    let now = Instant::now();
    if let Ok(g) = CLIENT_CACHE.lock() {
        if let Some((at, ref cached_key, ref rows)) = *g {
            if *cached_key == key && now.saturating_duration_since(at) < CACHE_TTL {
                return rows.clone();
            }
        }
    }
    let fresh = run_tmux_list_clients();
    if let Ok(mut g) = CLIENT_CACHE.lock() {
        *g = Some((now, key, fresh.clone()));
    }
    fresh
}

pub fn find_matching_client<'a>(
    visited: &HashSet<i32>,
    clients: &'a [LocalTmuxClient],
) -> Option<&'a LocalTmuxClient> {
    clients.iter().find(|c| visited.contains(&c.client_pid))
}

fn is_shell_name(cmd: &str) -> bool {
    matches!(cmd, "" | "zsh" | "bash" | "sh" | "fish")
}

pub fn apply_local_tmux_client<F>(
    client: &LocalTmuxClient,
    info: &mut ProcInfo,
    inspect_pane: F,
) where
    F: FnOnce(i32) -> ProcInfo,
{
    let pane_info = inspect_pane(client.pane_pid);
    info.tmux_session.clone_from(&client.session_name);
    info.tmux_window.clone_from(&client.window_id);
    info.active_pane_pid = client.pane_pid;
    apply_pane_cwd_and_agent(client, pane_info, info);
}

fn apply_pane_cwd_and_agent(
    client: &LocalTmuxClient,
    pane_info: ProcInfo,
    info: &mut ProcInfo,
) {
    if !client.pane_path.is_empty() {
        info.cwd = shorten_home(&client.pane_path);
    } else if !pane_info.cwd.is_empty() {
        info.cwd = pane_info.cwd;
    }
    info.agent = if !pane_info.agent.is_empty() {
        pane_info.agent
    } else {
        detect_title_agent(&client.pane_cmd).unwrap_or("").to_owned()
    };
    info.fg_cmd = resolve_pane_fg_cmd(&pane_info.fg_cmd, &client.pane_cmd);
}

fn resolve_pane_fg_cmd(pane_fg: &str, client_cmd: &str) -> String {
    if !pane_fg.is_empty() {
        pane_fg.to_owned()
    } else if !is_shell_name(client_cmd) {
        client_cmd.to_owned()
    } else {
        String::new()
    }
}

pub fn resolve_local_tmux_into<F>(visited: &HashSet<i32>, info: &mut ProcInfo, inspect_pane: F)
where
    F: FnOnce(i32) -> ProcInfo,
{
    let clients = query_local_tmux_clients();
    if let Some(client) = find_matching_client(visited, &clients) {
        apply_local_tmux_client(client, info, inspect_pane);
    }
}

fn collect_fd_tree_pids(root_pid: i32) -> Vec<i32> {
    if root_pid <= 0 {
        return Vec::new();
    }
    let mut visited = Vec::new();
    let mut queue = vec![root_pid];
    while let Some(pid) = queue.pop() {
        if visited.len() < 64 && !visited.contains(&pid) {
            visited.push(pid);
            push_fd_children(pid, &mut queue);
        }
    }
    visited
}

fn push_fd_children(pid: i32, queue: &mut Vec<i32>) {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else { return };
    for task in tasks.flatten() {
        if let Ok(raw) = fs::read_to_string(task.path().join("children")) {
            queue.extend(raw.split_whitespace().filter_map(|s| s.parse::<i32>().ok()));
        }
    }
}

fn codex_sessions_root(home: &Path) -> PathBuf {
    env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("sessions")
}

fn is_codex_subagent_file(path: &Path) -> bool {
    let Ok(file) = File::open(path) else { return false };
    let mut buf = String::new();
    let mut reader = BufReader::new(file.take(65_536));
    for _ in 0..3 {
        buf.clear();
        if reader.read_line(&mut buf).ok().unwrap_or(0) == 0 {
            break;
        }
        if buf.contains("\"subagent\"") {
            return true;
        }
    }
    false
}

fn scan_pid_codex_fd(pid: i32, sessions_root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    for entry in entries.flatten() {
        let Ok(target) = fs::read_link(entry.path()) else { continue };
        let is_jsonl = target.extension().and_then(|e| e.to_str()) == Some("jsonl");
        if is_jsonl && target.starts_with(sessions_root) && target.is_file() && !is_codex_subagent_file(&target) {
            return Some(target);
        }
    }
    None
}

pub fn discover_codex_by_fd(root_pid: i32, home: &Path) -> Option<PathBuf> {
    if root_pid <= 0 {
        return None;
    }
    let root = codex_sessions_root(home);
    if !root.is_dir() {
        return None;
    }
    collect_fd_tree_pids(root_pid)
        .into_iter()
        .find_map(|pid| scan_pid_codex_fd(pid, &root))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::pane_radar::apply_pane_vte_title;
    use crate::pane_registry::PaneId;
    use crate::remote_telemetry::tests::{dummy_session, seed_codex_0167_tmux_fixture, MockRemoteEnv};
    use crate::remote_telemetry::{
        run_ssh_probe_with_program, FrameMode, RemoteProbeFrame, RemoteSessionTarget,
        RemoteTelemetryScheduler,
    };
    use crate::session::SessionCategory;
    use crate::tabs::Tab;
    use crate::telemetry::{encode_claude_cwd, run_poll_job_with_remote, TelemetrySource};

    #[test]
    fn parses_legacy_and_indexed_tmux_vte_titles() {
        assert_eq!(
            parse_tmux_vte_title("dev|0:zsh|codex|/srv/app"),
            Some(("dev".into(), "0:zsh".into(), "codex".into(), "/srv/app".into()))
        );
        assert_eq!(
            parse_tmux_vte_title("dev|zsh|zsh|/srv/other"),
            Some(("dev".into(), "zsh".into(), "zsh".into(), "/srv/other".into()))
        );
        assert_eq!(split_tmux_window_id("1:my:win"), (Some("1"), "my:win"));
        assert_eq!(split_tmux_window_id("zsh"), (None, "zsh"));
        assert_eq!(parse_tmux_vte_title("|0:zsh|zsh|/tmp"), None);
    }

    #[test]
    fn resolves_local_tmux_client_and_active_pane_proc_info() {
        let out = "4100|work|0:codex|5100|codex|/tmp/repo-a\n4200|other|1:zsh|5200|zsh|/tmp/repo-b\n";
        let clients = parse_clients_output(out);
        let visited: HashSet<i32> = [3000, 4200].into_iter().collect();
        let client = find_matching_client(&visited, &clients).expect("matches client 4200");
        let mut info = ProcInfo { has_tmux: "tmux".into(), fg_cmd: "tmux".into(), ..Default::default() };
        apply_local_tmux_client(client, &mut info, |_| ProcInfo::default());
        assert_eq!(info.tmux_session, "other");
        assert_eq!(info.tmux_window, "1:zsh");
        assert_eq!(info.active_pane_pid, 5200);
        assert_eq!(info.cwd, "/tmp/repo-b");
        assert_eq!(info.fg_cmd, "");
    }

    #[test]
    fn tmux_local_meets_grade_b_mi_target() {
        let (_, _, score) = crate::mi::compute_file_mi(include_str!("tmux_local.rs"));
        assert!(score >= 60.0, "tmux_local.rs MI {score:.2} < 60.0");
    }

    fn seed_grouped_switch_panes(env: &MockRemoteEnv, codex_repo: &Path) -> (PathBuf, PathBuf) {
        let zsh_repo = env.home.join("plain_shell");
        let claude_repo = env.home.join("claude_repo");
        let claude_dir = env.home.join(".claude/projects").join(encode_claude_cwd(&claude_repo.to_string_lossy()));
        fs::create_dir_all(&zsh_repo).unwrap();
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(claude_dir.join("s.jsonl"), "{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":8000,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":140000,\"output_tokens\":900}}}\n").unwrap();
        write_switch_procs_and_tsv(env, codex_repo, &zsh_repo, &claude_repo);
        (zsh_repo, claude_repo)
    }

    fn write_switch_procs_and_tsv(env: &MockRemoteEnv, codex_repo: &Path, zsh_repo: &Path, claude_repo: &Path) {
        env.add_proc(3100, "-zsh\0", codex_repo, &[3101]);
        env.add_proc(3101, "codex\0resume\0", codex_repo, &[]);
        env.add_proc(3200, "-zsh\0", zsh_repo, &[]);
        env.add_proc(3300, "-zsh\0", claude_repo, &[3301]);
        env.add_proc(3301, "claude\0", claude_repo, &[]);
        let tsv = format!(
            "akhterm-1-pane-0|1|1|3100|codex|{}|0|codex\nakhterm-1-pane-0|0|1|3200|zsh|{}|1|zsh\nakhterm-1-pane-0|0|1|3300|claude|{}|2|claude\n",
            codex_repo.display(), zsh_repo.display(), claude_repo.display()
        );
        fs::write(&env.tmux_panes_file, tsv).unwrap();
    }

    fn probe_grouped_win(
        sched: &RemoteTelemetryScheduler,
        env: &MockRemoteEnv,
        s: &mut TermSession,
        win: (&str, &str, &Path),
    ) -> RemoteTailer {
        s.remote_tmux = Some(("akhterm-1-pane-0".into(), win.0.into(), win.1.into(), win.2.to_string_lossy().into_owned()));
        sched.sync_targets(&[RemoteSessionTarget::from_session(s).unwrap()]);
        let prog = env.ssh_bin.to_string_lossy().into_owned();
        sched.refresh_due_with(Instant::now(), |h, sc| run_ssh_probe_with_program(&prog, h, sc));
        sched.snapshot().remove(&s.id).unwrap()
    }

    #[test]
    fn remote_tmux_window_switch_switches_agents_and_clears_plain_shell() {
        let env = MockRemoteEnv::new("winswitch");
        let codex_repo = env.home.join("Development/Akhcheck");
        seed_codex_0167_tmux_fixture(&env);
        let (zsh_repo, claude_repo) = seed_grouped_switch_panes(&env, &codex_repo);
        let mut s = dummy_session(21, SessionCategory::Remote, Tab::remote("home", Some("akhterm-1-pane-0")), "home");
        let sched = RemoteTelemetryScheduler::new(Duration::from_secs(60));
        let rt1 = probe_grouped_win(&sched, &env, &mut s, ("0:codex", "codex", &codex_repo));
        let rt2 = probe_grouped_win(&sched, &env, &mut s, ("1:zsh", "zsh", &zsh_repo));
        let rt3 = probe_grouped_win(&sched, &env, &mut s, ("2:claude", "claude", &claude_repo));
        assert_eq!((rt1.snapshot.source, rt1.snapshot.context_tokens), (TelemetrySource::Codex, 154_000));
        assert_eq!((env.call_count(), rt2.snapshot.has_data(), rt2.tailer.snapshot.has_data()), (3, false, false));
        assert_eq!((rt3.snapshot.source, rt3.snapshot.context_tokens), (TelemetrySource::Claude, 148_000));
    }

    #[test]
    fn matches_target_and_frame_none_clear_remote_tailer() {
        let t0 = RemoteSessionTarget {
            sid: 1, host: "srv".into(), tmux_session: Some("dev".into()),
            tmux_window: Some("0:zsh".into()), pane_cwd_hint: String::new(),
            remote_cwd: None, agent_hint: String::new(),
        };
        let t1 = RemoteSessionTarget { tmux_window: Some("1:zsh".into()), ..t0.clone() };
        let mut rt = RemoteTailer::for_target(&t0);
        assert!(rt.matches_target(&t0) && !rt.matches_target(&t1));
        apply_sample_claude_and_none(&mut rt);
    }

    fn apply_sample_claude_and_none(rt: &mut RemoteTailer) {
        rt.apply_frame(&RemoteProbeFrame {
            sid: 1, mode: FrameMode::Full, kind: TelemetrySource::Claude, size: 160, mtime: 1,
            path: "/r/c.jsonl".into(), head: Vec::new(),
            body: b"{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":5000,\"cache_read_input_tokens\":20000,\"output_tokens\":200}}}\n".to_vec(),
        });
        assert!(rt.snapshot.has_data());
        rt.apply_frame(&RemoteProbeFrame {
            sid: 1, mode: FrameMode::None, kind: TelemetrySource::None, size: 0, mtime: 0,
            path: String::new(), head: Vec::new(), body: Vec::new(),
        });
        assert!(!rt.snapshot.has_data() && !rt.tailer.snapshot.has_data() && rt.tailer.path.is_none());
    }

    #[test]
    fn apply_tmux_vte_title_resets_agent_and_telemetry_on_duplicate_window_names() {
        let mut s = dummy_session(1, SessionCategory::Remote, Tab::remote("srv", Some("dev")), "srv");
        s.apply_vte_title(std::ptr::null_mut(), "AKHTERM_TMUX:dev|0:zsh|codex|/srv/a");
        s.telemetry.vte_fallback.context_tokens = 42_000;
        assert_eq!((s.proc_info.agent.as_str(), s.proc_info.tmux_window.as_str()), ("Codex", "0:zsh"));
        s.apply_vte_title(std::ptr::null_mut(), "AKHTERM_TMUX:dev|1:zsh|zsh|/srv/b");
        assert_eq!((s.proc_info.agent.as_str(), s.proc_info.tmux_window.as_str()), ("", "1:zsh"));
        assert!(!s.telemetry.snapshot().has_data());
        let mut pane = PaneRecord::new(PaneId(1), std::ptr::null_mut(), std::ptr::null_mut(), 0, String::new());
        apply_pane_vte_title(&mut pane, "AKHTERM_TMUX:dev|0:zsh|codex|/srv/a", "srv");
        apply_pane_vte_title(&mut pane, "AKHTERM_TMUX:dev|1:zsh|zsh|/srv/b", "srv");
        assert_eq!(pane.proc_info.tmux_window, "1:zsh");
    }

    fn seed_local_switch_files(base: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let home = base.join("home");
        let codex_day = home.join(".codex/sessions/2026/11/14");
        let claude_repo = home.join("claude_proj");
        let claude_dir = home.join(".claude/projects").join(encode_claude_cwd(&claude_repo.to_string_lossy()));
        fs::create_dir_all(&codex_day).unwrap();
        fs::create_dir_all(&claude_dir).unwrap();
        let codex_path = codex_day.join("rollout-fd.jsonl");
        fs::write(&codex_path, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":52000,\"cached_input_tokens\":40000,\"output_tokens\":800,\"total_tokens\":52800},\"last_token_usage\":{\"input_tokens\":52000,\"cached_input_tokens\":40000,\"output_tokens\":800,\"total_tokens\":52800},\"model_context_window\":200000}}}\n").unwrap();
        fs::write(claude_dir.join("c.jsonl"), "{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":4000,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":70000,\"output_tokens\":500}}}\n").unwrap();
        (home, codex_path, claude_repo)
    }

    fn local_job(root_pid: i32, cwd: &Path, win: &str, agent: &str, telemetry: SessionTelemetry) -> SessionPollJob {
        SessionPollJob {
            sid: 1, root_pid, vte: std::ptr::null_mut(), local_cwd: Some(cwd.to_string_lossy().into_owned()),
            has_ssh: false, has_tmux: true, tmux_window: Some(win.into()), agent: agent.into(),
            is_daemon: false, telemetry,
        }
    }

    #[test]
    fn local_tmux_codex_fd_and_window_switch_clears_stale_telemetry() {
        let base = std::env::temp_dir().join(format!("akhterm_tmux_local_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (home, codex_path, claude_repo) = seed_local_switch_files(&base);
        let _held_fd = File::open(&codex_path).unwrap();
        let self_pid = std::process::id() as i32;
        assert_eq!(discover_codex_by_fd(self_pid, &home).as_deref(), Some(codex_path.as_path()));
        assert_local_switch_sequence(&home, &base, &claude_repo, self_pid);
        let _ = fs::remove_dir_all(&base);
    }

    fn assert_local_switch_sequence(home: &Path, base: &Path, claude_repo: &Path, self_pid: i32) {
        let now = Instant::now();
        let (_, t0) = run_poll_job_with_remote(local_job(self_pid, home, "0:codex", "Codex", SessionTelemetry::default()), None, home, now);
        assert_eq!((t0.snapshot().source, t0.snapshot().context_tokens), (TelemetrySource::Codex, 52_000));
        let (_, t1) = run_poll_job_with_remote(local_job(0, base, "1:zsh", "", t0), None, home, now);
        assert!(!t1.snapshot().has_data() && t1.tailer.path.is_none());
        let (_, t2) = run_poll_job_with_remote(local_job(0, claude_repo, "2:claude", "Claude", t1), None, home, now);
        assert_eq!((t2.snapshot().source, t2.snapshot().context_tokens), (TelemetrySource::Claude, 74_000));
        invalidate_client_cache();
        let _ = query_local_tmux_clients();
    }
}
