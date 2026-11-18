//! Decode the tmux server, focused pane and directory from the terminal title.
use crate::pane_registry::PaneRecord;

pub const TMUX_TITLE_FORMAT: &str =
    "AKHTERM_TMUX:v2:#{pid}:#{pane_id}|#{session_name}|#{window_index}:#{window_name}|#{pane_current_command}|#{pane_current_path}";

type PaneDetails = (String, String, String, String);

struct PaneTitle<'a> {
    server: Option<u32>,
    pane: Option<&'a str>,
    session: &'a str,
    window: &'a str,
    command: &'a str,
    path: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_prefixes_preserve_legacy_sessions_and_paths_with_pipes() {
        for (prefix, server, pane) in [
            ("", None, None),
            ("%7|", None, Some("%7")),
            ("v2:41:%7|", Some(41), Some("%7")),
        ] {
            let raw = format!("{prefix}work|0:edit|opencode|/srv/a|b");
            let title = PaneTitle::parse(&raw).unwrap();
            assert_eq!((title.server, title.pane), (server, pane));
            assert_eq!(title.path, "/srv/a|b");
        }
        let legacy = PaneTitle::parse("%7|0:edit|bash|/srv").unwrap();
        assert_eq!((legacy.session, legacy.pane), ("%7", None));
    }
}

impl<'a> PaneTitle<'a> {
    fn parse(mut fields: &'a str) -> Option<Self> {
        let server = take_server(&mut fields);
        let pane = take_pane(&mut fields);
        let mut parts = fields.splitn(4, '|').map(str::trim);
        let session = parts.next()?;
        if session.is_empty() {
            return None;
        }
        Some(Self {
            server,
            pane,
            session,
            window: parts.next()?,
            command: parts.next()?,
            path: parts.next()?,
        })
    }

    fn details(&self) -> PaneDetails {
        (
            self.session.into(),
            self.window.into(),
            self.command.into(),
            self.path.into(),
        )
    }

    fn same_process(&self, stored: &PaneDetails) -> bool {
        self.session == stored.0 && self.window == stored.1 && self.command == stored.2
    }

    fn matches(&self, pane: &PaneRecord) -> bool {
        self.pane.is_some()
            && self.pane == pane.tmux_pane.as_deref()
            && self.server == pane.tmux_server
            && pane
                .remote_tmux
                .as_ref()
                .is_some_and(|d| self.same_process(d) && self.path == d.3)
    }
}

fn take_server(fields: &mut &str) -> Option<u32> {
    let (pid, rest) = fields.strip_prefix("v2:")?.split_once(':')?;
    let server = pid.parse().ok()?;
    *fields = rest;
    Some(server)
}

fn take_pane<'a>(fields: &mut &'a str) -> Option<&'a str> {
    let (id, rest) = fields.split_once('|')?;
    let digits = id.strip_prefix('%')?;
    if digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || rest.split('|').nth(3).is_none()
    {
        return None;
    }
    *fields = rest;
    Some(id)
}

pub fn parse_tmux_vte_title(raw: &str) -> Option<PaneDetails> {
    Some(PaneTitle::parse(raw)?.details())
}

pub fn title_matches_pane(title: &str, pane: &PaneRecord) -> bool {
    title
        .strip_prefix("AKHTERM_TMUX:")
        .and_then(PaneTitle::parse)
        .is_some_and(|t| t.matches(pane))
}

pub fn apply_pane_tmux_title(pane: &mut PaneRecord, raw: &str) {
    let Some(title) = PaneTitle::parse(raw) else {
        return;
    };
    let changed = pane
        .remote_tmux
        .as_ref()
        .is_some_and(|old| !title.same_process(old));
    if changed || pane.tmux_pane.as_deref() != title.pane {
        pane.clear_finished_agent();
        pane.proc_info.agent.clear();
    }
    pane.ssh_directory = None;
    pane.tmux_server = title.server;
    pane.tmux_pane = title.pane.map(str::to_owned);
    title.session.clone_into(&mut pane.proc_info.tmux_session);
    title.window.clone_into(&mut pane.proc_info.tmux_window);
    pane.remote_tmux = Some(title.details());
}
