//! Endpoint resolution for active tabs, VTE panes, and clickable remote links.

use std::path::PathBuf;

use crate::app::app_state::{pane_cwd, with_app};
use crate::ffi::GtkWidget;
use crate::files::{FileEndpoint, RemoteFileLink};
use crate::sys::paths::{home_dir, shorten_home, strip_anchor_or_line, strip_link_punctuation};
use crate::telemetry::ssh_connection::SshRoute;
use crate::terminal::pane_registry::PaneRecord;
use crate::terminal::session::TermSession;

fn non_empty(v: &str) -> Option<String> {
    let t = v.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn remote_host(s: &TermSession, pane: Option<&PaneRecord>, vte: *mut GtkWidget) -> Option<String> {
    if let Some(r) = s.panes.remote_for_vte(vte, s.scroll_wrap, s.vte).or_else(|| s.remote()) {
        return Some(r.host.clone());
    }
    pane.and_then(|p| p.remote_tmux.as_ref().and_then(|t| non_empty(&t.0)))
        .or_else(|| s.remote_tmux.as_ref().and_then(|t| non_empty(&t.0)))
        .or_else(|| pane.and_then(|p| non_empty(&p.proc_info.ssh_host)))
        .or_else(|| non_empty(&s.proc_info.ssh_host))
}

fn remote_pid(s: &TermSession, pane: Option<&PaneRecord>) -> i32 {
    pane.and_then(|p| p.ssh_directory.as_ref().map(|(pid, _)| *pid))
        .or_else(|| pane.and_then(|p| (p.proc_info.ssh_pid > 0).then_some(p.proc_info.ssh_pid)))
        .unwrap_or(s.proc_info.ssh_pid)
}

fn remote_cwd(s: &TermSession, pane: Option<&PaneRecord>, fallback: &str) -> String {
    pane.and_then(|p| p.ssh_directory.as_ref().and_then(|(_, d)| non_empty(d)))
        .or_else(|| pane.and_then(|p| p.remote_tmux.as_ref().and_then(|t| non_empty(&t.3))))
        .or_else(|| s.remote_tmux.as_ref().and_then(|t| non_empty(&t.3)))
        .or_else(|| non_empty(fallback))
        .unwrap_or_else(|| "~".to_owned())
}

fn local_cwd(s: &TermSession, fallback: &str) -> PathBuf {
    non_empty(fallback)
        .or_else(|| s.active_local_cwd())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home_dir()))
}

/// Resolve the local or remote SSH endpoint backing `vte` within session `s`.
pub fn resolve_session_endpoint(s: &TermSession, vte: *mut GtkWidget) -> FileEndpoint {
    let target = if vte.is_null() { s.active_vte() } else { vte };
    let pane = s.pane_registry.by_vte(target);
    let fallback = pane_cwd(s, target);
    let Some(host) = remote_host(s, pane, target) else {
        return FileEndpoint::Local { cwd: local_cwd(s, &fallback) };
    };
    let ssh_pid = remote_pid(s, pane);
    let route = if ssh_pid > 0 { SshRoute::from_client(ssh_pid, &host) } else { SshRoute::for_host(&host) };
    let Some(route) = route else {
        return FileEndpoint::Local { cwd: local_cwd(s, &fallback) };
    };
    FileEndpoint::Remote { sid: s.id, host, ssh_pid, route, cwd: remote_cwd(s, pane, &fallback) }
}

/// Resolve the endpoint for the tab/pane containing `vte`.
pub fn resolve_pane_endpoint(vte: *mut GtkWidget) -> Option<FileEndpoint> {
    if vte.is_null() {
        return None;
    }
    with_app(|app| {
        app.sessions
            .iter()
            .find(|s| s.vte == vte || s.pane_registry.by_vte(vte).is_some() || s.panes.contains_vte(vte))
            .map(|s| resolve_session_endpoint(s, vte))
    })
    .flatten()
}

/// Resolve the endpoint for the currently active tab and active pane.
pub fn resolve_active_endpoint() -> Option<FileEndpoint> {
    with_app(|app| {
        app.sessions.iter().find(|s| s.id == app.active_id).map(|s| resolve_session_endpoint(s, s.active_vte()))
    })
    .flatten()
}

pub(crate) fn clean_clicked_path(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.starts_with("mailto:") || (t.contains("://") && !t.starts_with("file://")) {
        return None;
    }
    let clean = strip_anchor_or_line(&strip_link_punctuation(t.strip_prefix("file://").unwrap_or(t)));
    non_empty(&clean)
}

/// Resolve a clicked path string inside an SSH VTE pane into a downloadable target.
pub fn resolve_remote_file_link(vte: *mut GtkWidget, raw_url_or_path: &str) -> Option<RemoteFileLink> {
    let clean = clean_clicked_path(raw_url_or_path)?;
    let FileEndpoint::Remote { sid, host, ssh_pid, route, cwd } = resolve_pane_endpoint(vte)? else {
        return None;
    };
    let path = if clean.starts_with('/') || clean.starts_with("~/") || clean == "~" {
        clean
    } else {
        let rel = clean.strip_prefix("./").unwrap_or(&clean);
        let base = cwd.trim_end_matches('/');
        if base.is_empty() || base == "~" { format!("~/{rel}") } else { format!("{base}/{rel}") }
    };
    Some(RemoteFileLink { sid, host, ssh_pid, route, path })
}

pub(crate) fn remote_download_for_vte_link(
    vte: *mut GtkWidget,
    link: Option<&crate::terminal::session::VteLink>,
) -> Option<RemoteFileLink> {
    let url = &link?.url;
    resolve_remote_file_link(vte, url)
}

/// Format a compact breadcrumb label for UI display (`~/proj` or `host:/srv/app`).
pub fn endpoint_short_label(endpoint: &FileEndpoint) -> String {
    match endpoint {
        FileEndpoint::Local { cwd } => shorten_home(&cwd.to_string_lossy()),
        FileEndpoint::Remote { host, cwd, .. } => {
            format!("{host}:{}", if cwd.is_empty() { "~" } else { cwd.as_str() })
        }
    }
}
