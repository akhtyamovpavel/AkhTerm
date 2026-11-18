//! Local and remote SSH git diff loaders plus safe local single-hunk apply/revert actions.

use crate::app_state;
use crate::git_status::resolve_git_repo;
use crate::markdown::Origin;
use crate::pane_registry::PaneRecord;
use crate::paths::{home_dir, shorten_home};
use crate::proc_radar::{self, ProcInfo};
use crate::session::TermSession;
use crate::telemetry::ssh_connection::SshRoute;
use crate::ui_text as text;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffMode {
    WorkingTree,
    Staged,
    Branch,
}

impl DiffMode {
    pub fn label_key(self) -> &'static str {
        match self {
            Self::WorkingTree => "working-tree",
            Self::Staged => "staged",
            Self::Branch => "branch",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffTarget {
    Local {
        repo_root: PathBuf,
        label: String,
        origin: Option<Origin>,
    },
    Remote {
        host: String,
        cwd: String,
        route: SshRoute,
        origin: Option<Origin>,
    },
}

impl DiffTarget {
    pub fn remote_host(&self) -> Option<&str> {
        match self {
            Self::Remote { host, .. } => Some(host.as_str()),
            Self::Local { .. } => None,
        }
    }

    pub fn local_root(&self) -> Option<&Path> {
        match self {
            Self::Local { repo_root, .. } => Some(repo_root.as_path()),
            Self::Remote { .. } => None,
        }
    }

    pub fn display_label(&self) -> String {
        match self {
            Self::Local { label, .. } => label.clone(),
            Self::Remote { host, cwd, .. } => format!("{host}:{cwd}"),
        }
    }

    pub fn origin(&self) -> Option<&Origin> {
        match self {
            Self::Local { origin, .. } | Self::Remote { origin, .. } => origin.as_ref(),
        }
    }

    pub fn allows_hunk_actions(&self, mode: DiffMode) -> bool {
        matches!(self, Self::Local { .. }) && mode == DiffMode::WorkingTree
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffLoadResult {
    pub raw_diff: String,
    pub truncated: bool,
    pub scope_label: String,
}

fn trim_to_last_newline(bytes: &mut Vec<u8>) {
    if let Some(pos) = bytes.iter().rposition(|&b| b == b'\n') {
        bytes.truncate(pos + 1);
    }
}

pub fn read_capped_diff(reader: impl Read) -> io::Result<(String, bool)> {
    let mut bytes = Vec::new();
    reader.take((MAX_DIFF_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > MAX_DIFF_BYTES;
    if truncated {
        bytes.truncate(MAX_DIFF_BYTES);
        trim_to_last_newline(&mut bytes);
    }
    let clean = String::from_utf8_lossy(&bytes).replace('\0', "");
    Ok((clean, truncated))
}

fn git_stdout_trimmed(repo_root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn resolve_local_base_branch(repo_root: &Path) -> Option<String> {
    if let Some(head) = git_stdout_trimmed(repo_root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        return Some(head);
    }
    for candidate in ["main", "master", "origin/main", "origin/master"] {
        if git_stdout_trimmed(repo_root, &["rev-parse", "--verify", "--quiet", candidate]).is_some() {
            return Some(candidate.to_owned());
        }
    }
    None
}

fn run_local_git_diff(repo_root: &Path, args: &[&str]) -> io::Result<(String, bool)> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other("missing git stdout"))?;
    let (raw, truncated) = read_capped_diff(stdout)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(git_stderr_err(&output.stderr, "git diff failed"));
    }
    Ok((raw, truncated))
}

pub fn load_local_diff(repo_root: &Path, mode: DiffMode) -> io::Result<DiffLoadResult> {
    let (raw_diff, truncated, scope_label) = match mode {
        DiffMode::WorkingTree => {
            let (raw, trunc) = run_local_git_diff(repo_root, &["diff", "--no-color", "--no-ext-diff"])?;
            (raw, trunc, "working tree".to_owned())
        }
        DiffMode::Staged => {
            let (raw, trunc) = run_local_git_diff(repo_root, &["diff", "--cached", "--no-color", "--no-ext-diff"])?;
            (raw, trunc, "staged".to_owned())
        }
        DiffMode::Branch => load_local_branch_diff(repo_root)?,
    };
    Ok(DiffLoadResult { raw_diff, truncated, scope_label })
}

fn load_local_branch_diff(repo_root: &Path) -> io::Result<(String, bool, String)> {
    let base = resolve_local_base_branch(repo_root)
        .ok_or_else(|| io::Error::other("Base branch (main/master) not found"))?;
    let merge_base = git_stdout_trimmed(repo_root, &["merge-base", &base, "HEAD"])
        .ok_or_else(|| io::Error::other(format!("Cannot compute merge-base with {base}")))?;
    let range = format!("{merge_base}..HEAD");
    let (raw, trunc) = run_local_git_diff(repo_root, &["diff", "--no-color", "--no-ext-diff", &range])?;
    Ok((raw, trunc, format!("branch vs {base}")))
}

fn git_stderr_err(stderr: &[u8], fallback: &str) -> io::Error {
    let msg = String::from_utf8_lossy(stderr).trim().to_owned();
    io::Error::other(if msg.is_empty() { fallback.to_owned() } else { msg })
}

const REMOTE_BRANCH_PREFIX: &str = "\
base=$(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null || true); \
if [ -z \"$base\" ]; then for c in main master origin/main origin/master; do \
  git rev-parse --verify --quiet \"$c\" >/dev/null && base=\"$c\" && break; \
done; fi; \
[ -n \"$base\" ] || exit 3; \
mb=$(git merge-base \"$base\" HEAD) || exit 4; \
git -c core.quotepath=false diff --no-color --no-ext-diff \"$mb..HEAD\"";

fn remote_diff_body(mode: DiffMode, cap: usize) -> String {
    match mode {
        DiffMode::WorkingTree => format!("git -c core.quotepath=false diff --no-color --no-ext-diff | head -c {cap}"),
        DiffMode::Staged => format!("git -c core.quotepath=false diff --cached --no-color --no-ext-diff | head -c {cap}"),
        DiffMode::Branch => format!("{REMOTE_BRANCH_PREFIX} | head -c {cap}"),
    }
}

pub fn build_remote_diff_script(cwd: &str, mode: DiffMode) -> String {
    let dir = proc_radar::shell_quote(if cwd.is_empty() { "." } else { cwd });
    let body = remote_diff_body(mode, MAX_DIFF_BYTES + 1);
    format!("cd {dir} && git rev-parse --show-toplevel >/dev/null 2>&1 && {body}")
}

pub fn load_remote_diff(
    route: &SshRoute,
    host: &str,
    cwd: &str,
    mode: DiffMode,
) -> io::Result<DiffLoadResult> {
    let script = build_remote_diff_script(cwd, mode);
    let mut child = route.spawn(&script, Stdio::null(), "15")?;
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other("missing SSH stdout"))?;
    let (raw_diff, truncated) = read_capped_diff(stdout)?;
    let status = child.wait()?;
    if !status.success() {
        return Err(io::Error::other(format!("SSH diff failed on {host}")));
    }
    let scope_label = format!("{host} · {}", mode.label_key());
    Ok(DiffLoadResult { raw_diff, truncated, scope_label })
}

pub fn load_diff_for_target(target: &DiffTarget, mode: DiffMode) -> io::Result<DiffLoadResult> {
    match target {
        DiffTarget::Local { repo_root, .. } => load_local_diff(repo_root, mode),
        DiffTarget::Remote { host, cwd, route, .. } => load_remote_diff(route, host, cwd, mode),
    }
}

fn run_git_apply(repo_root: &Path, args: &[&str], patch: &str) -> io::Result<()> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("apply")
        .args(args)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(patch.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(git_stderr_err(&output.stderr, "git apply failed"));
    }
    Ok(())
}

pub fn stage_hunk(repo_root: &Path, patch: &str) -> io::Result<()> {
    run_git_apply(repo_root, &["--cached", "--whitespace=nowarn"], patch)
}

pub fn revert_hunk(repo_root: &Path, patch: &str) -> io::Result<()> {
    run_git_apply(repo_root, &["-R", "--whitespace=nowarn"], patch)
}

fn pane_remote_host<'a>(session: &'a TermSession, vte: *mut crate::ffi::GtkWidget, info: &'a ProcInfo) -> Option<&'a str> {
    if !info.ssh_host.is_empty() {
        return Some(&info.ssh_host);
    }
    let profile = session.panes.remote_for_vte(vte, session.scroll_wrap, session.vte);
    let primary = (vte == session.vte).then(|| session.remote()).flatten();
    Some(&profile.or(primary)?.host)
}

fn pane_tmux_cwd(pane: &PaneRecord, info: &ProcInfo, title: &str) -> Option<String> {
    let valid = title.starts_with("AKHTERM_TMUX:")
        && pane.proc_info.ssh_pid == info.ssh_pid
        && crate::tmux_local::title_matches_pane(title, pane);
    let tmux = valid.then_some(pane.remote_tmux.as_ref()).flatten()?;
    (!tmux.3.is_empty()).then(|| tmux.3.clone())
}

fn pane_ssh_dir(pane: &PaneRecord, info: &ProcInfo) -> Option<String> {
    let (pid, dir) = pane.ssh_directory.as_ref()?;
    (*pid == info.ssh_pid && !dir.is_empty()).then(|| dir.clone())
}

fn pane_remote_cwd(session: &TermSession, pane: Option<&PaneRecord>, info: &ProcInfo, title: &str) -> String {
    pane.and_then(|p| pane_tmux_cwd(p, info, title).or_else(|| pane_ssh_dir(p, info)))
        .or_else(|| session.remote_tmux.as_ref().filter(|t| !t.3.is_empty()).map(|t| t.3.clone()))
        .unwrap_or_else(|| ".".to_owned())
}

fn resolve_remote_target(
    session: &TermSession,
    vte: *mut crate::ffi::GtkWidget,
    pane: Option<&PaneRecord>,
    info: &ProcInfo,
    origin: Option<Origin>,
) -> Result<DiffTarget, String> {
    let host = pane_remote_host(session, vte, info)
        .ok_or_else(|| text::MD_CONNECTION.as_str().to_owned())?
        .to_owned();
    let route = SshRoute::from_client(info.ssh_pid, &host)
        .or_else(|| SshRoute::for_host(&host))
        .ok_or_else(|| text::MD_CONNECTION.as_str().to_owned())?;
    let title = crate::session_ops::read_vte_window_title(vte).unwrap_or_default();
    let cwd = pane_remote_cwd(session, pane, info, &title);
    Ok(DiffTarget::Remote { host, cwd, route, origin })
}

fn resolve_local_target(cwd: &str, origin: Option<Origin>) -> Result<DiffTarget, String> {
    let (root, head) = resolve_git_repo(cwd, &home_dir())
        .ok_or_else(|| text::GIT_NOT_REPO.to_string_lossy().into_owned())?;
    let label = format!("{} · {}", head.badge(), shorten_home(&root));
    Ok(DiffTarget::Local { repo_root: PathBuf::from(root), label, origin })
}

fn resolve_session_target(
    session: &TermSession,
    vte: *mut crate::ffi::GtkWidget,
    origin: Option<Origin>,
) -> Result<DiffTarget, String> {
    let pane = session.pane_registry.by_vte(vte);
    let pid = pane.map_or_else(|| session.active_pid(), |p| p.child_pid);
    let live = proc_radar::inspect_proc_tree(pid);
    if live.ssh_pid > 0 || pane_remote_host(session, vte, &live).is_some() {
        return resolve_remote_target(session, vte, pane, &live, origin);
    }
    let cwd = session.active_local_cwd().ok_or_else(|| text::GIT_NOT_REPO.to_string_lossy().into_owned())?;
    resolve_local_target(&cwd, origin)
}

pub fn resolve_active_target() -> Result<DiffTarget, String> {
    let vte = crate::shortcuts::active_search_vte();
    let origin = crate::markdown::for_vte(vte);
    app_state::with_app(|app| {
        let session = app.sessions.iter().find(|s| s.id == app.active_id)
            .ok_or_else(|| text::GIT_NOT_REPO.to_string_lossy().into_owned())?;
        resolve_session_target(session, vte, origin)
    })
    .unwrap_or_else(|| Err(text::GIT_NOT_REPO.to_string_lossy().into_owned()))
}
