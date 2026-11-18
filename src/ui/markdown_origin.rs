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

fn inspect_pane<T>(
    vte: *mut GtkWidget,
    inspect: impl FnOnce(&TermSession, &PaneRecord, &ProcInfo) -> Option<T>,
) -> Option<T> {
    app_state::with_app(|app| {
        let session = app.sessions.iter().find(|s| s.has_vte(vte))?;
        let pane = session.pane_registry.by_vte(vte)?;
        let live = proc_radar::inspect_proc_tree(pane.child_pid);
        inspect(session, pane, &live)
    })?
}

pub fn for_vte(vte: *mut GtkWidget) -> Option<Origin> {
    inspect_pane(vte, |_, pane, live| Some(Origin::new(pane, live)))
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
    inspect_pane(vte, |session, p, info| {
        let host = document_host(session, vte, info)?.to_owned();
        let title = crate::session_ops::read_vte_window_title(vte).unwrap_or_default();
        let cwd = remote_directory(p, info, &title);
        Some(RemoteDocument {
            origin: Origin::new(p, info),
            connection: Connection::from_client(info.ssh_pid, &host),
            host,
            path: remote::resolve(&path, cwd).unwrap_or(path),
        })
    })
}

fn document_host<'a>(
    session: &'a TermSession,
    vte: *mut GtkWidget,
    info: &'a ProcInfo,
) -> Option<&'a str> {
    if !info.ssh_host.is_empty() {
        return Some(&info.ssh_host);
    }
    let profile = session
        .panes
        .remote_for_vte(vte, session.scroll_wrap, session.vte);
    let primary = (vte == session.vte).then(|| session.remote()).flatten();
    Some(&profile.or(primary)?.host)
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

type RemoteDelivery = (Connection, TmuxAddress);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub origin: Origin,
    pub sid: usize,
    pub vte: *mut GtkWidget,
    pub label: String,
    foreground_group: i32,
    pub delivery: Option<RemoteDelivery>,
}

impl Target {
    fn for_pane(s: &TermSession, pane: &PaneRecord, info: &ProcInfo) -> Option<Self> {
        if pane.child_pid <= 0 || info.foreground_shell {
            return None;
        }
        let (agent, location, delivery) = if info.ssh_pid > 0 {
            let (agent, address) = remote_agent(pane, info)?;
            (agent, info.ssh_host.as_str(), Some(address))
        } else {
            if info.agent.is_empty() {
                return None;
            }
            (info.agent.as_str(), s.base_title.as_str(), None)
        };
        let which = pane
            .tmux_pane
            .clone()
            .unwrap_or_else(|| format!("#{}", pane.id.0));
        Some(Self {
            origin: Origin::new(pane, info),
            sid: s.id,
            vte: pane.vte,
            label: format!("{agent} · {location} · {} · {which}", s.id),
            foreground_group: foreground_group(info, pane.child_pid)?,
            delivery,
        })
    }
}

fn foreground_group(info: &ProcInfo, child_pid: i32) -> Option<i32> {
    let pid = if info.active_pane_pid > 0 {
        info.active_pane_pid
    } else {
        child_pid
    };
    let (_, group) = proc_radar::read_proc_groups(pid)?;
    (group > 0).then_some(group)
}

fn remote_agent(pane: &PaneRecord, info: &ProcInfo) -> Option<(&'static str, RemoteDelivery)> {
    if info.ssh_pid != pane.proc_info.ssh_pid || info.shell_owns_pane(pane.remote_tmux.as_ref()) {
        return None;
    }
    let title = crate::session_ops::read_vte_window_title(pane.vte)?;
    if !crate::tmux_local::title_matches_pane(&title, pane) {
        return None;
    }
    let cmd = &pane.remote_tmux.as_ref()?.2;
    let agent = crate::session::detect_agent_name(cmd)?;
    let address = TmuxAddress::new(pane.tmux_server?, pane.tmux_pane.as_deref()?, cmd)?;
    let connection = Connection::from_client(info.ssh_pid, &info.ssh_host)?;
    Some((agent, (connection, address)))
}

pub fn targets() -> Vec<Target> {
    app_state::with_app(|app| {
        app.sessions
            .iter()
            .flat_map(|s| {
                s.pane_registry
                    .panes()
                    .iter()
                    .filter_map(move |p| Target::for_pane(s, p, &p.proc_info))
            })
            .collect()
    })
    .unwrap_or_default()
}

pub fn is_current(target: &Target) -> bool {
    crate::tmux_local::invalidate_client_cache();
    inspect_pane(target.vte, Target::for_pane).as_ref() == Some(target)
}
