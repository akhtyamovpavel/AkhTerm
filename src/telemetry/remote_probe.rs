//! Build the bounded transcript query sent over each SSH transport.
use super::{
    posix_quote, source_kind_slug, ssh_connection, HostBatchJob, SessionProbeQuery, REMOTE_FULL_MAX_BYTES,
    REMOTE_HEAD_BYTES, REMOTE_TAIL_BYTES,
};
use ssh_connection::SshRoute;
use std::{collections::HashMap, process::Stdio};

type VersionedQuery = (u64, SessionProbeQuery);

pub(super) fn probe_host(
    batch: HostBatchJob,
    mut apply: impl FnMut(&[VersionedQuery], &[u8]) -> usize,
) -> usize {
    group_routes(batch, route_for_probe).into_iter().map(|(route, queries)| {
        let script = format!("sh -c {}", super::shell_quote(&probe_script(&queries)));
        let output = route.spawn(&script, Stdio::null(), "10")
            .and_then(|child| child.wait_with_output());
        match output {
            Ok(out) if out.status.success() => apply(&queries, &out.stdout),
            _ => 0,
        }
    }).sum()
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

fn build_posix_probe_script(queries: &[SessionProbeQuery]) -> String {
    let mut out = String::with_capacity(4864 + queries.len() * 192);
    for chunk in posix_core_helpers() {
        out.push_str(chunk);
    }
    for chunk in posix_agent_resolvers() {
        out.push_str(chunk);
    }
    push_emit_slice_fn(&mut out);
    out.push_str(include_str!("tmux_probe.sh"));
    out.push_str(include_str!("ssh_probe.sh"));
    if !queries.is_empty() {
        out.push_str(posix_tmux_snapshot_line());
    }
    if queries.iter().any(|q| q.tmux_session.is_none()) {
        out.push_str("TMUX_PIDS=$(tree_pids \"$(tmux_roots)\")\n");
    }
    for q in queries {
        append_query_call(&mut out, q);
    }
    out
}

fn posix_core_helpers() -> [&'static str; 4] {
    [
        posix_preamble_fn(),
        posix_proc_kind_fn(),
        posix_tree_pids_fn(),
        posix_tree_agent_fn(),
    ]
}

fn posix_agent_resolvers() -> [&'static str; 11] {
    [
        posix_newest_in_fn(),
        posix_resolve_claude_fn(),
        posix_codex_fd_fn(),
        posix_codex_anc_fn(),
        posix_resolve_codex_fn(),
        posix_ws_match_fn(),
        posix_agy_is_sub_fn(),
        posix_agy_pids_fn(),
        posix_agy_fd_fn(),
        posix_resolve_agy_fn(),
        posix_resolve_kind_fn(),
    ]
}

fn posix_preamble_fn() -> &'static str {
    concat!(
        "LC_ALL=C; export LC_ALL\n",
        "P_ROOT=\"${AKHTERM_TEST_PROC:-/proc}\"\n",
        "detect_kind() {\n",
        "  case \"$1\" in ssh|sshd) return 1 ;; agy) printf 'agy'; return 0 ;; ",
        "opencode) printf 'opencode'; return 0 ;; codex|codex-*) printf 'codex'; return 0 ;; claude|claude-*) printf 'claude'; return 0 ;; esac\n",
        "  case \"$2\" in *akhterm_rtelem*) return 1 ;; ",
        "*/agy|*/agy\\ *|agy\\ *|*antigravity*) printf 'agy'; return 0 ;; ",
        "*codex*) printf 'codex'; return 0 ;; *claude*) printf 'claude'; return 0 ;; esac\n",
        "  return 1\n}\n",
    )
}

fn posix_proc_kind_fn() -> &'static str {
    concat!(
        "proc_kind() {\n",
        "  _p=\"$1\"; [ \"$_p\" = \"$$\" ] && return 1; [ \"$_p\" = \"${PPID:-0}\" ] && return 1\n",
        "  [ -r \"$P_ROOT/$_p/cmdline\" ] || [ -r \"$P_ROOT/$_p/comm\" ] || return 1\n",
        "  _raw=$(tr '\\000\\n\\t' '   ' < \"$P_ROOT/$_p/cmdline\" 2>/dev/null)\n",
        "  _cm=$(tr -d '\\n\\r' < \"$P_ROOT/$_p/comm\" 2>/dev/null)\n",
        "  [ -z \"$_raw\" ] && _raw=\"$_cm\"\n",
        "  _low=$(printf '%s' \"$_raw\" | tr 'A-Z' 'a-z')\n",
        "  _f1=\"${_low%% *}\"; _ex=\"${_f1##*/}\"\n",
        "  [ -z \"$_ex\" ] && _ex=$(printf '%s' \"$_cm\" | tr 'A-Z' 'a-z')\n",
        "  detect_kind \"$_ex\" \"$_low\"\n}\n",
    )
}

fn posix_tree_pids_fn() -> &'static str {
    concat!(
        "tree_pids() {\n",
        "  _q=\"$1\"; _sn=\" \"; _n=0; _ps=\"\"\n",
        "  if [ \"$P_ROOT\" = \"/proc\" ] && [ -n \"$_q\" ]; then\n",
        "    _f1=\"${_q%% *}\"; set -- \"$P_ROOT/$_f1/task\"/*/children\n",
        "    [ -r \"$1\" ] || [ -r \"$P_ROOT/$_f1/children\" ] || _ps=$(ps -eo pid=,ppid= 2>/dev/null || true)\n",
        "  fi\n",
        "  while [ -n \"$_q\" ] && [ \"$_n\" -lt 64 ]; do\n",
        "    _c=\"${_q%% *}\"\n",
        "    if [ \"$_c\" = \"$_q\" ]; then _q=\"\"; else _q=\"${_q#* }\"; fi\n",
        "    case \"$_c\" in ''|*[!0-9]*) continue ;; esac\n",
        "    case \"$_sn\" in *\" $_c \"*) continue ;; esac\n",
        "    _sn=\"$_sn$_c \"; _n=$((_n + 1))\n",
        "    for _cf in \"$P_ROOT/$_c/task\"/*/children \"$P_ROOT/$_c/children\"; do\n",
        "      [ -r \"$_cf\" ] || continue\n",
        "      for _kp in $(tr '\\n\\t' '  ' < \"$_cf\" 2>/dev/null); do _q=\"${_q:+$_q }$_kp\"; done\n",
        "    done\n",
        "    if [ -n \"$_ps\" ]; then\n",
        "      for _kp in $(printf '%s\\n' \"$_ps\" | awk -v p=\"$_c\" '$2==p{print $1}'); do _q=\"${_q:+$_q }$_kp\"; done\n",
        "    fi\n  done\n",
        "  printf '%s' \"$_sn\"\n}\n",
    )
}

fn posix_tree_agent_fn() -> &'static str {
    concat!(
        "find_tree_agent() {\n",
        "  for _c in $(tree_pids \"$1\"); do\n",
        "    if _k=$(proc_kind \"$_c\"); then printf '%s|%s' \"$_c\" \"$_k\"; return 0; fi\n",
        "  done; return 1\n}\n",
    )
}

fn posix_newest_in_fn() -> &'static str {
    concat!(
        "newest_in() {\n",
        "  _b=\"\"\n",
        "  for _x in \"$1\"/*.jsonl; do\n",
        "    [ -f \"$_x\" ] || continue\n",
        "    [ -z \"$_b\" ] || [ \"$_x\" -nt \"$_b\" ] && _b=\"$_x\"\n",
        "  done\n  [ -n \"$_b\" ] && printf '%s' \"$_b\"\n}\n",
    )
}

fn posix_resolve_claude_fn() -> &'static str {
    concat!(
        "resolve_claude() {\n",
        "  _cur=\"${1%/}\"; _pj=\"$HOME/.claude/projects\"; [ -d \"$_pj\" ] || return 1\n",
        "  _i=0\n",
        "  while [ \"$_i\" -le 3 ]; do\n",
        "    [ -z \"$_cur\" ] || [ \"$_cur\" = \"/\" ] || [ \"$_cur\" = \"$HOME\" ] && break\n",
        "    _sl=$(printf '%s' \"$_cur\" | tr '/.' '--')\n",
        "    _hit=$(newest_in \"$_pj/$_sl\")\n",
        "    if [ -n \"$_hit\" ]; then printf '%s' \"$_hit\"; return 0; fi\n",
        "    _pa=\"${_cur%/*}\"; [ \"$_pa\" = \"$_cur\" ] && break\n",
        "    _cur=\"$_pa\"; _i=$((_i + 1))\n  done\n  return 1\n}\n",
    )
}

fn posix_codex_fd_fn() -> &'static str {
    concat!(
        "codex_is_sub() {\n",
        "  case \"$1\" in *',\"source\":{\"subagent'*|*', \"source\": {\"subagent'*|*'{\"source\":{\"subagent'*|*'{\"source\": {\"subagent'*) return 0 ;; esac\n",
        "  return 1\n}\n",
        "codex_fd() {\n",
        "  [ -n \"$1\" ] || return 1; _sr=\"${CODEX_HOME:-$HOME/.codex}/sessions\"\n",
        "  for _cp in $(tree_pids \"$1\"); do\n",
        "    [ -d \"$P_ROOT/$_cp/fd\" ] || continue\n",
        "    for _fd in \"$P_ROOT/$_cp/fd\"/*; do\n",
        "      _t=$(readlink \"$_fd\" 2>/dev/null) || continue\n",
        "      case \"$_t\" in \"$_sr/\"*.jsonl) [ -f \"$_t\" ] || continue\n",
        "        _hd=$(head -c 32768 \"$_t\" 2>/dev/null); codex_is_sub \"$_hd\" && continue\n",
        "        printf '%s' \"$_t\"; return 0 ;; esac\n",
        "    done\n  done; return 1\n}\n",
    )
}

fn posix_codex_anc_fn() -> &'static str {
    concat!(
        "codex_anc() {\n",
        "  _hd=\"$1\"; _an=\"$2\"; _d=0\n",
        "  while [ \"$_d\" -lt 3 ]; do\n",
        "    _pa=\"${_an%/*}\"; [ -z \"$_pa\" ] || [ \"$_pa\" = \"$_an\" ] || [ \"$_pa\" = \"/\" ] || [ \"$_pa\" = \"$HOME\" ] && break\n",
        "    _an=\"$_pa\"; _d=$((_d + 1))\n",
        "    case \"$_hd\" in *'\"cwd\":\"'\"$_an\"'\"'*|*'\"cwd\": \"'\"$_an\"'\"'*) return 0 ;; esac\n",
        "  done; return 1\n}\n",
    )
}

fn posix_resolve_codex_fn() -> &'static str {
    concat!(
        "codex_has_tok() { tail -c 65536 \"$1\" 2>/dev/null | grep -q '\"token_count\"'; }\n",
        "resolve_codex() {\n",
        "  _cw=\"${1%/}\"; _pd=\"$2\"\n",
        "  _hit=$(codex_fd \"$_pd\") && [ -n \"$_hit\" ] && { printf '%s' \"$_hit\"; return 0; }\n",
        "  _sr=\"${CODEX_HOME:-$HOME/.codex}/sessions\"; [ -d \"$_sr\" ] || return 1\n",
        "  set --; for _g in \"$_sr\"/*/*/*/*.jsonl \"$_sr\"/*.jsonl; do [ -f \"$_g\" ] && set -- \"$@\" \"$_g\"; done\n",
        "  [ \"$#\" -gt 0 ] || return 1\n",
        "  _lst=$(ls -1t \"$@\" 2>/dev/null | head -n 48)\n",
        "  _bet=\"\"; _be=\"\"; _bst=\"\"; _bs=\"\"; _ba=\"\"; _oifs=\"$IFS\"; IFS='\n'\n",
        "  set -f\n",
        "  for _f in $_lst; do\n",
        "    IFS=\"$_oifs\"; set +f; [ -f \"$_f\" ] || continue\n",
        "    _hd=$(head -c 32768 \"$_f\" 2>/dev/null); codex_is_sub \"$_hd\" && continue\n",
        "    if [ -z \"$_cw\" ]; then [ -z \"$_ba\" ] && _ba=\"$_f\"; codex_has_tok \"$_f\" && { _ba=\"$_f\"; break; }; continue; fi\n",
        "    case \"$_hd\" in *'\"cwd\":\"'\"$_cw\"'\"'*|*'\"cwd\": \"'\"$_cw\"'\"'*)\n",
        "      [ -z \"$_be\" ] && _be=\"$_f\"\n",
        "      codex_has_tok \"$_f\" && { _bet=\"$_f\"; break; }; continue ;; esac\n",
        "    if [ -z \"$_bst\" ] && codex_anc \"$_hd\" \"$_cw\"; then\n",
        "      [ -z \"$_bs\" ] && _bs=\"$_f\"; codex_has_tok \"$_f\" && _bst=\"$_f\"\n    fi\n",
        "  done\n",
        "  IFS=\"$_oifs\"; set +f\n",
        "  for _r in \"$_bet\" \"$_be\" \"$_bst\" \"$_bs\"; do [ -n \"$_r\" ] && { printf '%s' \"$_r\"; return 0; }; done\n",
        "  [ -z \"$_cw\" ] && [ -n \"$_ba\" ] && { printf '%s' \"$_ba\"; return 0; }\n  return 1\n}\n",
    )
}

fn posix_ws_match_fn() -> &'static str {
    concat!(
        "ws_match() {\n",
        "  _tx=\"$1\"; _ws=\"${2%/}\"; [ -n \"$_ws\" ] || return 2; _sb=1\n",
        "  while :; do\n",
        "    case \"$_tx\" in *\"$_ws\"*)\n",
        "      _tx=\"${_tx#*\"$_ws\"}\"; _ch=$(printf '%.1s' \"$_tx\")\n",
        "      case \"$_ch\" in /) _sb=0 ;; [A-Za-z0-9_.-]) ;; *) return 0 ;; esac ;;\n",
        "      *) break ;; esac\n  done\n",
        "  [ \"$_sb\" -eq 0 ] && return 1\n  return 2\n}\n",
    )
}

fn posix_agy_is_sub_fn() -> &'static str {
    concat!(
        "agy_is_sub() {\n",
        "  [ -n \"$2\" ] || return 1\n",
        "  for _sf in \"$1\"/*/.system_generated/subagents/\"$2\".json; do\n",
        "    [ -f \"$_sf\" ] && return 0\n  done\n  return 1\n}\n",
    )
}

fn posix_agy_pids_fn() -> &'static str {
    concat!(
        "agy_pids() {\n",
        "  _pl=\"$1\"\n",
        "  for _cf in \"$P_ROOT/$1/task\"/*/children \"$P_ROOT/$1/children\"; do\n",
        "    [ -r \"$_cf\" ] && _pl=\"$_pl $(tr '\\n\\t' '  ' < \"$_cf\" 2>/dev/null)\"\n  done\n",
        "  printf '%s' \"$_pl\"\n}\n",
    )
}

fn posix_agy_fd_fn() -> &'static str {
    concat!(
        "agy_fd() {\n",
        "  _pd=\"$1\"; _br=\"$2\"; [ -n \"$_pd\" ] || return 1; _fb=\"\"\n",
        "  for _ap in $(agy_pids \"$_pd\"); do\n",
        "    [ -d \"$P_ROOT/$_ap/fd\" ] || continue\n",
        "    for _fd in \"$P_ROOT/$_ap/fd\"/*; do\n",
        "      _t=$(readlink \"$_fd\" 2>/dev/null) || continue\n",
        "      case \"$_t\" in \"$_br/\"*) _rl=\"${_t#\"$_br/\"}\"; _id=\"${_rl%%/*}\"; [ -n \"$_id\" ] || continue\n",
        "        agy_is_sub \"$_br\" \"$_id\" && continue\n",
        "        _cd=\"$_br/$_id/.system_generated/logs/transcript.jsonl\"\n",
        "        [ -f \"$_cd\" ] || _cd=\"$_br/$_id/transcript.jsonl\"; [ -f \"$_cd\" ] || continue\n",
        "        case \"${_t##*/}\" in transcript*) printf '%s' \"$_cd\"; return 0 ;; esac\n",
        "        [ -z \"$_fb\" ] || [ \"$_cd\" -nt \"$_fb\" ] && _fb=\"$_cd\" ;; esac\n",
        "    done\n  done\n",
        "  [ -n \"$_fb\" ] && { printf '%s' \"$_fb\"; return 0; }\n  return 1\n}\n",
    )
}

fn posix_resolve_agy_fn() -> &'static str {
    concat!(
        "resolve_agy() {\n",
        "  _cw=\"${1%/}\"; _pd=\"$2\"; _br=\"$HOME/.gemini/antigravity-cli/brain\"\n",
        "  [ -d \"$_br\" ] || return 1\n",
        "  _hit=$(agy_fd \"$_pd\" \"$_br\") && [ -n \"$_hit\" ] && { printf '%s' \"$_hit\"; return 0; }\n",
        "  [ -n \"$_cw\" ] || return 1; _be=\"\"; _bs=\"\"\n",
        "  for _cd in \"$_br\"/*/.system_generated/logs/transcript.jsonl \"$_br\"/*/transcript.jsonl; do\n",
        "    [ -f \"$_cd\" ] || continue\n",
        "    _rl=\"${_cd#\"$_br/\"}\"; _id=\"${_rl%%/*}\"; agy_is_sub \"$_br\" \"$_id\" && continue\n",
        "    _hd=$(head -c 65536 \"$_cd\" 2>/dev/null)\n",
        "    _rc=0; ws_match \"$_hd\" \"$_cw\" || _rc=$?\n",
        "    [ \"$_rc\" -eq 0 ] && { [ -z \"$_be\" ] || [ \"$_cd\" -nt \"$_be\" ] && _be=\"$_cd\"; }\n",
        "    [ \"$_rc\" -eq 1 ] && { [ -z \"$_bs\" ] || [ \"$_cd\" -nt \"$_bs\" ] && _bs=\"$_cd\"; }\n",
        "  done\n",
        "  for _r in \"$_be\" \"$_bs\"; do [ -n \"$_r\" ] && { printf '%s' \"$_r\"; return 0; }; done\n  return 1\n}\n",
    )
}

fn posix_resolve_kind_fn() -> &'static str {
    concat!(
        "opencode_fd() {\n",
        "  for p in $(tree_pids \"$1\"); do\n",
        "    for fd in \"$P_ROOT/$p/fd/\"*; do\n",
        "      path=$(readlink \"$fd\" 2>/dev/null) || continue\n",
        "      case \"$path\" in */akhterm/opencode/client-*/ses_*.jsonl) [ -f \"$path\" ] && { printf '%s' \"$path\"; return 0; } ;; esac\n",
        "    done\n  done\n  return 1\n}\n",
        "resolve_for_kind() {\n",
        "  case \"$1\" in opencode) opencode_fd \"$3\" ;; claude) resolve_claude \"$2\" ;; codex) resolve_codex \"$2\" \"$3\" ;; ",
        "agy) resolve_agy \"$2\" \"$3\" ;; *) return 1 ;; esac\n}\n",
    )
}

fn push_emit_slice_fn(out: &mut String) {
    out.push_str(concat!(
        "emit_slice() {\n",
        "  _s=\"$1\"; _k=\"$2\"; _f=\"$3\"; _pp=\"$4\"; _po=\"$5\"\n",
        "  if [ -z \"$_f\" ] || [ ! -f \"$_f\" ]; then\n",
        "    printf 'AKHTERM_RTELEM1|%s|NONE|-|0|0|0|0|\\n\\nAKHTERM_RTELEM_END\\n' \"$_s\"; return 0\n  fi\n",
        "  _sz=$(wc -c < \"$_f\" 2>/dev/null | tr -d ' \\t\\r\\n'); [ -n \"$_sz\" ] || _sz=0\n",
        "  _mt=$(stat -c '%Y' \"$_f\" 2>/dev/null || stat -f '%m' \"$_f\" 2>/dev/null || echo 0)\n",
        "  _mt=$(printf '%s' \"$_mt\" | tr -d ' \\t\\r\\n'); [ -n \"$_mt\" ] || _mt=0\n",
    ));
    push_emit_initial_branch(out);
    push_emit_delta_tail(out);
}

fn push_emit_initial_branch(out: &mut String) {
    out.push_str(&format!(
        concat!(
            "  if [ \"$_f\" != \"$_pp\" ] || [ \"$_po\" -le 0 ] || [ \"$_sz\" -lt \"$_po\" ]; then\n",
            "    if [ \"$_sz\" -le {max_full} ]; then\n",
            "      printf 'AKHTERM_RTELEM1|%s|FULL|%s|%s|%s|0|%s|%s\\n' \"$_s\" \"$_k\" \"$_sz\" \"$_mt\" \"$_sz\" \"$_f\"\n",
            "      [ \"$_sz\" -gt 0 ] && head -c \"$_sz\" \"$_f\"; printf '\\nAKHTERM_RTELEM_END\\n'\n",
            "    else\n",
            "      printf 'AKHTERM_RTELEM1|%s|HEAD_TAIL|%s|%s|%s|{head_b}|{tail_b}|%s\\n' \"$_s\" \"$_k\" \"$_sz\" \"$_mt\" \"$_f\"\n",
            "      head -c {head_b} \"$_f\"; tail -c {tail_b} \"$_f\" | head -c {tail_b}; printf '\\nAKHTERM_RTELEM_END\\n'\n    fi\n",
        ),
        max_full = REMOTE_FULL_MAX_BYTES,
        head_b = REMOTE_HEAD_BYTES,
        tail_b = REMOTE_TAIL_BYTES,
    ));
}

fn push_emit_delta_tail(out: &mut String) {
    out.push_str(&format!(
        concat!(
            "  elif [ \"$_sz\" -eq \"$_po\" ]; then\n",
            "    printf 'AKHTERM_RTELEM1|%s|UNCHANGED|%s|%s|%s|0|0|%s\\n\\nAKHTERM_RTELEM_END\\n' \"$_s\" \"$_k\" \"$_sz\" \"$_mt\" \"$_f\"\n",
            "  else\n",
            "    _dl=$((_sz - _po))\n",
            "    if [ \"$_dl\" -le {max_full} ]; then\n",
            "      _st=$((_po + 1))\n",
            "      printf 'AKHTERM_RTELEM1|%s|DELTA|%s|%s|%s|0|%s|%s\\n' \"$_s\" \"$_k\" \"$_sz\" \"$_mt\" \"$_dl\" \"$_f\"\n",
            "      tail -c +\"$_st\" \"$_f\" | head -c \"$_dl\"; printf '\\nAKHTERM_RTELEM_END\\n'\n",
            "    else\n",
            "      printf 'AKHTERM_RTELEM1|%s|TAIL_DELTA|%s|%s|%s|0|{tail_b}|%s\\n' \"$_s\" \"$_k\" \"$_sz\" \"$_mt\" \"$_f\"\n",
            "      tail -c {tail_b} \"$_f\" | head -c {tail_b}; printf '\\nAKHTERM_RTELEM_END\\n'\n    fi\n  fi\n}}\n",
        ),
        max_full = REMOTE_FULL_MAX_BYTES,
        tail_b = REMOTE_TAIL_BYTES,
    ));
}

fn posix_tmux_snapshot_line() -> &'static str {
    "TMUX_PANES=$(tmux list-panes -a -F '#{session_name}|#{window_active}|#{pane_active}|#{pane_pid}|#{pane_current_command}|#{pane_current_path}|#{window_index}|#{window_name}|#{pane_id}' 2>/dev/null || true)\n"
}

fn append_query_call(out: &mut String, q: &SessionProbeQuery) {
    let (sid, poff) = (q.sid, q.prev_offset);
    let cwd = posix_quote(&q.pane_cwd_hint);
    let hint = posix_quote(&q.agent_hint);
    let ppath = posix_quote(&q.prev_path);
    let pkind = posix_quote(source_kind_slug(q.prev_kind));
    if let Some(tmux) = &q.tmux_session {
        let tsess = posix_quote(tmux);
        let twin = posix_quote(q.tmux_window.as_deref().unwrap_or(""));
        out.push_str(&format!(
            "probe_tmux {sid} {tsess} {twin} {cwd} {hint} {ppath} {pkind} {poff}\n"
        ));
    } else {
        let (client, server) = q
            .ports
            .or_else(|| ssh_connection::ports(q.ssh_pid))
            .unwrap_or_default();
        out.push_str(&format!(
            "probe_plain {sid} {client} {server} {ppath} {poff}\n"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn routes_with_distinct_ssh_ports_are_probed_separately() {
        let query = |sid, ssh_pid| {
            (1, SessionProbeQuery {
                sid, ssh_pid, ports: None, tmux_session: Some("work".into()),
                tmux_window: Some("%4".into()), pane_cwd_hint: String::new(),
                agent_hint: String::new(), prev_path: String::new(),
                prev_kind: super::super::TelemetrySource::None, prev_offset: 0,
            })
        };
        let batch = HostBatchJob {
            host: "same-alias".into(),
            queries: vec![query(1, 2201), query(2, 2202), query(3, 2201)],
        };
        let groups = group_routes(batch, |host, port| {
            let args = ["ssh", "-t", "-p", &port.to_string(), host, "tmux"]
                .map(str::to_owned);
            SshRoute::from_argv(&args, PathBuf::from("/tmp"))
        });
        let mut groups: Vec<_> = groups.values()
            .map(|queries| queries.iter().map(|(_, q)| q.sid).collect::<Vec<_>>())
            .collect();
        groups.sort();
        assert_eq!(groups, vec![vec![1, 3], vec![2]]);
    }
}
