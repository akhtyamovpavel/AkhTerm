//! Fast filesystem Git HEAD / worktree detection and isolated agent worktree creation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths::home_dir;

static WORKTREE_SEQ: AtomicU32 = AtomicU32::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHeadInfo {
    pub head: String,
    pub is_linked_worktree: bool,
}

impl GitHeadInfo {
    pub fn badge(&self) -> String {
        if self.is_linked_worktree {
            format!("⎇ {} [wt]", self.head)
        } else {
            format!("⎇ {}", self.head)
        }
    }
}

pub fn detect_git_head(dir: &Path) -> Option<GitHeadInfo> {
    let home = home_dir();
    detect_git_head_bounded(dir, Path::new(&home))
}

pub fn detect_git_repo(dir: &Path) -> Option<(PathBuf, GitHeadInfo)> {
    let home = home_dir();
    detect_git_repo_bounded(dir, Path::new(&home))
}

pub fn detect_git_repo_bounded(dir: &Path, home: &Path) -> Option<(PathBuf, GitHeadInfo)> {
    let stop_dir = home.parent().filter(|p| *p != Path::new("/"));
    for ancestor in dir.ancestors().take(32) {
        if ancestor.parent().is_none() || Some(ancestor) == stop_dir {
            break;
        }
        if let Some(info) = inspect_dot_git(&ancestor.join(".git")) {
            return Some((ancestor.to_path_buf(), info));
        }
    }
    None
}

fn detect_git_head_bounded(dir: &Path, home: &Path) -> Option<GitHeadInfo> {
    detect_git_repo_bounded(dir, home).map(|(_, info)| info)
}

fn inspect_dot_git(dot_git: &Path) -> Option<GitHeadInfo> {
    let meta = fs::metadata(dot_git).ok()?;
    let (git_dir, is_linked_worktree) = if meta.is_dir() {
        (dot_git.to_path_buf(), false)
    } else if meta.is_file() {
        resolve_gitdir_file(dot_git)?
    } else {
        return None;
    };
    let raw = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = parse_head_ref(&raw)?;
    Some(GitHeadInfo { head, is_linked_worktree })
}

fn resolve_gitdir_file(dot_git: &Path) -> Option<(PathBuf, bool)> {
    let content = fs::read_to_string(dot_git).ok()?;
    let target = content.lines().find_map(|l| l.trim().strip_prefix("gitdir:"))?.trim();
    if target.is_empty() {
        return None;
    }
    let resolved = resolve_relative_gitdir(dot_git, Path::new(target))?;
    let is_wt = !target.contains("/modules/") && !target.contains("\\modules\\");
    Some((resolved, is_wt))
}

fn resolve_relative_gitdir(dot_git: &Path, raw_path: &Path) -> Option<PathBuf> {
    if raw_path.is_absolute() {
        Some(raw_path.to_path_buf())
    } else {
        Some(dot_git.parent()?.join(raw_path))
    }
}

pub fn parse_head_ref(raw: &str) -> Option<String> {
    let line = raw.lines().next()?.trim();
    if let Some(rest) = line.strip_prefix("ref:") {
        return parse_symbolic_ref(rest.trim());
    }
    let is_sha = line.len() >= 7 && line.bytes().all(|b| b.is_ascii_hexdigit());
    is_sha.then(|| line[..7].to_ascii_lowercase())
}

fn parse_symbolic_ref(target: &str) -> Option<String> {
    let branch = target
        .strip_prefix("refs/heads/")
        .or_else(|| target.strip_prefix("refs/"))
        .unwrap_or(target);
    (!branch.is_empty()).then(|| branch.to_owned())
}

pub fn agent_slug_from_cmd(command: &str) -> String {
    let first = command.split_whitespace().next().unwrap_or("agent");
    let exe = Path::new(first).file_name().and_then(|s| s.to_str()).unwrap_or(first);
    sanitize_agent_slug(exe)
}

fn sanitize_agent_slug(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch.is_whitespace()) && !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let clean = out.trim_matches('-');
    if clean.is_empty() { "agent".to_owned() } else { clean.to_owned() }
}

fn next_short_stamp() -> String {
    let seq = WORKTREE_SEQ.fetch_add(1, Ordering::Relaxed);
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32);
    format!("{:05x}", ((secs & 0xfff) << 8) | (seq & 0xff))
}

fn run_git_in(cwd: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Не удалось запустить git: {e}"))
}

fn resolve_repo_root(cwd: &Path) -> Result<PathBuf, String> {
    if !cwd.is_dir() {
        return Err("Каталог вкладки не найден".to_owned());
    }
    let out = run_git_in(cwd, &["rev-parse", "--show-toplevel"])?;
    if !out.status.success() {
        return Err("Текущий каталог не является Git-репозиторием".to_owned());
    }
    let root = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!root.is_empty())
        .then(|| PathBuf::from(root))
        .ok_or_else(|| "Не удалось определить корень репозитория".to_owned())
}

fn build_worktree_paths(repo_root: &Path, slug: &str, stamp: &str) -> Result<(PathBuf, String), String> {
    let parent = repo_root
        .parent()
        .ok_or_else(|| "Не найден родительский каталог репозитория".to_owned())?;
    let name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "Некорректное имя каталога репозитория".to_owned())?;
    Ok((parent.join(format!("{name}-wt-{slug}-{stamp}")), format!("wt/{slug}-{stamp}")))
}

fn run_worktree_add(repo_root: &Path, wt_path: &Path, branch: &str) -> Result<(), String> {
    let wt_str = wt_path.to_str().ok_or_else(|| "Некорректный путь worktree".to_owned())?;
    let out = run_git_in(repo_root, &["worktree", "add", "-b", branch, wt_str])?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail = stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("ошибка git");
    Err(format!("Не удалось создать worktree: {detail}"))
}

pub fn create_agent_worktree(cwd: &Path, agent_slug: &str) -> Result<PathBuf, String> {
    let repo_root = resolve_repo_root(cwd)?;
    let slug = sanitize_agent_slug(agent_slug);
    let stamp = next_short_stamp();
    let (wt_path, branch) = build_worktree_paths(&repo_root, &slug, &stamp)?;
    run_worktree_add(&repo_root, &wt_path, &branch)?;
    Ok(wt_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("akhterm_{tag}_{}", next_short_stamp()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git_ok(cwd: &Path, args: &[&str]) {
        assert!(run_git_in(cwd, args).unwrap().status.success());
    }

    fn init_temp_repo(repo: &Path) {
        fs::create_dir_all(repo).unwrap();
        git_ok(repo, &["init", "-b", "main"]);
        fs::write(repo.join("README.md"), "init\n").unwrap();
        git_ok(repo, &["add", "."]);
        git_ok(
            repo,
            &["-c", "commit.gpgsign=false", "-c", "user.name=T", "-c", "user.email=t@e", "commit", "-m", "i"],
        );
    }

    #[test]
    fn parses_branch_and_detached_head_contents() {
        assert_eq!(parse_head_ref("ref: refs/heads/main\n").as_deref(), Some("main"));
        assert_eq!(parse_head_ref("ref: refs/heads/wt/claude-1\n").as_deref(), Some("wt/claude-1"));
        assert_eq!(parse_head_ref("2D4C7FE89abcdef0123456789\n").as_deref(), Some("2d4c7fe"));
        assert_eq!(parse_head_ref(""), None);
        assert_eq!(parse_head_ref("not-a-sha"), None);
    }

    #[test]
    fn stops_before_home_parent_boundary() {
        let tmp = TempRoot::new("bound");
        let home_parent = tmp.0.join("users");
        let fake_home = home_parent.join("dev");
        fs::create_dir_all(home_parent.join(".git")).unwrap();
        fs::write(home_parent.join(".git/HEAD"), "ref: refs/heads/stray\n").unwrap();
        let plain = fake_home.join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(detect_git_head_bounded(&plain, &fake_home), None);
    }

    #[test]
    fn detects_standard_branch_and_detached_head() {
        let tmp = TempRoot::new("std");
        let repo = tmp.0.join("proj");
        let nested = repo.join("crates/core");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feat/ui\n").unwrap();
        let branch = detect_git_head_bounded(&nested, &tmp.0).unwrap();
        assert_eq!((branch.head.as_str(), branch.is_linked_worktree), ("feat/ui", false));
        assert_eq!(branch.badge(), "⎇ feat/ui");

        fs::write(repo.join(".git/HEAD"), "9f8e7d6c5b4a32109f8e7d6c5b4a32109f8e7d6c\n").unwrap();
        let detached = detect_git_head_bounded(&nested, &tmp.0).unwrap();
        assert_eq!((detached.head.as_str(), detached.is_linked_worktree), ("9f8e7d6", false));
    }

    #[test]
    fn detects_linked_worktree_git_file_in_cyrillic_directory() {
        let tmp = TempRoot::new("кир");
        let gitdir = tmp.0.join("Хранилище/.git/worktrees/wt-1");
        let wt_sub = tmp.0.join("Копии/Терминал ЦУ-wt-agy-01/модули");
        fs::create_dir_all(&gitdir).unwrap();
        fs::create_dir_all(&wt_sub).unwrap();
        fs::write(gitdir.join("HEAD"), "ref: refs/heads/wt/agy-01\n").unwrap();
        fs::write(wt_sub.parent().unwrap().join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        let info = detect_git_head(&wt_sub).unwrap();
        assert_eq!(info.badge(), "⎇ wt/agy-01 [wt]");
    }

    #[test]
    fn reports_error_when_creating_worktree_outside_git_repo() {
        let tmp = TempRoot::new("nonrepo");
        let outside = tmp.0.join("Вне репозитория");
        fs::create_dir_all(&outside).unwrap();
        let err = create_agent_worktree(&outside, "claude").unwrap_err();
        assert!(err.contains("Git"));
    }

    #[test]
    fn creates_sibling_agent_worktree_in_cyrillic_repo() {
        let tmp = TempRoot::new("wt");
        let repo = tmp.0.join("Стенд ЦУ/МойПроект");
        init_temp_repo(&repo);
        let sub = repo.join("подпапка");
        fs::create_dir_all(&sub).unwrap();
        let wt = create_agent_worktree(&sub, "Claude").unwrap();
        assert_eq!(wt.parent(), repo.parent());
        assert!(wt.file_name().unwrap().to_string_lossy().starts_with("МойПроект-wt-claude-"));
        let head = detect_git_head(&wt).unwrap();
        assert!(head.is_linked_worktree && head.head.starts_with("wt/claude-"));
    }

    #[test]
    fn resolves_relative_gitdir_and_agent_slugs() {
        let tmp = TempRoot::new("rel");
        let gd = tmp.0.join("store/wt_meta");
        let wt = tmp.0.join("work/copy");
        fs::create_dir_all(&gd).unwrap();
        fs::create_dir_all(&wt).unwrap();
        fs::write(gd.join("HEAD"), "ref: refs/heads/wt/codex-7\n").unwrap();
        fs::write(wt.join(".git"), "gitdir: ../../store/wt_meta\n").unwrap();
        assert_eq!(detect_git_head(&wt).unwrap().badge(), "⎇ wt/codex-7 [wt]");
        assert_eq!(agent_slug_from_cmd("claude\n"), "claude");
        assert_eq!(agent_slug_from_cmd("/usr/bin/agy --fast\n"), "agy");
    }
}
