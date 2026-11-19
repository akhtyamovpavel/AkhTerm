//! Build the bounded transcript query sent over each SSH transport.
use super::{
    posix_quote, source_kind_slug, ssh_connection, HostBatchJob, SessionProbeQuery,
    REMOTE_FULL_MAX_BYTES, REMOTE_HEAD_BYTES, REMOTE_TAIL_BYTES,
};
use ssh_connection::SshRoute;
use std::{collections::HashMap, process::Stdio};

type VersionedQuery = (u64, SessionProbeQuery);

fn run_route_probe(
    route: &SshRoute,
    queries: &[VersionedQuery],
    apply: &mut impl FnMut(&[VersionedQuery], &[u8]) -> usize,
) -> usize {
    let script = format!("sh -c {}", super::shell_quote(&probe_script(queries)));
    let output = route
        .spawn(&script, Stdio::null(), "10")
        .and_then(|child| child.wait_with_output());
    match output {
        Ok(out) if out.status.success() => apply(queries, &out.stdout),
        _ => 0,
    }
}

pub(super) fn probe_host(
    batch: HostBatchJob,
    mut apply: impl FnMut(&[VersionedQuery], &[u8]) -> usize,
) -> usize {
    group_routes(batch, route_for_probe)
        .into_iter()
        .map(|(route, queries)| run_route_probe(&route, &queries, &mut apply))
        .sum()
}

fn route_for_probe(host: &str, pid: i32) -> Option<SshRoute> {
    if pid > 0 {
        return SshRoute::from_client(pid, host);
    }
    SshRoute::for_host(host)
}

fn group_routes(
    batch: HostBatchJob,
    route: impl Fn(&str, i32) -> Option<SshRoute>,
) -> HashMap<SshRoute, Vec<VersionedQuery>> {
    let mut groups: HashMap<_, Vec<_>> = HashMap::new();
    for entry in batch.queries {
        if let Some(key) = route(&batch.host, entry.1.ssh_pid) {
            groups.entry(key).or_default().push(entry);
        }
    }
    groups
}

pub(super) fn probe_script(queries: &[VersionedQuery]) -> String {
    let plain: Vec<_> = queries.iter().map(|(_, query)| query.clone()).collect();
    build_posix_probe_script(&plain)
}

fn push_slice_limits(out: &mut String) {
    out.push_str(&format!(
        "AKHTERM_MAX_FULL={}; AKHTERM_HEAD_B={}; AKHTERM_TAIL_B={}\n",
        REMOTE_FULL_MAX_BYTES, REMOTE_HEAD_BYTES, REMOTE_TAIL_BYTES
    ));
}

fn push_probe_scripts(out: &mut String) {
    push_slice_limits(out);
    out.push_str(include_str!("agent_probe.sh"));
    out.push_str(include_str!("tmux_probe.sh"));
    out.push_str(include_str!("ssh_probe.sh"));
}

fn push_tmux_preamble(out: &mut String, queries: &[SessionProbeQuery]) {
    if !queries.is_empty() {
        out.push_str(posix_tmux_snapshot_line());
    }
    if queries.iter().any(|q| q.tmux_session.is_none()) {
        out.push_str("TMUX_PIDS=$(tree_pids \"$(tmux_roots)\")\n");
    }
}

fn build_posix_probe_script(queries: &[SessionProbeQuery]) -> String {
    let mut out = String::with_capacity(4864 + queries.len() * 192);
    push_probe_scripts(&mut out);
    push_tmux_preamble(&mut out, queries);
    for q in queries {
        append_query_call(&mut out, q);
    }
    out
}

fn posix_tmux_snapshot_line() -> &'static str {
    "TMUX_PANES=$(tmux list-panes -a -F '#{session_name}|#{window_active}|#{pane_active}|#{pane_pid}|#{pane_current_command}|#{pane_current_path}|#{window_index}|#{window_name}|#{pane_id}' 2>/dev/null || true)\n"
}

fn append_tmux_query(out: &mut String, q: &SessionProbeQuery, tmux: &str) {
    let tsess = posix_quote(tmux);
    let twin = posix_quote(q.tmux_window.as_deref().unwrap_or(""));
    let cwd = posix_quote(&q.pane_cwd_hint);
    let hint = posix_quote(&q.agent_hint);
    let ppath = posix_quote(&q.prev_path);
    let pkind = posix_quote(source_kind_slug(q.prev_kind));
    out.push_str(&format!(
        "probe_tmux {} {tsess} {twin} {cwd} {hint} {ppath} {pkind} {}\n",
        q.sid, q.prev_offset
    ));
}

fn append_plain_query(out: &mut String, q: &SessionProbeQuery) {
    let ppath = posix_quote(&q.prev_path);
    let (client, server) = q
        .ports
        .or_else(|| ssh_connection::ports(q.ssh_pid))
        .unwrap_or_default();
    out.push_str(&format!(
        "probe_plain {} {client} {server} {ppath} {}\n",
        q.sid, q.prev_offset
    ));
}

fn append_query_call(out: &mut String, q: &SessionProbeQuery) {
    if let Some(tmux) = &q.tmux_session {
        append_tmux_query(out, q, tmux);
    } else {
        append_plain_query(out, q);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_query(sid: usize, ssh_pid: i32) -> VersionedQuery {
        (
            1,
            SessionProbeQuery {
                sid,
                ssh_pid,
                ports: None,
                tmux_session: Some("work".into()),
                tmux_window: Some("%4".into()),
                pane_cwd_hint: String::new(),
                agent_hint: String::new(),
                prev_path: String::new(),
                prev_kind: super::super::TelemetrySource::None,
                prev_offset: 0,
            },
        )
    }

    fn route_for_port(host: &str, port: i32) -> Option<SshRoute> {
        let args = ["ssh", "-t", "-p", &port.to_string(), host, "tmux"].map(str::to_owned);
        SshRoute::from_argv(&args, PathBuf::from("/tmp"))
    }

    #[test]
    fn routes_with_distinct_ssh_ports_are_probed_separately() {
        let batch = HostBatchJob {
            host: "same-alias".into(),
            queries: vec![
                sample_query(1, 2201),
                sample_query(2, 2202),
                sample_query(3, 2201),
            ],
        };
        let groups = group_routes(batch, route_for_port);
        let mut sids: Vec<_> = groups
            .values()
            .map(|queries| queries.iter().map(|(_, q)| q.sid).collect::<Vec<_>>())
            .collect();
        sids.sort();
        assert_eq!(sids, vec![vec![1, 3], vec![2]]);
    }
}
