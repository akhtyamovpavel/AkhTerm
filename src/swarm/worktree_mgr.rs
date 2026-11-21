//! Git worktree listing, safe sibling creation, guarded removal, and merged pruning.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::ui_text as text;

const MAX_SLUG_LEN: usize = 64;
const MAX_REF_LEN: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head_sha: String,
    pub is_primary: bool,
    pub is_detached: bool,
    pub is_dirty: bool,
    pub is_merged: bool,
}

impl WorktreeEntry {
    pub fn short_sha(&self) -> &str {
        let len = self.head_sha.len().min(7);
        &self.head_sha[..len]
    }

    pub fn display_name(&self) -> String {
        if let Some(ref branch) = self.branch {
            return branch.clone();
        }
        let sha = self.short_sha();
        if sha.is_empty() {
            text::WT_DETACHED_TAG.as_str().to_owned()
        } else {
            format!("{} ({sha})", text::WT_DETACHED_TAG.as_str())
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub removed: Vec<WorktreeEntry>,
    pub skipped_dirty: usize,
    pub skipped_unmerged: usize,
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("git error: {err}"))
}

fn first_stderr_line(out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git command failed")
        .to_owned()
}

fn strip_branch_prefix(raw: &str) -> String {
    raw.strip_prefix("refs/heads/")
        .or_else(|| raw.strip_prefix("refs/"))
        .unwrap_or(raw)
        .to_owned()
}

fn apply_stanza_line(entry: &mut WorktreeEntry, line: &str) {
    if let Some(val) = line.strip_prefix("worktree ") {
        entry.path = PathBuf::from(val);
    } else if let Some(val) = line.strip_prefix("HEAD ") {
        entry.head_sha = val.trim().to_owned();
    } else if let Some(val) = line.strip_prefix("branch ") {
        entry.branch = Some(strip_branch_prefix(val.trim()));
    } else if line == "detached" {
        entry.is_detached = true;
    }
}

fn parse_stanza(stanza: &str, is_primary: bool) -> Option<WorktreeEntry> {
    let mut entry = WorktreeEntry {
        path: PathBuf::new(),
        branch: None,
        head_sha: String::new(),
        is_primary,
        is_detached: false,
        is_dirty: false,
        is_merged: false,
    };
    for raw_line in stanza.lines() {
        apply_stanza_line(&mut entry, raw_line.trim_end_matches('\r'));
    }
    (!entry.path.as_os_str().is_empty()).then_some(entry)
}

pub fn parse_worktree_list(porcelain: &str) -> Vec<WorktreeEntry> {
    let normalized = porcelain.replace("\r\n", "\n");
    let mut entries = Vec::new();
    for stanza in normalized.split("\n\n").filter(|s| !s.trim().is_empty()) {
        if let Some(entry) = parse_stanza(stanza, entries.is_empty()) {
            entries.push(entry);
        }
    }
    entries
}

fn has_forbidden_edges(s: &str, prefixes: &[&str], suffixes: &[&str], forbidden: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
        || suffixes.iter().any(|p| s.ends_with(p))
        || forbidden.iter().any(|p| s.contains(p))
}

fn is_valid_slug_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

pub fn validate_branch_slug(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(text::WT_ERR_EMPTY_SLUG.as_str().to_owned());
    }
    let bad = raw.contains('\0')
        || trimmed.len() > MAX_SLUG_LEN
        || has_forbidden_edges(trimmed, &["-", "."], &[".", ".lock"], &[".."])
        || !trimmed.chars().all(is_valid_slug_char);
    if bad {
        return Err(text::WT_ERR_BAD_SLUG.as_str().to_owned());
    }
    Ok(trimmed.to_owned())
}

fn is_valid_ref_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/')
}

pub fn validate_base_ref(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() && !raw.contains('\0') {
        return Ok("HEAD".to_owned());
    }
    let bad = raw.contains('\0')
        || trimmed.len() > MAX_REF_LEN
        || has_forbidden_edges(trimmed, &["-", "/", "."], &["/", ".", ".lock"], &["..", "//"])
        || !trimmed.chars().all(is_valid_ref_char);
    if bad {
        return Err(text::WT_ERR_BAD_BASE.as_str().to_owned());
    }
    Ok(trimmed.to_owned())
}

pub fn worktree_path_for(repo_root: &Path, slug: &str) -> Result<PathBuf, String> {
    let valid_slug = validate_branch_slug(slug)?;
    let parent = repo_root
        .parent()
        .ok_or_else(|| text::WT_ERR_NOT_REPO.as_str().to_owned())?;
    let name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| text::WT_ERR_NOT_REPO.as_str().to_owned())?;
    Ok(parent.join(format!(".{name}-wt")).join(valid_slug))
}

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn check_worktree_dirty(wt_path: &Path) -> bool {
    if !wt_path.is_dir() {
        return false;
    }
    match run_git(wt_path, &["status", "--porcelain"]) {
        Ok(out) if out.status.success() => !out.stdout.is_empty(),
        _ => true,
    }
}

fn primary_merge_ref(primary: &WorktreeEntry) -> Option<String> {
    if let Some(ref branch) = primary.branch {
        return Some(format!("refs/heads/{branch}"));
    }
    (!primary.head_sha.is_empty()).then(|| primary.head_sha.clone())
}

fn check_entry_merged(primary_root: &Path, base_ref: &str, entry: &WorktreeEntry) -> bool {
    if entry.is_primary {
        return false;
    }
    let target = match entry.branch {
        Some(ref b) => format!("refs/heads/{b}"),
        None if !entry.head_sha.is_empty() => entry.head_sha.clone(),
        None => return false,
    };
    run_git(primary_root, &["merge-base", "--is-ancestor", &target, base_ref])
        .is_ok_and(|out| out.status.success())
}

fn enrich_entries(entries: &mut [WorktreeEntry]) {
    let Some(primary) = entries.first().cloned() else { return };
    let merge_ref = primary_merge_ref(&primary);
    for entry in entries {
        entry.is_dirty = check_worktree_dirty(&entry.path);
        entry.is_merged = merge_ref.as_deref().is_some_and(|b| check_entry_merged(&primary.path, b, entry));
    }
}

pub fn list_worktrees(repo_root: &Path) -> Result<Vec<WorktreeEntry>, String> {
    if !repo_root.is_dir() {
        return Err(text::WT_ERR_NOT_REPO.as_str().to_owned());
    }
    let out = run_git(repo_root, &["worktree", "list", "--porcelain"])?;
    if !out.status.success() {
        return Err(text::WT_ERR_NOT_REPO.as_str().to_owned());
    }
    let mut entries = parse_worktree_list(&String::from_utf8_lossy(&out.stdout));
    enrich_entries(&mut entries);
    Ok(entries)
}

fn primary_from_entries(entries: &[WorktreeEntry]) -> Result<PathBuf, String> {
    entries.first().map(|e| e.path.clone()).ok_or_else(|| text::WT_ERR_NOT_REPO.as_str().to_owned())
}

fn prepare_target_dir(primary: &Path, slug: &str) -> Result<PathBuf, String> {
    let target = worktree_path_for(primary, slug)?;
    if target.exists() {
        return Err(text::WT_ERR_PATH_EXISTS.as_str().to_owned());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("mkdir error: {err}"))?;
    }
    Ok(target)
}

fn exec_worktree_add(primary: &Path, slug: &str, target: &Path, base: &str) -> Result<(), String> {
    let target_str = target.to_str().ok_or_else(|| text::WT_ERR_BAD_SLUG.as_str().to_owned())?;
    let out = run_git(primary, &["worktree", "add", "-b", slug, target_str, base])?;
    if out.status.success() { Ok(()) } else { Err(first_stderr_line(&out)) }
}

pub fn create_worktree(repo_root: &Path, branch_slug: &str, base_ref: &str) -> Result<WorktreeEntry, String> {
    let slug = validate_branch_slug(branch_slug)?;
    let base = validate_base_ref(base_ref)?;
    let primary = primary_from_entries(&list_worktrees(repo_root)?)?;
    let target = prepare_target_dir(&primary, &slug)?;
    exec_worktree_add(&primary, &slug, &target, &base)?;
    list_worktrees(&primary)?
        .into_iter()
        .find(|entry| same_path(&entry.path, &target))
        .ok_or_else(|| text::WT_ERR_NOT_FOUND.as_str().to_owned())
}

fn delete_branch_if_merged(primary: &Path, entry: &WorktreeEntry) {
    if entry.is_merged {
        if let Some(ref branch) = entry.branch {
            let _ = run_git(primary, &["branch", "-d", branch]);
        }
    }
}

fn exec_remove_entry(primary: &Path, entry: &WorktreeEntry) -> Result<(), String> {
    let wt_str = entry.path.to_str().ok_or_else(|| text::WT_ERR_NOT_FOUND.as_str().to_owned())?;
    let out = run_git(primary, &["worktree", "remove", wt_str])?;
    if !out.status.success() {
        return Err(first_stderr_line(&out));
    }
    delete_branch_if_merged(primary, entry);
    Ok(())
}

fn guard_removable(entry: &WorktreeEntry) -> Result<(), String> {
    if entry.is_primary {
        return Err(text::WT_ERR_PRIMARY_REMOVE.as_str().to_owned());
    }
    if entry.is_dirty {
        return Err(text::WT_ERR_DIRTY_REMOVE.as_str().to_owned());
    }
    Ok(())
}

pub fn remove_worktree(repo_root: &Path, wt_path: &Path) -> Result<WorktreeEntry, String> {
    let entries = list_worktrees(repo_root)?;
    let primary = primary_from_entries(&entries)?;
    let entry = entries
        .into_iter()
        .find(|item| same_path(&item.path, wt_path))
        .ok_or_else(|| text::WT_ERR_NOT_FOUND.as_str().to_owned())?;
    guard_removable(&entry)?;
    exec_remove_entry(&primary, &entry)?;
    Ok(entry)
}

fn prune_one_entry(primary: &Path, entry: WorktreeEntry, report: &mut PruneReport) {
    if entry.is_dirty {
        report.skipped_dirty += 1;
    } else if !entry.is_merged {
        report.skipped_unmerged += 1;
    } else if exec_remove_entry(primary, &entry).is_ok() {
        report.removed.push(entry);
    }
}

pub fn prune_merged_worktrees(repo_root: &Path) -> Result<PruneReport, String> {
    let entries = list_worktrees(repo_root)?;
    let primary = primary_from_entries(&entries)?;
    let mut report = PruneReport::default();
    for entry in entries.into_iter().filter(|item| !item.is_primary) {
        prune_one_entry(&primary, entry, &mut report);
    }
    let out = run_git(&primary, &["worktree", "prune"])?;
    if out.status.success() { Ok(report) } else { Err(first_stderr_line(&out)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_SEQ: AtomicU32 = AtomicU32::new(1);

    struct TempRepo {
        root: PathBuf,
        repo: PathBuf,
    }

    impl TempRepo {
        fn new(tag: &str) -> Self {
            let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "akhterm_wtmgr_{tag}_{}_{}",
                std::process::id(),
                seq
            ));
            let _ = fs::remove_dir_all(&root);
            let repo = root.join("demo");
            fs::create_dir_all(&repo).unwrap();
            git_ok(&repo, &["init", "-b", "main"]);
            fs::write(repo.join("README.md"), "hello\n").unwrap();
            git_commit_all(&repo, "init");
            Self { root, repo }
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn git_ok(cwd: &Path, args: &[&str]) {
        let out = run_git(cwd, args).unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_commit_all(cwd: &Path, msg: &str) {
        git_ok(cwd, &["add", "."]);
        git_ok(
            cwd,
            &[
                "-c", "commit.gpgsign=false",
                "-c", "user.name=Test",
                "-c", "user.email=test@example.com",
                "commit", "-m", msg,
            ],
        );
    }

    #[test]
    fn parses_porcelain_primary_branch_and_detached_entries() {
        let sample = concat!(
            "worktree /tmp/repo\nHEAD 11111112222222\nbranch refs/heads/main\n\n",
            "worktree /tmp/.repo-wt/feat\nHEAD 33333334444444\nbranch refs/heads/feat\n\n",
            "worktree /tmp/.repo-wt/det\nHEAD 55555556666666\ndetached\n"
        );
        let list = parse_worktree_list(sample);
        assert_eq!(list.len(), 3);
        assert!(list[0].is_primary);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert!(!list[1].is_primary && !list[1].is_detached);
        assert_eq!(list[1].branch.as_deref(), Some("feat"));
        assert!(list[2].is_detached && list[2].branch.is_none());
        assert_eq!(list[2].short_sha(), "5555555");
    }

    #[test]
    fn validates_slug_and_builds_deterministic_sibling_path() {
        assert_eq!(validate_branch_slug("feat-105").unwrap(), "feat-105");
        assert_eq!(validate_branch_slug("fix_v1.5").unwrap(), "fix_v1.5");
        for bad in [
            "", "   ", "..", "a..b", "feat/sub", "a\\b", "-flag", ".hidden",
            "name.lock", "two words", "a;rm", "$(id)", "a\0b", "x|y",
        ] {
            assert!(validate_branch_slug(bad).is_err(), "expected reject for {bad:?}");
        }
        assert_eq!(validate_base_ref("").unwrap(), "HEAD");
        assert_eq!(validate_base_ref("origin/main").unwrap(), "origin/main");
        assert!(validate_base_ref("-bad").is_err());
        assert!(validate_base_ref("a..b").is_err());
        let p = worktree_path_for(Path::new("/work/akhterm-rs"), "v15-wt").unwrap();
        assert_eq!(p, PathBuf::from("/work/.akhterm-rs-wt/v15-wt"));
    }

    fn setup_merge_fixture(tmp: &TempRepo) -> (WorktreeEntry, WorktreeEntry, WorktreeEntry) {
        let wt_merged = create_worktree(&tmp.repo, "wt-merged", "main").unwrap();
        fs::write(wt_merged.path.join("m.txt"), "merged\n").unwrap();
        git_commit_all(&wt_merged.path, "add merged");
        git_ok(&tmp.repo, &["merge", "--no-ff", "-m", "merge wt-merged", "wt-merged"]);

        let wt_dirty = create_worktree(&tmp.repo, "wt-merged-dirty", "main").unwrap();
        fs::write(wt_dirty.path.join("wip.txt"), "dirty\n").unwrap();

        let wt_unmerged = create_worktree(&tmp.repo, "wt-unmerged", "main").unwrap();
        fs::write(wt_unmerged.path.join("u.txt"), "unmerged\n").unwrap();
        git_commit_all(&wt_unmerged.path, "add unmerged");
        (wt_merged, wt_dirty, wt_unmerged)
    }

    fn find_by_branch<'a>(entries: &'a [WorktreeEntry], name: &str) -> &'a WorktreeEntry {
        entries.iter().find(|e| e.branch.as_deref() == Some(name)).unwrap()
    }

    #[test]
    fn creates_and_lists_sibling_worktree() {
        let tmp = TempRepo::new("create_list");
        let created = create_worktree(&tmp.repo, "feat-alpha", "HEAD").unwrap();
        assert!(!created.is_primary);
        assert_eq!(created.branch.as_deref(), Some("feat-alpha"));
        assert!(created.path.ends_with(".demo-wt/feat-alpha"));
        let listed = list_worktrees(&tmp.repo).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].is_primary && !listed[1].is_dirty);
    }

    #[test]
    fn guards_primary_and_dirty_worktrees_on_remove() {
        let tmp = TempRepo::new("guard_remove");
        let created = create_worktree(&tmp.repo, "feat-beta", "HEAD").unwrap();
        assert!(remove_worktree(&tmp.repo, &tmp.repo).is_err());
        fs::write(created.path.join("untracked.txt"), "wip\n").unwrap();
        assert!(remove_worktree(&tmp.repo, &created.path).is_err());
        fs::remove_file(created.path.join("untracked.txt")).unwrap();
        let removed = remove_worktree(&tmp.repo, &created.path).unwrap();
        assert_eq!(removed.branch.as_deref(), Some("feat-beta"));
        assert!(!created.path.exists());
    }

    #[test]
    fn detects_merged_and_dirty_worktree_flags() {
        let tmp = TempRepo::new("detect_merged");
        let _ = setup_merge_fixture(&tmp);
        let list = list_worktrees(&tmp.repo).unwrap();
        let merged = find_by_branch(&list, "wt-merged");
        let unmerged = find_by_branch(&list, "wt-unmerged");
        let dirty = find_by_branch(&list, "wt-merged-dirty");
        assert!(merged.is_merged && !merged.is_dirty);
        assert!(!unmerged.is_merged && !unmerged.is_dirty);
        assert!(dirty.is_merged && dirty.is_dirty);
    }

    #[test]
    fn prunes_only_clean_merged_worktrees() {
        let tmp = TempRepo::new("prune_merged");
        let (wt_merged, wt_dirty, wt_unmerged) = setup_merge_fixture(&tmp);
        let report = prune_merged_worktrees(&tmp.repo).unwrap();
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].branch.as_deref(), Some("wt-merged"));
        assert_eq!((report.skipped_dirty, report.skipped_unmerged), (1, 1));
        assert!(!wt_merged.path.exists() && wt_dirty.path.is_dir() && wt_unmerged.path.is_dir());
    }
}
