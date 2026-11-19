use super::*;
use crate::pane_registry::{PaneId, PaneRecord};
use crate::proc_radar::ProcInfo;
use crate::telemetry::{TelemetrySource, TokenSnapshot};
use crate::tmux_local::{apply_pane_tmux_title, pane_tmux_window};
use std::path::Path;
use std::{fs, ptr};

fn dummy_pane() -> PaneRecord {
    PaneRecord::new(PaneId(1), ptr::null_mut(), ptr::null_mut(), 1, "/repo".into())
}

fn tmux_target(sid: usize, host: &str, sess: &str, win: &str) -> RemoteSessionTarget {
    RemoteSessionTarget {
        sid,
        host: host.into(),
        tmux_session: Some(sess.into()),
        tmux_window: Some(win.into()),
        ..Default::default()
    }
}

fn plain_port_target(sid: usize, port: u16) -> RemoteSessionTarget {
    RemoteSessionTarget {
        sid,
        host: "box".into(),
        ports: Some((port, 22)),
        ..Default::default()
    }
}

fn refresh_mock(sched: &RemoteTelemetryScheduler, env: &tests::MockRemoteEnv, at: Instant) {
    sched.refresh_due_with(at, |host, cmd| {
        run_ssh_probe_with_program(env.ssh_bin.to_str().unwrap(), host, cmd)
    });
}

fn write_ssh_env(proc_dir: &Path, port: u16) {
    let val = format!("SSH_CONNECTION=127.0.0.1 {port} 127.0.0.1 22\0");
    fs::write(proc_dir.join("environ"), val.as_bytes()).unwrap();
}

fn write_opencode_fd(env: &tests::MockRemoteEnv, pid: u32, port: u16) {
    let proc_dir = env.add_proc(pid, "opencode\0", &env.home, &[]);
    write_ssh_env(&proc_dir, port);
    let dir = env.home.join(".local/state/akhterm/opencode/client-A");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ses_a.jsonl");
    let line = "{\"input\":200,\"read\":800,\"context\":1000,\"last_read\":800}\n";
    fs::write(&file, line).unwrap();
    std::os::unix::fs::symlink(file, proc_dir.join("fd/3")).unwrap();
}

fn write_claude_fd(env: &tests::MockRemoteEnv, dir: &Path, pid: u32, count: u32) {
    let proc_dir = env.add_proc(pid, "claude\0", &env.home, &[]);
    let log = dir.join(format!("{pid}.jsonl"));
    let line = format!(
        r#"{{"type":"assistant","message":{{"id":"msg","usage":{{"input_tokens":{count},"output_tokens":2}}}}}}"#
    );
    fs::write(&log, format!("{line}\n")).unwrap();
    std::os::unix::fs::symlink(log, proc_dir.join("fd/3")).unwrap();
}

fn write_codex_fixture(env: &tests::MockRemoteEnv, context: u32) {
    let process = env.add_proc(4200, "codex\0", &env.home, &[]);
    let sessions = env.home.join(".codex/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let log = sessions.join("rollout.jsonl");
    let usage = format!(r#"{{"input_tokens":{context},"cached_input_tokens":200,"output_tokens":10}}"#);
    let line = format!(
        r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{usage},"last_token_usage":{usage}}}}}}}"#
    );
    fs::write(&log, format!("{line}\n")).unwrap();
    std::os::unix::fs::symlink(log, process.join("fd/3")).unwrap();
    let panes = format!("work|1|1|4200|codex|{}|0|work|%4\n", env.home.display());
    fs::write(&env.tmux_panes_file, panes).unwrap();
}

fn rewrite_proxy_config(env: &tests::MockRemoteEnv, base: &str) -> PathBuf {
    let home = shell_quote(&env.home.to_string_lossy());
    let proc = shell_quote(&env.proc_dir.to_string_lossy());
    let bin = shell_quote(&env.root.join("bin").to_string_lossy());
    let prefix = format!(
        "ProxyCommand env HOME={home} AKHTERM_TEST_PROC={proc} PATH={bin}:\"$PATH\" "
    );
    assert!(base.contains("ProxyCommand "));
    let config = env.root.join("ssh-config");
    fs::write(&config, base.replacen("ProxyCommand ", &prefix, 1)).unwrap();
    config
}

fn isolated_ssh_config(env: &tests::MockRemoteEnv, base: &str, context: u32) -> PathBuf {
    write_codex_fixture(env, context);
    rewrite_proxy_config(env, base)
}

fn write_managed_shell_stats(shell: &Path, agent: &Path, tpgid: i32) {
    let sh = format!("4100 (zsh) S 1 4100 4100 34816 {tpgid}");
    let ag = format!("4200 (codex) S 4100 4200 4100 34816 {tpgid}");
    fs::write(shell.join("stat"), sh).unwrap();
    fs::write(agent.join("stat"), ag).unwrap();
}

fn assert_managed_context(
    session: &mut TermSession,
    pane_id: PaneId,
    env: &tests::MockRemoteEnv,
    sched: &RemoteTelemetryScheduler,
    title: Option<&str>,
    at: Instant,
) {
    if let Some(t) = title {
        apply_pane_tmux_title(session.pane_registry.get_mut(pane_id).unwrap(), t);
    }
    let target = RemoteSessionTarget::from_pane(session, &session.pane_registry.panes()[0]).unwrap();
    sched.sync_targets(&[target]);
    refresh_mock(sched, env, at);
    let jobs = crate::pane_radar::collect_sessions_pane_jobs(std::slice::from_ref(session));
    let result = crate::pane_radar::run_pane_poll_jobs(jobs, &sched.snapshot(), &env.home, at);
    assert_eq!(result[0].2.snapshot().context_tokens, 1_200);
}

#[test]
fn two_opencode_panes_share_path_and_window_but_never_usage() {
    let mut p = dummy_pane();
    apply_pane_tmux_title(&mut p, "%7|work|0:code|opencode|/repo");
    p.telemetry.vte_fallback.context_tokens = 71_000;
    apply_pane_tmux_title(&mut p, "%9|work|0:code|opencode|/repo");
    assert_eq!(pane_tmux_window(&p).as_deref(), Some("%9"));
    assert!(!p.telemetry.snapshot().has_data());
}

#[test]
fn disconnect_discards_previous_screen_and_agent() {
    let mut p = dummy_pane();
    p.proc_info.ssh_host = "host-a".into();
    p.tmux_pane = Some("%3".into());
    p.telemetry.vte_fallback.context_tokens = 71_000;
    p.sync_connection(&ProcInfo::default());
    assert!(p.tmux_pane.is_none() && !p.telemetry.snapshot().has_data());
}

#[test]
fn tmux_window_named_after_agent_clears_badge_and_status_when_shell_returns() {
    let mut p = dummy_pane();
    apply_pane_tmux_title(&mut p, "%4|work|0:codex|codex|/repo");
    assert_eq!(p.effective_agent(), "Codex");
    p.set_approval(Some("Allow?".into()), true, Instant::now());
    apply_pane_tmux_title(&mut p, "%4|work|0:codex|zsh|/repo");
    assert!(p.effective_agent().is_empty() && p.approval_snippet.is_none() && !p.unread);
    assert_eq!(p.state, crate::session::WorkState::Idle);
}

#[test]
fn cumulative_codex_input_cannot_become_current_context() {
    let mut tail = crate::telemetry::TranscriptTailer::default();
    tail.kind = TelemetrySource::Codex;
    tail.consume_line(r#"{"type":"token_count","info":{"total_token_usage":{"input_tokens":2000000,"output_tokens":20},"model_context_window":200000}}"#);
    assert_eq!(tail.snapshot.context_tokens, 0);
    assert_eq!(tail.snapshot.context_pct, None);
    assert!(tail.snapshot.usage_badge().is_none());
}

#[test]
fn opencode_reads_cache_fraction_and_distinguishes_missing_counter() {
    let mut snap = TokenSnapshot::default();
    crate::telemetry::opencode::apply_usage(
        r#"{"input":200,"read":800,"context":1000,"last_read":800}"#,
        &mut snap,
    );
    assert_eq!(snap.usage_badge().as_deref(), Some("ctx 1k · cache 80%"));
    crate::telemetry::opencode::apply_usage(
        r#"{"input":400,"read":800,"context":200,"last_read":0}"#,
        &mut snap,
    );
    assert_eq!(snap.usage_badge().as_deref(), Some("ctx 200 · cache 0%"));
    crate::telemetry::opencode::apply_usage(r#"{"input":500,"read":800,"context":100}"#, &mut snap);
    assert_eq!(snap.usage_badge().as_deref(), Some("ctx 100"));
}

#[test]
fn late_response_after_switch_cannot_restore_previous_context() {
    let mut state = SchedulerState::new(REMOTE_PROBE_TTL);
    let target = RemoteSessionTarget {
        sid: 14,
        host: "host-a".into(),
        tmux_window: Some("%1".into()),
        ..Default::default()
    };
    state.sync_targets(std::slice::from_ref(&target));
    let first = state.build_queries_for_host("host-a");
    state.sync_targets(&[RemoteSessionTarget { tmux_window: Some("%2".into()), ..target }]);
    assert!(!state.is_current_generation(&first, 14));
}

#[test]
fn empty_remote_shell_does_not_inherit_another_ssh_clients_agent() {
    let env = tests::MockRemoteEnv::new("ssh-ports");
    write_opencode_fd(&env, 3200, 50001);
    let shell = env.add_proc(3201, "-zsh\0", &env.home, &[]);
    write_ssh_env(&shell, 50002);
    fs::write(shell.join("stat"), "3201 (zsh) S 1 3201 3201 34816 3201").unwrap();
    let sched = RemoteTelemetryScheduler::default();
    sched.sync_targets(&[plain_port_target(1, 50001), plain_port_target(2, 50002)]);
    refresh_mock(&sched, &env, Instant::now());
    let snap = sched.snapshot();
    assert_eq!(snap[&1].snapshot.source, TelemetrySource::OpenCode);
    assert!(!snap[&2].snapshot.has_data() && snap[&2].foreground_shell);
}

#[test]
fn same_directory_claude_panes_use_only_their_own_transcript() {
    let env = tests::MockRemoteEnv::new("claude-pane-fds");
    let dir = env.home.join(".claude/projects/work");
    fs::create_dir_all(&dir).unwrap();
    write_claude_fd(&env, &dir, 6101, 300);
    write_claude_fd(&env, &dir, 6102, 700);
    let panes = format!(
        "s|1|1|6101|claude|{0}|0|work|%1\ns|1|0|6102|claude|{0}|0|work|%2\n",
        env.home.display()
    );
    fs::write(&env.tmux_panes_file, panes).unwrap();
    let sched = RemoteTelemetryScheduler::default();
    sched.sync_targets(&[tmux_target(1, "box", "s", "%1"), tmux_target(2, "box", "s", "%2")]);
    refresh_mock(&sched, &env, Instant::now());
    let snap = sched.snapshot();
    assert_eq!(snap[&1].snapshot.context_tokens, 300);
    assert_eq!(snap[&2].snapshot.context_tokens, 700);
}

#[test]
fn managed_tmux_shell_title_keeps_context_of_the_actual_foreground_process() {
    let now = Instant::now();
    let env = tests::MockRemoteEnv::new("managed-shell-title");
    let _ = isolated_ssh_config(&env, "ProxyCommand test-peer", 1_200);
    let shell = env.add_proc(4100, "-zsh\0", &env.home, &[4200]);
    let agent = env.proc_dir.join("4200");
    write_managed_shell_stats(&shell, &agent, 4200);
    fs::write(&env.tmux_panes_file, format!("work|1|1|4100|zsh|{}|0|work|%4\n", env.home.display())).unwrap();
    let mut session = tests::dummy_session(7, SessionCategory::Remote, Tab::remote("host-a", Some("work")), "host-a");
    let pane_id = session.pane_registry.register(ptr::null_mut(), ptr::null_mut(), 1, String::new());
    let sched = RemoteTelemetryScheduler::default();
    assert_managed_context(&mut session, pane_id, &env, &sched, None, now);
    assert_managed_context(&mut session, pane_id, &env, &sched, Some("v2:51:%4|work|0:work|zsh|/repo"), now + Duration::from_secs(1));
    write_managed_shell_stats(&shell, &agent, 4100);
    refresh_mock(&sched, &env, now + REMOTE_PROBE_TTL + Duration::from_secs(1));
    let tailer = sched.snapshot().into_values().next().unwrap();
    assert!(tailer.foreground_shell && !tailer.snapshot.has_data());
}

fn live_ssh_target(sid: usize, ssh_pid: i32) -> RemoteSessionTarget {
    RemoteSessionTarget {
        sid,
        ssh_pid,
        host: "akhterm-fixture".into(),
        tmux_session: Some("work".into()),
        tmux_window: Some("%4".into()),
        agent_hint: "codex".into(),
        ..Default::default()
    }
}

#[test]
#[ignore = "requires the isolated stdio OpenSSH test peer"]
fn scheduler_reads_tmux_usage_through_each_live_ssh_config() {
    use crate::telemetry::ssh_connection::FixtureClient;
    let base = fs::read_to_string(std::env::var_os("AKHTERM_TEST_SSH_CONFIG").unwrap()).unwrap();
    let a = tests::MockRemoteEnv::new("tmux-config-a");
    let b = tests::MockRemoteEnv::new("tmux-config-b");
    let peer_a = FixtureClient::start(&isolated_ssh_config(&a, &base, 1_200));
    let peer_b = FixtureClient::start(&isolated_ssh_config(&b, &base, 2_400));
    let _ = (peer_a.route(), peer_b.route());
    let sched = RemoteTelemetryScheduler::default();
    sched.sync_targets(&[live_ssh_target(1, peer_a.pid()), live_ssh_target(2, peer_b.pid())]);
    assert_eq!(sched.refresh_due(Instant::now()), 2);
    let results = sched.snapshot();
    assert_eq!(results[&1].snapshot.context_tokens, 1_200);
    assert_eq!(results[&2].snapshot.context_tokens, 2_400);
}
