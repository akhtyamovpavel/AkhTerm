use super::*;
use crate::pane_registry::{PaneId, PaneRecord};
use crate::proc_radar::ProcInfo;
use crate::telemetry::{TelemetrySource, TokenSnapshot};
use crate::tmux_local::{apply_pane_tmux_title, pane_tmux_window};
use std::{fs, ptr};

#[test]
fn two_opencode_panes_share_path_and_window_but_never_usage() {
    let mut p = PaneRecord::new(
        PaneId(1),
        ptr::null_mut(),
        ptr::null_mut(),
        1,
        "/repo".into(),
    );
    apply_pane_tmux_title(&mut p, "%7|work|0:code|opencode|/repo");
    p.telemetry.vte_fallback.context_tokens = 71_000;
    apply_pane_tmux_title(&mut p, "%9|work|0:code|opencode|/repo");
    assert_eq!(pane_tmux_window(&p).as_deref(), Some("%9"));
    assert!(!p.telemetry.snapshot().has_data());
}

#[test]
fn disconnect_discards_previous_screen_and_agent() {
    let mut p = PaneRecord::new(
        PaneId(1),
        ptr::null_mut(),
        ptr::null_mut(),
        1,
        "/repo".into(),
    );
    p.proc_info.ssh_host = "host-a".into();
    p.tmux_pane = Some("%3".into());
    p.telemetry.vte_fallback.context_tokens = 71_000;
    p.sync_connection(&ProcInfo::default());
    assert!(p.tmux_pane.is_none() && !p.telemetry.snapshot().has_data());
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
    let next = RemoteSessionTarget {
        tmux_window: Some("%2".into()),
        ..target
    };
    state.sync_targets(&[next]);
    assert!(!state.is_current_generation(&first, 14));
}

#[test]
fn empty_remote_shell_does_not_inherit_another_ssh_clients_agent() {
    let env = tests::MockRemoteEnv::new("ssh-ports");
    let cwd = &env.home;
    let pid = env.add_proc(3200, "opencode\0", cwd, &[]);
    fs::write(
        pid.join("environ"),
        b"SSH_CONNECTION=127.0.0.1 50001 127.0.0.1 22\0",
    )
    .unwrap();
    let dir = cwd.join(".local/state/akhterm/opencode/client-A");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ses_a.jsonl");
    fs::write(
        &file,
        "{\"input\":200,\"read\":800,\"context\":1000,\"last_read\":800}\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(file, pid.join("fd/3")).unwrap();
    let sched = RemoteTelemetryScheduler::default();
    let a = RemoteSessionTarget {
        sid: 1,
        host: "box".into(),
        ports: Some((50001, 22)),
        ..Default::default()
    };
    let b = RemoteSessionTarget {
        sid: 2,
        ports: Some((50002, 22)),
        ..a.clone()
    };
    sched.sync_targets(&[a, b]);
    sched.refresh_due_with(Instant::now(), |host, cmd| {
        run_ssh_probe_with_program(env.ssh_bin.to_str().unwrap(), host, cmd)
    });
    assert_eq!(
        sched.snapshot()[&1].snapshot.source,
        TelemetrySource::OpenCode
    );
    assert!(!sched.snapshot()[&2].snapshot.has_data());
}

#[test]
fn same_directory_claude_panes_use_only_their_own_transcript() {
    let env = tests::MockRemoteEnv::new("claude-pane-fds");
    let dir = env.home.join(".claude/projects/work");
    fs::create_dir_all(&dir).unwrap();
    for (pid, count) in [(6101, 300), (6102, 700)] {
        let proc = env.add_proc(pid, "claude\0", &env.home, &[]);
        let log = dir.join(format!("{pid}.jsonl"));
        let line = format!(
            r#"{{"type":"assistant","message":{{"id":"msg","usage":{{"input_tokens":{count},"output_tokens":2}}}}}}"#
        );
        fs::write(&log, format!("{line}\n")).unwrap();
        std::os::unix::fs::symlink(log, proc.join("fd/3")).unwrap();
    }
    let panes = format!(
        "s|1|1|6101|claude|{0}|0|work|%1\ns|1|0|6102|claude|{0}|0|work|%2\n",
        env.home.display()
    );
    fs::write(&env.tmux_panes_file, panes).unwrap();
    let scheduler = RemoteTelemetryScheduler::default();
    let first = RemoteSessionTarget {
        sid: 1,
        host: "box".into(),
        tmux_session: Some("s".into()),
        tmux_window: Some("%1".into()),
        ..Default::default()
    };
    let second = RemoteSessionTarget {
        sid: 2,
        tmux_window: Some("%2".into()),
        ..first.clone()
    };
    scheduler.sync_targets(&[first, second]);
    scheduler.refresh_due_with(Instant::now(), |host, cmd| {
        run_ssh_probe_with_program(env.ssh_bin.to_str().unwrap(), host, cmd)
    });
    let snap = scheduler.snapshot();
    assert_eq!(snap[&1].snapshot.context_tokens, 300);
    assert_eq!(snap[&2].snapshot.context_tokens, 700);
}
