//! Identify the exact terminal pane displaying a document or an agent prompt.
use super::remote::{self, Connection, TmuxAddress};
use crate::{
    app_state,
    ffi::GtkWidget,
    pane_registry::PaneRecord,
    proc_radar::{self, ProcInfo},
    session::TermSession,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub probe: usize,
    ssh_pid: i32,
    local_pane: i32,
    tmux: Option<String>,
}

impl Origin {
    pub(super) fn new(pane: &PaneRecord, info: &ProcInfo) -> Self {
        Self {
            probe: pane.probe_id,
            ssh_pid: info.ssh_pid,
            local_pane: info.active_pane_pid,
            tmux: pane.tmux_pane.clone(),
        }
    }
}

pub fn for_vte(vte: *mut GtkWidget) -> Option<Origin> {
    app_state::with_app(|app| {
        let p = app
            .sessions
            .iter()
            .find_map(|s| s.pane_registry.by_vte(vte))?;
        Some(Origin::new(p, &proc_radar::inspect_proc_tree(p.child_pid)))
    })
    .flatten()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteDocument {
    pub origin: Origin,
    pub host: String,
    pub connection: Option<Connection>,
    pub path: PathBuf,
}

pub fn remote_document(vte: *mut GtkWidget, raw: &str) -> Option<RemoteDocument> {
    let path = remote::candidate(raw)?;
    app_state::with_app(|app| {
        let session = app.sessions.iter().find(|s| s.has_vte(vte))?;
        let p = session.pane_registry.by_vte(vte)?;
        let info = proc_radar::inspect_proc_tree(p.child_pid);
        let profile = session
            .panes
            .remote_for_vte(vte, session.scroll_wrap, session.vte)
            .or_else(|| (session.vte == vte).then(|| session.remote()).flatten());
        let host = if info.ssh_host.is_empty() {
            profile?.host.clone()
        } else {
            info.ssh_host.clone()
        };
        let title = crate::session_ops::read_vte_window_title(vte).unwrap_or_default();
        let cwd = remote_directory(p, &info, &title);
        Some(RemoteDocument {
            origin: Origin::new(p, &info),
            connection: Connection::from_client(info.ssh_pid, &host),
            host,
            path: remote::resolve(&path, cwd).unwrap_or(path),
        })
    })
    .flatten()
}

pub(super) fn remote_directory<'a>(
    pane: &'a PaneRecord,
    info: &ProcInfo,
    title: &str,
) -> Option<&'a Path> {
    if info.ssh_pid <= 0 {
        return None;
    }
    if title.starts_with("AKHTERM_TMUX:") {
        let tmux = pane.remote_tmux.as_ref()?;
        return (pane.proc_info.ssh_pid == info.ssh_pid
            && crate::tmux_local::title_matches_pane(title, pane))
        .then(|| Path::new(&tmux.3));
    }
    pane.ssh_directory
        .as_ref()
        .filter(|(pid, _)| *pid == info.ssh_pid)
        .map(|(_, cwd)| Path::new(cwd))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub origin: Origin,
    pub sid: usize,
    pub vte: *mut GtkWidget,
    pub label: String,
    foreground_group: i32,
    pub delivery: Option<(Connection, TmuxAddress)>,
}

impl Target {
    fn for_pane(s: &TermSession, p: &PaneRecord, live: bool) -> Option<Self> {
        if p.child_pid <= 0 {
            return None;
        }
        let fresh;
        let info = if live {
            fresh = proc_radar::inspect_proc_tree(p.child_pid);
            &fresh
        } else {
            &p.proc_info
        };
        let remote = info.ssh_pid > 0;
        if info.shell_owns_pane(if remote { p.remote_tmux.as_ref() } else { None }) {
            return None;
        }
        let agent = if remote {
            if info.ssh_pid != p.proc_info.ssh_pid || p.tmux_pane.is_none() {
                return None;
            }
            if !crate::tmux_local::title_matches_pane(
                &crate::session_ops::read_vte_window_title(p.vte)?,
                p,
            ) {
                return None;
            }
            crate::session::detect_agent_name(&p.remote_tmux.as_ref()?.2)?
        } else {
            if info.agent.is_empty() {
                return None;
            }
            &info.agent
        };
        let origin = Origin::new(p, info);
        let location = if remote {
            &info.ssh_host
        } else {
            &s.base_title
        };
        let which = p
            .tmux_pane
            .clone()
            .unwrap_or_else(|| format!("#{}", p.id.0));
        let group_pid = if info.active_pane_pid > 0 {
            info.active_pane_pid
        } else {
            p.child_pid
        };
        let foreground_group = proc_radar::read_proc_groups(group_pid)?.1;
        if foreground_group <= 0 {
            return None;
        }
        let delivery = if remote {
            Some((
                Connection::from_client(info.ssh_pid, &info.ssh_host)?,
                TmuxAddress::new(
                    p.tmux_server?,
                    p.tmux_pane.as_deref()?,
                    &p.remote_tmux.as_ref()?.2,
                )?,
            ))
        } else {
            None
        };
        Some(Self {
            delivery,
            origin,
            sid: s.id,
            vte: p.vte,
            label: format!("{agent} · {location} · {} · {which}", s.id),
            foreground_group,
        })
    }
}

pub fn targets() -> Vec<Target> {
    app_state::with_app(|app| {
        app.sessions
            .iter()
            .flat_map(|s| {
                s.pane_registry
                    .panes()
                    .iter()
                    .filter_map(move |p| Target::for_pane(s, p, false))
            })
            .collect()
    })
    .unwrap_or_default()
}

pub fn is_current(target: &Target) -> bool {
    crate::tmux_local::invalidate_client_cache();
    app_state::with_app(|app| {
        let session = app.sessions.iter().find(|s| s.id == target.sid)?;
        let pane = session.pane_registry.by_vte(target.vte)?;
        Target::for_pane(session, pane, true)
    })
    .flatten()
    .as_ref()
        == Some(target)
}
