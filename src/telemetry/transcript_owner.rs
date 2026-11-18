use crate::tmux_local::collect_fd_tree_pids;
use std::fs;
use std::path::{Path, PathBuf};

pub fn claude_fd(root_pid: i32, home: &Path) -> Option<PathBuf> {
    let projects = home.join(".claude/projects");
    for pid in collect_fd_tree_pids(root_pid) {
        let Ok(files) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for target in files
            .flatten()
            .filter_map(|fd| fs::read_link(fd.path()).ok())
        {
            let Ok(relative) = target.strip_prefix(&projects) else {
                continue;
            };
            if relative.components().count() == 2
                && target.extension().is_some_and(|e| e == "jsonl")
                && target.is_file()
            {
                return Some(target);
            }
        }
    }
    None
}

pub fn shared_workspace(root_pid: i32, cwd: &Path, agent: &str) -> bool {
    let own = collect_fd_tree_pids(root_pid);
    let Ok(procs) = fs::read_dir("/proc") else {
        return false;
    };
    procs.flatten().any(|entry| {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        else {
            return false;
        };
        if own.contains(&pid)
            || fs::read_link(entry.path().join("cwd")).ok().as_deref() != Some(cwd)
        {
            return false;
        }
        let Ok(raw) = fs::read(entry.path().join("cmdline")) else {
            return false;
        };
        let argv: Vec<_> = raw
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        crate::agent_detect::detect_argv_agent(&argv)
            .is_some_and(|name| name.eq_ignore_ascii_case(agent))
    })
}
