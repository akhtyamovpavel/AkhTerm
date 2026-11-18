//! Decode the tmux server, focused pane and directory from the terminal title.
use crate::pane_registry::PaneRecord;

pub const TMUX_TITLE_FORMAT: &str =
    "AKHTERM_TMUX:v2:#{pid}:#{pane_id}|#{session_name}|#{window_index}:#{window_name}|#{pane_current_command}|#{pane_current_path}";
fn title_server(rest: &str) -> Option<(u32, &str)> {
    let (server, payload) = rest.strip_prefix("v2:")?.split_once(':')?;
    Some((server.parse().ok()?, payload))
}

fn title_target(rest: &str) -> (Option<&str>, &str) {
    let rest = title_server(rest).map_or(rest, |(_, payload)| payload);
    if let Some((id, payload)) = rest.split_once('|') {
        if id
            .strip_prefix('%')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
            && payload.split('|').count() >= 4
        {
            return (Some(id), payload);
        }
    }
    (None, rest)
}

pub fn parse_tmux_vte_title(rest: &str) -> Option<(String, String, String, String)> {
    let mut parts = title_target(rest).1.splitn(4, '|');
    let sess = parts.next()?.trim();
    if sess.is_empty() {
        return None;
    }
    let win = parts.next()?.trim().to_owned();
    let cmd = parts.next()?.trim().to_owned();
    let path = parts.next()?.trim().to_owned();
    Some((sess.to_owned(), win, cmd, path))
}

pub fn title_matches_pane(title: &str, pane: &PaneRecord) -> bool {
    let Some(rest) = title.strip_prefix("AKHTERM_TMUX:") else {
        return false;
    };
    pane.tmux_pane.is_some()
        && title_target(rest).0 == pane.tmux_pane.as_deref()
        && title_server(rest).map(|(pid, _)| pid) == pane.tmux_server
        && parse_tmux_vte_title(rest).as_ref() == pane.remote_tmux.as_ref()
}

pub fn apply_pane_tmux_title(pane: &mut PaneRecord, rest: &str) {
    let Some((sess, win, cmd, path)) = parse_tmux_vte_title(rest) else {
        return;
    };
    let switched = pane
        .remote_tmux
        .as_ref()
        .is_some_and(|(ps, pw, ..)| (ps.as_str(), pw.as_str()) != (sess.as_str(), win.as_str()));
    let target = title_target(rest).0.map(str::to_owned);
    if switched || target != pane.tmux_pane || pane.remote_tmux.as_ref().is_some_and(|t| t.2 != cmd)
    {
        pane.clear_finished_agent();
        pane.proc_info.agent.clear();
    }
    pane.ssh_directory = None;
    pane.tmux_server = title_server(rest).map(|(pid, _)| pid);
    pane.tmux_pane = target;
    pane.proc_info.tmux_session.clone_from(&sess);
    pane.proc_info.tmux_window.clone_from(&win);
    pane.remote_tmux = Some((sess, win, cmd, path));
}
