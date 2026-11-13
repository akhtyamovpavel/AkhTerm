//! Pure project-folder resolution and grouping for local sidebar sessions.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::git_wt::resolve_primary_worktree_root;
use crate::mi_watch::resolve_project_root;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderGroup {
    pub key: String,
    pub label: String,
    pub tooltip: String,
    pub item_indices: Vec<usize>,
    pub visible_indices: Vec<usize>,
}

fn expand_home_with(path: &str, home: &str) -> String {
    if path == "~" {
        return home.to_owned();
    }
    match path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", home.trim_end_matches('/')),
        None => path.to_owned(),
    }
}

fn shorten_home_with(path: &str, home: &str) -> String {
    let norm = home.trim_end_matches('/');
    if norm.is_empty() || path != norm && !path.starts_with(&format!("{norm}/")) {
        return path.to_owned();
    }
    match path.strip_prefix(norm) {
        Some("") => "~".to_owned(),
        Some(rest) => format!("~{rest}"),
        None => path.to_owned(),
    }
}

fn clean_path_buf(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolve_git_path(base: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw.trim());
    let joined = if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
    clean_path_buf(&joined)
}

fn read_gitdir_target(dot_git: &Path, repo_dir: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(dot_git).ok()?;
    let target = raw.lines().find_map(|l| l.trim().strip_prefix("gitdir:"))?;
    let trimmed = target.trim();
    (!trimmed.is_empty()).then(|| resolve_git_path(repo_dir, trimmed))
}

fn root_from_commondir(gitdir: &Path) -> Option<PathBuf> {
    let rel = fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = resolve_git_path(gitdir, rel.trim());
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_path_buf)).flatten()
}

fn root_from_worktrees_parent(gitdir: &Path) -> Option<PathBuf> {
    let wt_dir = gitdir.parent()?;
    let dot_git = (wt_dir.file_name()? == "worktrees").then(|| wt_dir.parent())??;
    (dot_git.file_name()? == ".git").then(|| dot_git.parent().map(Path::to_path_buf)).flatten()
}

fn primary_root_from_gitdir(gitdir: &Path) -> Option<PathBuf> {
    root_from_commondir(gitdir).or_else(|| root_from_worktrees_parent(gitdir))
}

fn is_stop_dir(dir: &Path) -> bool {
    dir == Path::new("/") || dir == Path::new("/tmp") || dir == Path::new("/var/tmp")
}

fn resolve_git_file_root(dot_git: &Path, ancestor: &Path) -> PathBuf {
    read_gitdir_target(dot_git, ancestor)
        .and_then(|g| primary_root_from_gitdir(&g))
        .or_else(|| resolve_primary_worktree_root(ancestor))
        .unwrap_or_else(|| ancestor.to_path_buf())
}

pub fn resolve_primary_repo_root(dir: &Path) -> Option<PathBuf> {
    for ancestor in dir.ancestors() {
        if is_stop_dir(ancestor) {
            break;
        }
        let dot_git = ancestor.join(".git");
        if dot_git.is_dir() {
            return Some(ancestor.to_path_buf());
        }
        if dot_git.is_file() {
            return Some(resolve_git_file_root(&dot_git, ancestor));
        }
    }
    None
}

pub fn normalize_dir_key(raw: &str, home: &str) -> String {
    let expanded = expand_home_with(raw.trim(), home);
    let clean = clean_path_buf(Path::new(&expanded));
    let s = clean.to_string_lossy();
    let trimmed = s.trim_end_matches('/');
    if !trimmed.is_empty() {
        return trimmed.to_owned();
    }
    if s.starts_with('/') { "/".to_owned() } else { home.trim_end_matches('/').to_owned() }
}

pub fn folder_basename(canonical_dir: &str, home: &str) -> String {
    let norm = home.trim_end_matches('/');
    if !norm.is_empty() && canonical_dir == norm {
        return "~".to_owned();
    }
    if canonical_dir == "/" {
        return "/".to_owned();
    }
    Path::new(canonical_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| shorten_home_with(canonical_dir, home))
}

pub fn resolve_project_folder(dir: &str, home: &str) -> String {
    let base = if dir.trim().is_empty() {
        home.to_owned()
    } else {
        expand_home_with(dir.trim(), home)
    };
    if let Some(repo) = resolve_primary_repo_root(Path::new(&base)) {
        return normalize_dir_key(&repo.to_string_lossy(), home);
    }
    if let Some(root) = resolve_project_root(&base, home) {
        return normalize_dir_key(&root, home);
    }
    normalize_dir_key(&base, home)
}

fn get_or_insert_group(
    groups: &mut Vec<FolderGroup>,
    by_key: &mut HashMap<String, usize>,
    key: String,
    home: &str,
) -> usize {
    if let Some(&pos) = by_key.get(&key) {
        return pos;
    }
    let pos = groups.len();
    by_key.insert(key.clone(), pos);
    groups.push(FolderGroup {
        label: folder_basename(&key, home),
        tooltip: shorten_home_with(&key, home),
        key,
        item_indices: Vec::new(),
        visible_indices: Vec::new(),
    });
    pos
}

pub fn group_local_entries<'a, I>(entries: I, home: &str) -> Vec<FolderGroup>
where
    I: IntoIterator<Item = (usize, &'a str, bool)>,
{
    let mut groups = Vec::new();
    let mut by_key = HashMap::new();
    for (idx, dir, visible) in entries {
        let key = resolve_project_folder(dir, home);
        let pos = get_or_insert_group(&mut groups, &mut by_key, key, home);
        groups[pos].item_indices.push(idx);
        if visible {
            groups[pos].visible_indices.push(idx);
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let root = std::env::temp_dir().join(format!("akhterm_sf_{tag}_{nanos}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn setup_repo_and_worktree(home: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let repo = home.join("Productivity");
        let sub = repo.join("docs/canons");
        let wt = home.join(".Productivity-wt/feature-x/sub");
        let gitdir = repo.join(".git/worktrees/feature-x");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(&wt).unwrap();
        fs::create_dir_all(&gitdir).unwrap();
        fs::write(gitdir.join("commondir"), "../..\n").unwrap();
        fs::write(home.join(".Productivity-wt/feature-x/.git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        (repo, sub, wt)
    }

    #[test]
    fn groups_git_repo_subdirs_and_linked_worktrees_under_primary_repo() {
        let home = make_temp_dir("home");
        let (repo, sub, wt) = setup_repo_and_worktree(&home);
        let (home_s, repo_s) = (home.to_string_lossy(), repo.to_string_lossy());
        assert_eq!(resolve_project_folder(&sub.to_string_lossy(), &home_s), repo_s);
        assert_eq!(resolve_project_folder(&wt.to_string_lossy(), &home_s), repo_s);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn resolves_non_git_manifest_home_and_plain_directories() {
        let home = make_temp_dir("manifest");
        let (app, src, plain) = (home.join("crate-app"), home.join("crate-app/src"), home.join("notes"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&plain).unwrap();
        fs::write(app.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let home_s = home.to_string_lossy().into_owned();
        assert_eq!(resolve_project_folder(&src.to_string_lossy(), &home_s), app.to_string_lossy());
        assert_eq!(resolve_project_folder(&plain.to_string_lossy(), &home_s), plain.to_string_lossy());
        assert_eq!(resolve_project_folder("~", &home_s), home_s);
        assert_eq!(folder_basename(&home_s, &home_s), "~");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn preserves_first_appearance_order_and_tracks_visibility_counts() {
        let home = make_temp_dir("order");
        let (a, b) = (home.join("RepoA"), home.join("RepoB"));
        fs::create_dir_all(a.join(".git")).unwrap();
        fs::create_dir_all(b.join(".git")).unwrap();
        let (hs, as_, bs) = (home.to_string_lossy(), a.to_string_lossy(), b.to_string_lossy());
        let g = group_local_entries([(0, as_.as_ref(), true), (1, bs.as_ref(), false), (2, as_.as_ref(), false)], &hs);
        assert_eq!((g.len(), g[0].label.as_str(), g[0].tooltip.as_str()), (2, "RepoA", "~/RepoA"));
        assert_eq!((&g[0].item_indices, &g[0].visible_indices), (&vec![0, 2], &vec![0]));
        assert!(g[1].visible_indices.is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn sidebar_folders_module_achieves_target_mi() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("sidebar_folders.rs"));
        assert!(mi >= 60.0, "sidebar_folders.rs MI {mi:.2} < 60.0");
    }
}
