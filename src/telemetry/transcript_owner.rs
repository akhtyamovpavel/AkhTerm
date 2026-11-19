use crate::tmux_local::collect_fd_tree_pids;
use std::fs;
use std::path::{Path, PathBuf};

fn is_claude_jsonl(target: &Path, projects: &Path) -> bool {
    let Ok(relative) = target.strip_prefix(projects) else {
        return false;
    };
    relative.components().count() == 2
        && target.extension().is_some_and(|e| e == "jsonl")
        && target.is_file()
}

fn find_claude_jsonl_in_pid(pid: i32, projects: &Path) -> Option<PathBuf> {
    let files = fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    files
        .flatten()
        .filter_map(|fd| fs::read_link(fd.path()).ok())
        .find(|target| is_claude_jsonl(target, projects))
}

pub fn claude_fd(root_pid: i32, home: &Path) -> Option<PathBuf> {
    let projects = home.join(".claude/projects");
    collect_fd_tree_pids(root_pid)
        .into_iter()
        .find_map(|pid| find_claude_jsonl_in_pid(pid, &projects))
}

fn parse_proc_pid(entry: &fs::DirEntry) -> Option<i32> {
    entry.file_name().to_str()?.parse::<i32>().ok()
}

fn read_proc_argv(proc_dir: &Path) -> Option<Vec<String>> {
    let raw = fs::read(proc_dir.join("cmdline")).ok()?;
    let argv = raw
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    Some(argv)
}

fn pid_matches_workspace_agent(
    entry: &fs::DirEntry,
    own: &[i32],
    cwd: &Path,
    agent: &str,
) -> bool {
    let Some(pid) = parse_proc_pid(entry) else {
        return false;
    };
    if own.contains(&pid) {
        return false;
    }
    let path = entry.path();
    if fs::read_link(path.join("cwd")).ok().as_deref() != Some(cwd) {
        return false;
    }
    let Some(argv) = read_proc_argv(&path) else {
        return false;
    };
    crate::agent_detect::detect_argv_agent(&argv)
        .is_some_and(|name| name.eq_ignore_ascii_case(agent))
}

pub fn shared_workspace(root_pid: i32, cwd: &Path, agent: &str) -> bool {
    let own = collect_fd_tree_pids(root_pid);
    let Ok(procs) = fs::read_dir("/proc") else {
        return false;
    };
    procs
        .flatten()
        .any(|entry| pid_matches_workspace_agent(&entry, &own, cwd, agent))
}
