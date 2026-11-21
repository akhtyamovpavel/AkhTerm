//! Deterministic per-pane Git checkpoints under refs/akhterm/checkpoints.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_int, c_void, CStr};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{fs, ptr, thread};

use crate::ctl_protocol::HookState;
use crate::ffi::*;
use crate::git_status::resolve_git_repo;
use crate::paths::home_dir;
use crate::{app_state, connect_click, pane_actions, topbar, ui_text};

pub const CHECKPOINT_REF_PREFIX: &str = "refs/akhterm/checkpoints";
pub const MAX_CHECKPOINTS_PER_PANE: usize = 20;
pub const DEFAULT_HOOK_DEBOUNCE: Duration = Duration::from_secs(10);
const CHECKPOINT_MSG_PREFIX: &str = "akhterm-checkpoint | ";
const POPOVER_VISIBLE_LIMIT: usize = 8;

static CHECKPOINT_SEQ: AtomicU64 = AtomicU64::new(1);
static HOOK_STATE: OnceLock<Mutex<HookCaptureGuard>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointEntry {
    pub ref_name: String, pub sha: String, pub short_sha: String,
    pub timestamp_ms: u128, pub seq: u64, pub label: String, pub stat_summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureOutcome { Saved(CheckpointEntry), SkippedNotRepo, SkippedClean, SkippedDuplicate(String) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreOutcome { pub restored_ref: String, pub short_sha: String, pub safety_ref: Option<String> }

#[derive(Clone, Debug, PartialEq, Eq)]
struct TreeSignature { base_oid: String, work_tree: String, index_tree: String, untracked_tree: Option<String> }

#[derive(Default)]
pub struct HookDebounceMap { last_trigger: HashMap<u64, Instant>, in_flight: HashSet<u64> }

impl HookDebounceMap {
    pub fn try_acquire(&mut self, pane_id: u64, now: Instant, window: Duration) -> bool {
        if self.in_flight.contains(&pane_id) {
            return false;
        }
        if self.last_trigger.get(&pane_id).is_some_and(|t| now.duration_since(*t) < window) {
            return false;
        }
        self.last_trigger.insert(pane_id, now);
        self.in_flight.insert(pane_id);
        true
    }

    pub fn release(&mut self, pane_id: u64) {
        self.in_flight.remove(&pane_id);
    }
}

#[derive(Default)]
struct HookCaptureGuard {
    debounce: HookDebounceMap,
}

pub fn sanitize_pane_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(48));
    for ch in raw.chars() {
        if out.len() >= 48 {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() { "default".to_owned() } else { trimmed.to_owned() }
}

pub fn pane_key_for_id(pane_id: u64) -> String {
    format!("pane-{pane_id}")
}

fn pane_ref_prefix(pane_key: &str) -> String {
    format!("{CHECKPOINT_REF_PREFIX}/{}/", sanitize_pane_key(pane_key))
}

fn build_ref_name(pane_key: &str, ts_ms: u128, seq: u64) -> String {
    format!("{}{ts_ms:013}-{seq:06}", pane_ref_prefix(pane_key))
}

fn parse_ref_ts_seq(ref_name: &str) -> (u128, u64) {
    let leaf = ref_name.rsplit('/').next().unwrap_or("");
    let Some((ts_str, seq_str)) = leaf.split_once('-') else {
        return (0, 0);
    };
    let ts = ts_str.parse::<u128>().unwrap_or(0);
    let seq = seq_str.parse::<u64>().unwrap_or(0);
    (ts, seq)
}

fn parse_checkpoint_label(subject: &str) -> String {
    let trimmed = subject.trim();
    if let Some(rest) = trimmed.strip_prefix(CHECKPOINT_MSG_PREFIX) {
        let label = rest.trim();
        if !label.is_empty() {
            return label.to_owned();
        }
    }
    if let Some((_, tail)) = trimmed.split_once(CHECKPOINT_MSG_PREFIX) {
        let label = tail.trim();
        if !label.is_empty() {
            return label.to_owned();
        }
    }
    if trimmed.is_empty() { "checkpoint".to_owned() } else { trimmed.to_owned() }
}

fn now_epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn format_relative_age(ts_ms: u128, now_ms: u128) -> String {
    let diff_secs = now_ms.saturating_sub(ts_ms) / 1000;
    if diff_secs < 60 {
        format!("{diff_secs}s")
    } else if diff_secs < 3600 {
        format!("{}m", diff_secs / 60)
    } else if diff_secs < 86_400 {
        format!("{}h", diff_secs / 3600)
    } else {
        format!("{}d", diff_secs / 86_400)
    }
}

pub fn format_checkpoint_row(entry: &CheckpointEntry, now_ms: u128) -> String {
    let age = format_relative_age(entry.timestamp_ms, now_ms);
    if entry.stat_summary.is_empty() {
        format!("{age}  {}  {}", entry.short_sha, entry.label)
    } else {
        format!("{age}  {}  {} · {}", entry.short_sha, entry.label, entry.stat_summary)
    }
}

pub fn should_trigger_hook_checkpoint(
    state: HookState,
    message: Option<&str>,
    is_local: bool,
    cwd: &str,
) -> bool {
    if !is_local || cwd.trim().is_empty() {
        return false;
    }
    if state == HookState::Working {
        return true;
    }
    message.is_some_and(|m| m.to_ascii_lowercase().contains("pretooluse"))
}

struct TempIndexGuard(PathBuf);

impl TempIndexGuard {
    fn new() -> Self {
        let seq = CHECKPOINT_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = format!("akhterm-cp-idx-{}-{seq}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_file(&path);
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempIndexGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn build_git_cmd(cwd: &Path, args: &[&str], with_identity: bool) -> Command {
    let mut cmd = Command::new("git");
    cmd.env("LC_MESSAGES", "C")
        .arg("-c")
        .arg("core.quotepath=false");
    if with_identity {
        cmd.arg("-c")
            .arg("user.name=AkhTerm")
            .arg("-c")
            .arg("user.email=akhterm@local");
    }
    cmd.arg("-C").arg(cwd).args(args).stdin(Stdio::null());
    cmd
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<Output, String> {
    build_git_cmd(cwd, args, false)
        .output()
        .map_err(|e| format!("git failed: {e}"))
}

fn run_git_id(cwd: &Path, args: &[&str]) -> Result<Output, String> {
    build_git_cmd(cwd, args, true)
        .output()
        .map_err(|e| format!("git failed: {e}"))
}

fn run_git_with_index(cwd: &Path, idx: &Path, args: &[&str]) -> Result<Output, String> {
    build_git_cmd(cwd, args, false)
        .env("GIT_INDEX_FILE", idx)
        .output()
        .map_err(|e| format!("git failed: {e}"))
}

fn output_err(out: &Output, ctx: &str) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    if stderr.is_empty() {
        format!("{ctx} (exit {:?})", out.status.code())
    } else {
        format!("{ctx}: {stderr}")
    }
}

fn git_stdout_ok(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = run_git(cwd, args).ok().filter(|o| o.status.success())?;
    let txt = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!txt.is_empty()).then_some(txt)
}

fn resolve_repo_root(dir: &Path) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    git_stdout_ok(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn list_untracked_files(root: &Path) -> Result<Vec<u8>, String> {
    let out = run_git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    if !out.status.success() {
        return Err(output_err(&out, "git ls-files failed"));
    }
    Ok(out.stdout)
}

fn ensure_git_ok(out: Output, ctx: &str) -> Result<String, String> {
    if !out.status.success() {
        return Err(output_err(&out, ctx));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn add_untracked_to_temp_index(root: &Path, idx: &Path, nul_paths: &[u8]) -> Result<(), String> {
    let mut child = build_git_cmd(root, &["update-index", "--add", "-z", "--stdin"], false)
        .env("GIT_INDEX_FILE", idx)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git update-index spawn failed: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(nul_paths).map_err(|e| format!("stdin write failed: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git update-index wait failed: {e}"))?;
    ensure_git_ok(out, "git update-index failed").map(|_| ())
}

fn create_untracked_commit(root: &Path, nul_paths: &[u8]) -> Result<String, String> {
    let idx = TempIndexGuard::new();
    add_untracked_to_temp_index(root, idx.path(), nul_paths)?;
    let u_tree = ensure_git_ok(run_git_with_index(root, idx.path(), &["write-tree"])?, "git write-tree failed")?;
    ensure_git_ok(run_git_id(root, &["commit-tree", &u_tree, "-m", "untracked files"])?, "git commit-tree untracked failed")
}

fn create_tracked_stash(root: &Path, message: &str) -> Result<Option<String>, String> {
    let sha = ensure_git_ok(run_git_id(root, &["stash", "create", message])?, "git stash create failed")?;
    Ok((!sha.is_empty()).then_some(sha))
}

fn resolve_tracked_parents(root: &Path, s: &str) -> Result<(String, String, String), String> {
    let tree = git_stdout_ok(root, &["rev-parse", &format!("{s}^{{tree}}")]).ok_or("cannot resolve stash tree")?;
    let p1 = git_stdout_ok(root, &["rev-parse", &format!("{s}^1")]).ok_or("cannot resolve stash parent 1")?;
    let p2 = git_stdout_ok(root, &["rev-parse", &format!("{s}^2")]).ok_or("cannot resolve stash parent 2")?;
    Ok((tree, p1, p2))
}

fn create_clean_index_parents(root: &Path, head_oid: &str) -> Result<(String, String, String), String> {
    let head_tree = git_stdout_ok(root, &["rev-parse", "HEAD^{tree}"]).ok_or("cannot resolve HEAD tree")?;
    let idx_oid = ensure_git_ok(
        run_git_id(root, &["commit-tree", &head_tree, "-p", head_oid, "-m", "index on checkpoint"])?,
        "git commit-tree index failed",
    )?;
    Ok((head_tree, head_oid.to_owned(), idx_oid))
}

fn wrap_with_untracked(
    root: &Path,
    tracked_oid: Option<&str>,
    head_oid: &str,
    u_oid: &str,
    msg: &str,
) -> Result<String, String> {
    let (w_tree, p1, p2) = match tracked_oid {
        Some(s) => resolve_tracked_parents(root, s)?,
        None => create_clean_index_parents(root, head_oid)?,
    };
    ensure_git_ok(
        run_git_id(root, &["commit-tree", &w_tree, "-p", &p1, "-p", &p2, "-p", u_oid, "-m", msg])?,
        "git commit-tree checkpoint failed",
    )
}

fn commit_signature(root: &Path, oid: &str) -> Option<TreeSignature> {
    let base_oid = git_stdout_ok(root, &["rev-parse", &format!("{oid}^1")])?;
    let work_tree = git_stdout_ok(root, &["rev-parse", &format!("{oid}^{{tree}}")])?;
    let index_tree = git_stdout_ok(root, &["rev-parse", &format!("{oid}^2^{{tree}}")])?;
    let untracked_tree = git_stdout_ok(root, &["rev-parse", "--verify", &format!("{oid}^3^{{tree}}")]);
    Some(TreeSignature { base_oid, work_tree, index_tree, untracked_tree })
}

fn format_diff_stat_summary(root: &Path, oid: &str) -> String {
    let tracked = git_stdout_ok(root, &["diff", "--stat", &format!("{oid}^1"), oid])
        .and_then(|raw| raw.lines().last().map(|l| l.trim().to_owned()))
        .unwrap_or_default();
    let untracked_count = git_stdout_ok(root, &["ls-tree", "-r", "--name-only", &format!("{oid}^3")])
        .map(|raw| raw.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    match (tracked.is_empty(), untracked_count) {
        (true, 0) => String::new(),
        (true, n) => format!("+{n} untracked"),
        (false, 0) => tracked,
        (false, n) => format!("{tracked}, +{n} untracked"),
    }
}

fn list_pane_ref_lines(root: &Path, pane_key: &str) -> Result<Vec<String>, String> {
    let prefix = pane_ref_prefix(pane_key);
    let fmt = "%(refname)\t%(objectname:short)\t%(objectname)\t%(subject)";
    let out = run_git(root, &["for-each-ref", "--sort=-refname", &format!("--format={fmt}"), &prefix])?;
    if !out.status.success() {
        return Err(output_err(&out, "git for-each-ref failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect())
}

fn parse_ref_line(root: &Path, line: &str) -> Option<CheckpointEntry> {
    let mut parts = line.splitn(4, '\t');
    let ref_name = parts.next()?.trim().to_owned();
    let short_sha = parts.next()?.trim().to_owned();
    let sha = parts.next()?.trim().to_owned();
    let subject = parts.next().unwrap_or("");
    let (timestamp_ms, seq) = parse_ref_ts_seq(&ref_name);
    let label = parse_checkpoint_label(subject);
    let stat_summary = format_diff_stat_summary(root, &sha);
    Some(CheckpointEntry { ref_name, sha, short_sha, timestamp_ms, seq, label, stat_summary })
}

pub fn list_checkpoints(dir: &Path, pane_key: &str) -> Result<Vec<CheckpointEntry>, String> {
    let Some(root) = resolve_repo_root(dir) else {
        return Ok(Vec::new());
    };
    let lines = list_pane_ref_lines(&root, pane_key)?;
    let mut entries: Vec<CheckpointEntry> = lines
        .iter()
        .filter_map(|line| parse_ref_line(&root, line))
        .collect();
    entries.sort_by(|a, b| (b.timestamp_ms, b.seq).cmp(&(a.timestamp_ms, a.seq)));
    Ok(entries)
}

fn sorted_pane_ref_oids(root: &Path, pane_key: &str) -> Result<Vec<(String, String)>, String> {
    let lines = list_pane_ref_lines(root, pane_key)?;
    let mut parsed: Vec<(u128, u64, String, String)> = lines
        .iter()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '\t');
            let ref_name = parts.next()?.trim().to_owned();
            let _short = parts.next()?;
            let oid = parts.next()?.trim().to_owned();
            let (ts, seq) = parse_ref_ts_seq(&ref_name);
            Some((ts, seq, ref_name, oid))
        })
        .collect();
    parsed.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    Ok(parsed.into_iter().map(|(_, _, r, o)| (r, o)).collect())
}

fn latest_pane_ref_and_oid(root: &Path, pane_key: &str) -> Result<Option<(String, String)>, String> {
    Ok(sorted_pane_ref_oids(root, pane_key)?.into_iter().next())
}

pub fn prune_pane_checkpoints(root: &Path, pane_key: &str, max_keep: usize) -> Result<usize, String> {
    let refs = sorted_pane_ref_oids(root, pane_key)?;
    if refs.len() <= max_keep {
        return Ok(0);
    }
    let mut deleted = 0;
    for (old_ref, _) in refs.into_iter().skip(max_keep) {
        if run_git(root, &["update-ref", "-d", &old_ref])?.status.success() {
            deleted += 1;
        }
    }
    Ok(deleted)
}

fn build_checkpoint_oid(root: &Path, head_oid: &str, msg: &str) -> Result<Option<String>, String> {
    let tracked = create_tracked_stash(root, msg)?;
    let untracked_raw = list_untracked_files(root)?;
    let has_untracked = untracked_raw.iter().any(|&b| b != 0);
    if tracked.is_none() && !has_untracked {
        return Ok(None);
    }
    if !has_untracked {
        return Ok(tracked);
    }
    let u_oid = create_untracked_commit(root, &untracked_raw)?;
    let cp_oid = wrap_with_untracked(root, tracked.as_deref(), head_oid, &u_oid, msg)?;
    Ok(Some(cp_oid))
}

fn save_checkpoint_ref(
    root: &Path,
    pane_key: &str,
    cp_oid: String,
    clean_label: &str,
) -> Result<CheckpointEntry, String> {
    let ts_ms = now_epoch_ms();
    let seq = CHECKPOINT_SEQ.fetch_add(1, Ordering::Relaxed);
    let ref_name = build_ref_name(pane_key, ts_ms, seq);
    ensure_git_ok(run_git(root, &["update-ref", &ref_name, &cp_oid])?, "git update-ref failed")?;
    let _ = prune_pane_checkpoints(root, pane_key, MAX_CHECKPOINTS_PER_PANE);
    let short_sha = git_stdout_ok(root, &["rev-parse", "--short", &cp_oid])
        .unwrap_or_else(|| cp_oid.chars().take(7).collect());
    let stat_summary = format_diff_stat_summary(root, &cp_oid);
    Ok(CheckpointEntry {
        ref_name, sha: cp_oid, short_sha, timestamp_ms: ts_ms, seq,
        label: clean_label.to_owned(), stat_summary,
    })
}

pub fn capture_checkpoint(dir: &Path, pane_key: &str, trigger: &str) -> Result<CaptureOutcome, String> {
    let Some(root) = resolve_repo_root(dir) else { return Ok(CaptureOutcome::SkippedNotRepo); };
    let Some(head_oid) = git_stdout_ok(&root, &["rev-parse", "--verify", "HEAD"]) else {
        return Ok(CaptureOutcome::SkippedClean);
    };
    let clean_label = if trigger.trim().is_empty() { "manual" } else { trigger.trim() };
    let msg = format!("{CHECKPOINT_MSG_PREFIX}{clean_label}");
    let Some(cp_oid) = build_checkpoint_oid(&root, &head_oid, &msg)? else {
        return Ok(CaptureOutcome::SkippedClean);
    };
    if let Some((latest_ref, latest_oid)) = latest_pane_ref_and_oid(&root, pane_key)? {
        if commit_signature(&root, &cp_oid) == commit_signature(&root, &latest_oid) {
            return Ok(CaptureOutcome::SkippedDuplicate(latest_ref));
        }
    }
    Ok(CaptureOutcome::Saved(save_checkpoint_ref(&root, pane_key, cp_oid, clean_label)?))
}

fn restore_untracked_from_commit(root: &Path, cp_oid: &str) -> Result<(), String> {
    let u_ref = format!("{cp_oid}^3");
    if git_stdout_ok(root, &["rev-parse", "--verify", &format!("{u_ref}^{{tree}}")]).is_none() {
        return Ok(());
    }
    let idx = TempIndexGuard::new();
    ensure_git_ok(run_git_with_index(root, idx.path(), &["read-tree", &u_ref])?, "git read-tree untracked failed")?;
    ensure_git_ok(run_git_with_index(root, idx.path(), &["checkout-index", "-a", "-f"])?, "git checkout-index untracked failed")?;
    Ok(())
}

fn restore_exact_on_same_head(root: &Path, cp_oid: &str) -> Result<(), String> {
    ensure_git_ok(run_git(root, &["read-tree", "--reset", "-u", cp_oid])?, "git read-tree restore failed")?;
    ensure_git_ok(run_git(root, &["read-tree", &format!("{cp_oid}^2")])?, "git read-tree index restore failed")?;
    restore_untracked_from_commit(root, cp_oid)
}

fn restore_via_stash_apply(root: &Path, cp_oid: &str) -> Result<(), String> {
    let first = run_git(root, &["stash", "apply", "--index", cp_oid])?;
    if first.status.success() {
        return Ok(());
    }
    let second = run_git(root, &["stash", "apply", cp_oid])?;
    if second.status.success() {
        return Ok(());
    }
    Err(output_err(&second, "git stash apply failed"))
}

pub fn restore_checkpoint(dir: &Path, pane_key: &str, ref_name: &str) -> Result<RestoreOutcome, String> {
    let Some(root) = resolve_repo_root(dir) else {
        return Err("not a Git repository".to_owned());
    };
    let Some(cp_oid) = git_stdout_ok(&root, &["rev-parse", "--verify", ref_name]) else {
        return Err(format!("checkpoint ref not found: {ref_name}"));
    };
    let safety_ref = match capture_checkpoint(&root, pane_key, "pre-undo")? {
        CaptureOutcome::Saved(entry) => Some(entry.ref_name),
        _ => None,
    };
    let cur_head = git_stdout_ok(&root, &["rev-parse", "--verify", "HEAD"]);
    let base_head = git_stdout_ok(&root, &["rev-parse", "--verify", &format!("{cp_oid}^1")]);
    if cur_head.is_some() && cur_head == base_head {
        restore_exact_on_same_head(&root, &cp_oid)?;
    } else {
        restore_via_stash_apply(&root, &cp_oid)?;
    }
    let short_sha = git_stdout_ok(&root, &["rev-parse", "--short", &cp_oid])
        .unwrap_or_else(|| cp_oid.chars().take(7).collect());
    Ok(RestoreOutcome { restored_ref: ref_name.to_owned(), short_sha, safety_ref })
}

pub fn undo_last_checkpoint(dir: &Path, pane_key: &str) -> Result<RestoreOutcome, String> {
    let Some(root) = resolve_repo_root(dir) else {
        return Err("not a Git repository".to_owned());
    };
    let entries = list_checkpoints(&root, pane_key)?;
    let target = entries
        .iter()
        .find(|e| e.label != "pre-undo")
        .or_else(|| entries.first())
        .ok_or_else(|| ui_text::CP_MSG_NONE.as_str().to_owned())?;
    restore_checkpoint(&root, pane_key, &target.ref_name)
}

fn hook_guard() -> &'static Mutex<HookCaptureGuard> {
    HOOK_STATE.get_or_init(|| Mutex::new(HookCaptureGuard::default()))
}

pub fn schedule_hook_checkpoint(pane_id: u64, cwd: String, trigger: String) {
    let acquired = hook_guard()
        .lock()
        .is_ok_and(|mut g| g.debounce.try_acquire(pane_id, Instant::now(), DEFAULT_HOOK_DEBOUNCE));
    if !acquired {
        return;
    }
    let key = pane_key_for_id(pane_id);
    thread::spawn(move || {
        let _ = capture_checkpoint(Path::new(&cwd), &key, &trigger);
        if let Ok(mut guard) = hook_guard().lock() {
            guard.debounce.release(pane_id);
        }
    });
}

#[derive(Default)]
struct CheckpointUiState {
    popover: usize,
    repo_root: String,
    pane_key: String,
    refs: Vec<String>,
}

thread_local! {
    static CP_UI: RefCell<CheckpointUiState> = RefCell::new(CheckpointUiState::default());
}

struct CpActionDone {
    repo_root: String,
    message: String,
    refresh_popover: bool,
}

fn active_local_target() -> Option<(String, String)> {
    let home = home_dir();
    app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        if s.remote().is_some() || s.is_dynamic_ssh() {
            return None;
        }
        let vte = s.active_vte();
        let pane = s.pane_registry.by_vte(vte);
        if pane.is_some_and(|p| !p.proc_info.ssh_host.is_empty()) {
            return None;
        }
        let cwd = s.active_local_cwd()?;
        let (root, _) = resolve_git_repo(&cwd, &home)?;
        let key = pane
            .map(|p| pane_key_for_id(p.id.raw()))
            .unwrap_or_else(|| "default".to_owned());
        Some((root, key))
    })
    .flatten()
}

fn active_pane_key() -> String {
    app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        let pane = s.pane_registry.by_vte(s.active_vte())?;
        Some(pane_key_for_id(pane.id.raw()))
    })
    .flatten()
    .unwrap_or_else(|| "default".to_owned())
}

fn make_cp_section_lbl(title: &CStr) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(title.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-label");
        lbl
    }
}

fn make_cp_muted_lbl(content: &str) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "muted-note");
        set_label_text(lbl, content);
        lbl
    }
}

fn set_widget_tooltip(widget: *mut GtkWidget, text: &str) {
    if let Ok(cstr) = std::ffi::CString::new(text.replace('\0', "")) {
        unsafe { gtk_widget_set_tooltip_text(widget, cstr.as_ptr()) };
    }
}

fn make_checkpoint_label(entry: &CheckpointEntry, now_ms: u128) -> *mut GtkWidget {
    let summary = format_checkpoint_row(entry, now_ms);
    let tip = format!("{} ({})\n{}\n{}", entry.short_sha, entry.label, entry.stat_summary, entry.ref_name);
    unsafe {
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        gtk_label_set_ellipsize(lbl, PANGO_ELLIPSIZE_END);
        gtk_label_set_max_width_chars(lbl, 34);
        add_class(lbl, "muted-note");
        set_label_text(lbl, &summary);
        set_widget_tooltip(lbl, &tip);
        lbl
    }
}

fn make_checkpoint_row(entry: &CheckpointEntry, idx: usize, now_ms: u128) -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let lbl = make_checkpoint_label(entry, now_ms);
        let undo_btn = gtk_button_new_with_label(ui_text::CP_UNDO_BTN.as_ptr());
        gtk_widget_set_tooltip_text(undo_btn, ui_text::CP_UNDO_TIP.as_ptr());
        connect_click(undo_btn, on_undo_checkpoint_clicked, idx as *mut c_void);
        gtk_box_pack_start(row, lbl, 1, 1, 0);
        gtk_box_pack_end(row, undo_btn, 0, 0, 0);
        row
    }
}

fn wrap_checkpoints_scroll(list: *mut GtkWidget, count: usize) -> *mut GtkWidget {
    if count <= 4 {
        return list;
    }
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, 360);
        gtk_scrolled_window_set_min_content_height(scroll, 110);
        gtk_scrolled_window_set_max_content_height(scroll, 180);
        gtk_container_add(scroll, list);
        scroll
    }
}

fn store_popover_ui_state(popover: *mut GtkWidget, repo_root: &str, pane_key: String, visible: &[CheckpointEntry]) {
    CP_UI.with(|cell| {
        let mut st = cell.borrow_mut();
        st.popover = popover as usize;
        st.repo_root = repo_root.to_owned();
        st.pane_key = pane_key;
        st.refs = visible.iter().map(|e| e.ref_name.clone()).collect();
    });
}

fn pack_popover_list(vbox: *mut GtkWidget, visible: &[CheckpointEntry]) {
    if visible.is_empty() {
        unsafe { gtk_box_pack_start(vbox, make_cp_muted_lbl(ui_text::CP_EMPTY.as_str()), 0, 1, 1) };
        return;
    }
    let now_ms = now_epoch_ms();
    let list_box = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 3) };
    for (idx, entry) in visible.iter().enumerate() {
        unsafe { gtk_box_pack_start(list_box, make_checkpoint_row(entry, idx, now_ms), 0, 1, 0) };
    }
    unsafe { gtk_box_pack_start(vbox, wrap_checkpoints_scroll(list_box, visible.len()), 0, 1, 0) };
}

pub fn pack_popover_section(vbox: *mut GtkWidget, popover: *mut GtkWidget, repo_root: &str) {
    let pane_key = active_pane_key();
    let entries = list_checkpoints(Path::new(repo_root), &pane_key).unwrap_or_default();
    let visible: Vec<CheckpointEntry> = entries.into_iter().take(POPOVER_VISIBLE_LIMIT).collect();
    store_popover_ui_state(popover, repo_root, pane_key, &visible);
    unsafe {
        gtk_box_pack_start(vbox, make_cp_section_lbl(ui_text::CP_SECTION_HEADER.cstr()), 0, 0, 0);
        let save_btn = gtk_button_new_with_label(ui_text::CP_SAVE_BTN.as_ptr());
        gtk_widget_set_tooltip_text(save_btn, ui_text::CP_SAVE_TIP.as_ptr());
        connect_click(save_btn, on_save_checkpoint_clicked, ptr::null_mut());
        gtk_box_pack_start(vbox, save_btn, 0, 1, 1);
    }
    pack_popover_list(vbox, &visible);
}

fn format_capture_message(outcome: Result<CaptureOutcome, String>) -> String {
    match outcome {
        Ok(CaptureOutcome::Saved(entry)) => {
            format!("{} {} ({})", ui_text::CP_MSG_SAVED.as_str(), entry.short_sha, entry.label)
        }
        Ok(CaptureOutcome::SkippedClean) => ui_text::CP_MSG_CLEAN.as_str().to_owned(),
        Ok(CaptureOutcome::SkippedDuplicate(_)) => ui_text::CP_MSG_DUPLICATE.as_str().to_owned(),
        Ok(CaptureOutcome::SkippedNotRepo) => ui_text::CP_MSG_LOCAL_ONLY.as_str().to_owned(),
        Err(err) => err,
    }
}

fn format_restore_message(outcome: Result<RestoreOutcome, String>) -> String {
    match outcome {
        Ok(done) => format!("{} {}", ui_text::CP_MSG_RESTORED.as_str(), done.short_sha),
        Err(err) => err,
    }
}

extern "C" fn on_cp_action_done(data: *mut c_void) -> c_int {
    if data.is_null() {
        return 0;
    }
    let done = unsafe { *Box::from_raw(data as *mut CpActionDone) };
    app_state::set_footer_message(&done.message);
    if !done.repo_root.is_empty() {
        app_state::with_app(|app| app.git_scheduler.request_refresh(&done.repo_root));
    }
    topbar::refresh_ui();
    if done.refresh_popover {
        pane_actions::refresh_open_git_popover();
    } else {
        app_state::focus_active_vte();
    }
    0
}

fn dispatch_cp_bg<F>(repo_root: String, refresh_popover: bool, op: F)
where
    F: FnOnce(&Path) -> String + Send + 'static,
{
    thread::spawn(move || {
        let message = op(Path::new(&repo_root));
        let payload = Box::new(CpActionDone { repo_root, message, refresh_popover });
        unsafe { g_idle_add(on_cp_action_done, Box::into_raw(payload) as *mut c_void) };
    });
}

fn popdown_cp_popover() {
    let pop = CP_UI.with(|cell| cell.borrow().popover as *mut GtkWidget);
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
}

extern "C" fn on_save_checkpoint_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let (repo_root, pane_key) = CP_UI.with(|cell| {
        let st = cell.borrow();
        (st.repo_root.clone(), st.pane_key.clone())
    });
    if repo_root.is_empty() {
        app_state::set_footer_message(ui_text::CP_MSG_LOCAL_ONLY.as_str());
        return;
    }
    dispatch_cp_bg(repo_root, true, move |root| {
        format_capture_message(capture_checkpoint(root, &pane_key, "manual"))
    });
}

extern "C" fn on_undo_checkpoint_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let idx = data as usize;
    let (repo_root, pane_key, target_ref) = CP_UI.with(|cell| {
        let st = cell.borrow();
        (st.repo_root.clone(), st.pane_key.clone(), st.refs.get(idx).cloned())
    });
    popdown_cp_popover();
    let Some(ref_name) = target_ref else {
        app_state::set_footer_message(ui_text::CP_MSG_NONE.as_str());
        return;
    };
    dispatch_cp_bg(repo_root, false, move |root| {
        format_restore_message(restore_checkpoint(root, &pane_key, &ref_name))
    });
}

pub fn trigger_save_for_active() {
    let Some((repo_root, pane_key)) = active_local_target() else {
        app_state::set_footer_message(ui_text::CP_MSG_LOCAL_ONLY.as_str());
        app_state::focus_active_vte();
        return;
    };
    dispatch_cp_bg(repo_root, true, move |root| {
        format_capture_message(capture_checkpoint(root, &pane_key, "manual"))
    });
}

pub fn trigger_undo_last_for_active() {
    let Some((repo_root, pane_key)) = active_local_target() else {
        app_state::set_footer_message(ui_text::CP_MSG_LOCAL_ONLY.as_str());
        app_state::focus_active_vte();
        return;
    };
    dispatch_cp_bg(repo_root, false, move |root| {
        format_restore_message(undo_last_checkpoint(root, &pane_key))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRepo {
        dir: PathBuf,
    }

    impl TempRepo {
        fn empty(tag: &str) -> Self {
            let seq = CHECKPOINT_SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "akhterm_cp_{tag}_{}_{seq}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create temp dir");
            Self { dir }
        }

        fn init_with_commit(tag: &str) -> Self {
            let repo = Self::empty(tag);
            repo.git(&["init", "-b", "main"]);
            repo.git(&["config", "user.name", "Test User"]);
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.write("app.rs", "fn main() { println!(\"v0\"); }\n");
            repo.write("staged.rs", "pub const VER: u32 = 0;\n");
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "initial"]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            let out = run_git(&self.dir, args).expect("spawn git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }

        fn write(&self, rel: &str, content: &str) {
            let full = self.dir.join(rel);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(full, content).expect("write file");
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.dir.join(rel)).expect("read file")
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn capture_skips_non_repo_and_clean_tree() {
        let plain = TempRepo::empty("nonrepo");
        let res = capture_checkpoint(&plain.dir, "pane-1", "manual").unwrap();
        assert_eq!(res, CaptureOutcome::SkippedNotRepo);

        let repo = TempRepo::init_with_commit("clean");
        let res2 = capture_checkpoint(&repo.dir, "pane-1", "manual").unwrap();
        assert_eq!(res2, CaptureOutcome::SkippedClean);
        assert!(list_checkpoints(&repo.dir, "pane-1").unwrap().is_empty());
    }

    fn unwrap_saved(out: CaptureOutcome) -> CheckpointEntry {
        match out {
            CaptureOutcome::Saved(entry) => entry,
            other => panic!("expected Saved, got {other:?}"),
        }
    }

    fn seed_pre_turn_changes(repo: &TempRepo) {
        repo.write("app.rs", "pre-turn-tracked\n");
        repo.write("staged.rs", "pub const VER: u32 = 42;\n");
        repo.git(&["add", "staged.rs"]);
        repo.write("notes.txt", "pre-turn-untracked\n");
    }

    #[test]
    fn capture_leaves_head_branch_index_and_worktree_untouched() {
        let repo = TempRepo::init_with_commit("untouched");
        seed_pre_turn_changes(&repo);
        let snap = (
            repo.git(&["rev-parse", "HEAD"]),
            repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]),
            repo.git(&["diff", "--cached"]),
            repo.git(&["status", "--porcelain"]),
        );
        let saved = unwrap_saved(capture_checkpoint(&repo.dir, "pane-7", "pre-tool").unwrap());
        assert!(saved.ref_name.starts_with("refs/akhterm/checkpoints/pane-7/"));
        assert_eq!(saved.label, "pre-tool");
        assert!(saved.stat_summary.contains("untracked"));
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), snap.0);
        assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), snap.1);
        assert_eq!(repo.git(&["diff", "--cached"]), snap.2);
        assert_eq!(repo.git(&["status", "--porcelain"]), snap.3);
    }

    #[test]
    fn capture_deduplicates_identical_state_and_prunes_above_limit() {
        let repo = TempRepo::init_with_commit("dedup_prune");
        repo.write("app.rs", "change 1\n");
        let _ = unwrap_saved(capture_checkpoint(&repo.dir, "pane-3", "turn-1").unwrap());
        let dup = capture_checkpoint(&repo.dir, "pane-3", "turn-1-again").unwrap();
        assert!(matches!(dup, CaptureOutcome::SkippedDuplicate(_)));
        for i in 2..=23 {
            repo.write("app.rs", &format!("change {i}\n"));
            let _ = unwrap_saved(capture_checkpoint(&repo.dir, "pane-3", &format!("turn-{i}")).unwrap());
        }
        let listed = list_checkpoints(&repo.dir, "pane-3").unwrap();
        assert_eq!(listed.len(), MAX_CHECKPOINTS_PER_PANE);
        assert_eq!((listed[0].label.as_str(), listed[19].label.as_str()), ("turn-23", "turn-4"));
    }

    #[test]
    fn restore_restores_tracked_staged_and_untracked_with_safety_snapshot() {
        let repo = TempRepo::init_with_commit("restore_full");
        seed_pre_turn_changes(&repo);
        let saved = unwrap_saved(capture_checkpoint(&repo.dir, "pane-9", "pre-tool").unwrap());
        repo.write("app.rs", "broken-agent-edit\n");
        repo.write("staged.rs", "pub const VER: u32 = 999;\n");
        repo.git(&["add", "staged.rs"]);
        repo.write("notes.txt", "corrupted-untracked\n");

        let done = undo_last_checkpoint(&repo.dir, "pane-9").unwrap();
        assert_eq!(done.restored_ref, saved.ref_name);
        let safety_ref = done.safety_ref.expect("safety pre-undo checkpoint");
        assert_eq!(repo.read("app.rs"), "pre-turn-tracked\n");
        assert_eq!(repo.read("staged.rs"), "pub const VER: u32 = 42;\n");
        assert!(repo.git(&["diff", "--cached", "staged.rs"]).contains("42"));
        assert_eq!(repo.read("notes.txt"), "pre-turn-untracked\n");
        assert!(repo.git(&["status", "--porcelain"]).contains("?? notes.txt"));

        let redo = restore_checkpoint(&repo.dir, "pane-9", &safety_ref).unwrap();
        assert_eq!(redo.restored_ref, safety_ref);
        assert_eq!(repo.read("app.rs"), "broken-agent-edit\n");
        assert_eq!(repo.read("notes.txt"), "corrupted-untracked\n");
    }

    #[test]
    fn restore_missing_ref_returns_clear_error() {
        let repo = TempRepo::init_with_commit("missing_ref");
        let err = restore_checkpoint(&repo.dir, "pane-1", "refs/akhterm/checkpoints/pane-1/missing")
            .unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn hook_filter_and_debounce_window_work_deterministically() {
        assert!(should_trigger_hook_checkpoint(HookState::Working, None, true, "/repo"));
        assert!(should_trigger_hook_checkpoint(HookState::Waiting, Some("PreToolUse"), true, "/repo"));
        assert!(!should_trigger_hook_checkpoint(HookState::Done, None, true, "/repo"));
        assert!(!should_trigger_hook_checkpoint(HookState::Working, None, false, "/repo"));
        assert!(!should_trigger_hook_checkpoint(HookState::Working, None, true, ""));

        let mut map = HookDebounceMap::default();
        let t0 = Instant::now();
        assert!(map.try_acquire(1, t0, Duration::from_secs(10)));
        assert!(!map.try_acquire(1, t0 + Duration::from_secs(12), Duration::from_secs(10)));
        map.release(1);
        assert!(!map.try_acquire(1, t0 + Duration::from_secs(5), Duration::from_secs(10)));
        assert!(map.try_acquire(1, t0 + Duration::from_secs(11), Duration::from_secs(10)));
    }

    #[test]
    fn capture_and_restore_untracked_only_changes() {
        let repo = TempRepo::init_with_commit("untracked_only");
        repo.write("draft.md", "first draft\n");
        let saved = match capture_checkpoint(&repo.dir, "pane-4", "manual").unwrap() {
            CaptureOutcome::Saved(entry) => entry,
            other => panic!("expected Saved, got {other:?}"),
        };
        assert_eq!(saved.stat_summary, "+1 untracked");
        repo.write("draft.md", "overwritten by agent\n");
        let done = undo_last_checkpoint(&repo.dir, "pane-4").unwrap();
        assert_eq!(done.restored_ref, saved.ref_name);
        assert_eq!(repo.read("draft.md"), "first draft\n");
    }

    #[test]
    fn palette_exposes_bilingual_checkpoint_actions() {
        let src = crate::palette::PaletteSource {
            sessions: &[], tmux_items: &[], profiles: &[], ssh_hosts: &[],
            briefs_dir: None, extra_handoffs: &[], active_git: None, active_is_worktree: false,
        };
        for loc in [crate::i18n::UiLocale::Ru, crate::i18n::UiLocale::En] {
            let items = crate::palette::collect_items_for_locale(&src, loc);
            assert!(items.iter().any(|i| i.action == crate::palette::PaletteAction::SaveCheckpoint));
            assert!(items.iter().any(|i| i.action == crate::palette::PaletteAction::UndoLastCheckpoint));
        }
    }
}
