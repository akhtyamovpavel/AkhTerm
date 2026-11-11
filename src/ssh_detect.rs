//! Accurate interactive OpenSSH destination detection and process-tree filtering.

use std::path::Path;

use crate::tabs::is_ssh_host;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: i32,
    pub ppid: i32,
    pub pgrp: i32,
    pub argv: Vec<String>,
}

pub fn exe_basename(arg0: &str) -> String {
    let trimmed = arg0.trim_start_matches('-');
    let target = if trimmed.is_empty() { arg0 } else { trimmed };
    Path::new(target)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_non_interactive_flag(ch: char) -> bool {
    matches!(ch, '-' | 'N' | 'f' | 'W' | 'O' | 'Q' | 'G' | 'V' | 'n' | 's')
}

fn takes_option_value(ch: char) -> bool {
    matches!(
        ch,
        'B' | 'b' | 'c' | 'D' | 'E' | 'e' | 'F' | 'I' | 'i' | 'J'
            | 'L' | 'l' | 'm' | 'o' | 'P' | 'p' | 'R' | 'S' | 'w'
    )
}

fn parse_option_tail<'a>(
    ch: char,
    rest: &'a str,
    next_tok: Option<&'a str>,
) -> Option<(usize, Option<&'a str>)> {
    let (used_next, val) = if rest.is_empty() {
        (1, next_tok?)
    } else {
        (0, rest)
    };
    let user = (ch == 'l').then_some(val);
    Some((used_next, user))
}

fn parse_flag_cluster<'a>(
    cluster: &'a str,
    next_tok: Option<&'a str>,
) -> Option<(usize, Option<&'a str>)> {
    for (byte_idx, ch) in cluster.char_indices() {
        if is_non_interactive_flag(ch) {
            return None;
        }
        if takes_option_value(ch) {
            let rest = &cluster[byte_idx + ch.len_utf8()..];
            return parse_option_tail(ch, rest, next_tok);
        }
    }
    Some((0, None))
}

fn strip_uri_port(host_port: &str) -> &str {
    let Some((head, port)) = host_port.rsplit_once(':') else {
        return host_port;
    };
    let valid_port = !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
    let unbracketed_v6 = head.contains(':') && !head.ends_with(']');
    if !head.is_empty() && valid_port && !unbracketed_v6 {
        head
    } else {
        host_port
    }
}

pub fn format_ssh_destination(raw_dest: &str, login_user: Option<&str>) -> Option<String> {
    let dest = match raw_dest.strip_prefix("ssh://") {
        Some(rest) => strip_uri_port(rest.split('/').next().unwrap_or(rest)),
        None => raw_dest,
    };
    if !is_ssh_host(dest) {
        return None;
    }
    match login_user.filter(|u| is_ssh_host(u)) {
        Some(user) if !dest.contains('@') => Some(format!("{user}@{dest}")),
        _ => Some(dest.to_owned()),
    }
}

pub fn parse_ssh_argv(argv: &[String]) -> Option<String> {
    if exe_basename(argv.first()?) != "ssh" {
        return None;
    }
    let mut login_user: Option<&str> = None;
    let mut raw_dest: Option<&str> = None;
    let mut idx = 1usize;
    while idx < argv.len() {
        let tok = argv[idx].as_str();
        if tok == "--" {
            if raw_dest.is_none() {
                raw_dest = Some(argv.get(idx + 1)?.as_str());
            }
            break;
        }
        if let Some(cluster) = tok.strip_prefix('-').filter(|c| !c.is_empty()) {
            let next = argv.get(idx + 1).map(String::as_str);
            let (extra, maybe_user) = parse_flag_cluster(cluster, next)?;
            login_user = login_user.or(maybe_user);
            idx += 1 + extra;
            continue;
        }
        if raw_dest.is_none() {
            raw_dest = Some(tok);
            idx += 1;
        } else {
            break;
        }
    }
    format_ssh_destination(raw_dest?, login_user)
}

fn is_excluded_tool_name(name: &str) -> bool {
    matches!(name, "sshuttle" | "git" | "scp" | "sftp" | "rsync") || name.starts_with("git-")
}

fn is_wrapper_for_excluded(exe: &str, args: &[String]) -> bool {
    let is_wrapper = exe.starts_with("python") || matches!(exe, "sudo" | "env");
    is_wrapper && args.iter().any(|arg| is_excluded_tool_name(&exe_basename(arg)))
}

pub fn is_excluded_ssh_parent(argv: &[String]) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    let exe = exe_basename(first);
    exe == "ssh" || is_excluded_tool_name(&exe) || is_wrapper_for_excluded(&exe, &argv[1..])
}

fn has_excluded_ancestor(entry: &ProcEntry, root_pid: i32, procs: &[ProcEntry]) -> bool {
    if entry.pid == root_pid {
        return false;
    }
    let mut cur_ppid = entry.ppid;
    for _ in 0..32 {
        if cur_ppid <= 0 {
            break;
        }
        let Some(parent) = procs.iter().find(|p| p.pid == cur_ppid) else {
            break;
        };
        if is_excluded_ssh_parent(&parent.argv) {
            return true;
        }
        if cur_ppid == root_pid {
            break;
        }
        cur_ppid = parent.ppid;
    }
    false
}

fn is_foreground_candidate(entry: &ProcEntry, tpgid: i32) -> bool {
    entry.pid > 0 && (tpgid <= 0 || entry.pgrp == tpgid)
}

pub fn detect_active_ssh(root_pid: i32, tpgid: i32, procs: &[ProcEntry]) -> Option<String> {
    for entry in procs {
        if !is_foreground_candidate(entry, tpgid) {
            continue;
        }
        let Some(dest) = parse_ssh_argv(&entry.argv) else {
            continue;
        };
        if !has_excluded_ancestor(entry, root_pid, procs) {
            return Some(dest);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| (*s).to_owned()).collect()
    }

    fn proc_node(pid: i32, ppid: i32, pgrp: i32, words: &[&str]) -> ProcEntry {
        ProcEntry { pid, ppid, pgrp, argv: vec_args(words) }
    }

    #[test]
    fn parses_complex_interactive_ssh_flags_and_attached_forms() {
        let cmd = vec_args(&[
            "/usr/bin/ssh", "-p", "2222", "-i", "~/.ssh/id_ed25519", "-J", "jump", "ubuntu@host",
        ]);
        assert_eq!(parse_ssh_argv(&cmd).as_deref(), Some("ubuntu@host"));

        let attached = vec_args(&[
            "ssh", "-vp2222", "-i/tmp/key", "-oStrictHostKeyChecking=no", "-L", "8080:127.0.0.1:80",
            "deploy@prod.example.com",
        ]);
        assert_eq!(parse_ssh_argv(&attached).as_deref(), Some("deploy@prod.example.com"));

        let sep = vec_args(&["ssh", "-t", "-F", "/etc/ssh/ssh_config", "--", "gpu-box", "tmux", "a"]);
        assert_eq!(parse_ssh_argv(&sep).as_deref(), Some("gpu-box"));
    }

    #[test]
    fn reconstructs_login_user_and_stops_at_destination() {
        let with_l = vec_args(&["ssh", "-p", "2200", "-l", "ubuntu", "10.0.0.42"]);
        assert_eq!(parse_ssh_argv(&with_l).as_deref(), Some("ubuntu@10.0.0.42"));

        let post_dest_l = vec_args(&["ssh", "10.0.0.42", "-l", "ubuntu", "-p", "2222"]);
        assert_eq!(parse_ssh_argv(&post_dest_l).as_deref(), Some("ubuntu@10.0.0.42"));

        let first_l_wins = vec_args(&["ssh", "-l", "alice", "10.0.0.42", "-l", "bob"]);
        assert_eq!(parse_ssh_argv(&first_l_wins).as_deref(), Some("alice@10.0.0.42"));

        let attached_l = vec_args(&["ssh", "-lroot", "db-node"]);
        assert_eq!(parse_ssh_argv(&attached_l).as_deref(), Some("root@db-node"));

        let explicit_at = vec_args(&["ssh", "-l", "ignored", "alice@server"]);
        assert_eq!(parse_ssh_argv(&explicit_at).as_deref(), Some("alice@server"));

        let remote_cmd = vec_args(&["ssh", "box.local", "ls", "-l", "otheruser", "-N"]);
        assert_eq!(parse_ssh_argv(&remote_cmd).as_deref(), Some("box.local"));

        let after_ddash = vec_args(&["ssh", "box.local", "--", "-N", "-O", "exit"]);
        assert_eq!(parse_ssh_argv(&after_ddash).as_deref(), Some("box.local"));

        let uri = vec_args(&["ssh", "-l", "ops", "ssh://prod.internal:2222/"]);
        assert_eq!(parse_ssh_argv(&uri).as_deref(), Some("ops@prod.internal"));
    }

    #[test]
    fn rejects_tunnel_mux_background_and_incomplete_invocations() {
        for raw in [
            &["ssh", "-fN", "-L", "5432:localhost:5432", "db"][..],
            &["ssh", "ubuntu@host", "-N", "-L", "5432:localhost:5432"][..],
            &["ssh", "host", "-fN", "-D", "1080"][..],
            &["ssh", "ubuntu@host", "-O", "exit"][..],
            &["ssh", "bastion", "-W", "127.0.0.1:22"][..],
            &["ssh", "-N", "ubuntu@host"][..],
            &["ssh", "-f", "ubuntu@host", "sleep", "60"][..],
            &["ssh", "-W", "127.0.0.1:22", "bastion"][..],
            &["ssh", "-O", "check", "ubuntu@host"][..],
            &["ssh", "-Oexit", "ubuntu@host"][..],
            &["ssh", "-Q", "cipher"][..],
            &["ssh", "host", "--unknown"][..],
            &["ssh", "-p"][..],
            &["ssh"][..],
        ] {
            assert_eq!(parse_ssh_argv(&vec_args(raw)), None, "should reject {raw:?}");
        }
    }

    #[test]
    fn excludes_sshuttle_git_scp_sftp_rsync_and_background_jobs() {
        let sshuttle_tree = [
            proc_node(200, 100, 200, &["/usr/bin/python3", "/usr/bin/sshuttle", "-r", "vpn@gw", "0/0"]),
            proc_node(201, 200, 200, &["ssh", "vpn@gw", "--", "python3", "-c", "import sys"]),
        ];
        assert_eq!(detect_active_ssh(100, 200, &sshuttle_tree), None);

        for tool in ["sshuttle", "git", "scp", "sftp", "rsync", "/usr/lib/git-core/git-remote-ssh"] {
            let tree = [
                proc_node(300, 100, 300, &[tool, "arg"]),
                proc_node(301, 300, 300, &["ssh", "git@github.com"]),
            ];
            assert_eq!(detect_active_ssh(100, 300, &tree), None, "tool {tool} must be excluded");
        }

        let bg_ssh = [proc_node(400, 100, 400, &["ssh", "ubuntu@bg-host"])];
        assert_eq!(detect_active_ssh(100, 100, &bg_ssh), None);
        assert_eq!(detect_active_ssh(100, 400, &bg_ssh).as_deref(), Some("ubuntu@bg-host"));
    }

    #[test]
    fn prefers_outer_ssh_over_proxyjump_helper_child() {
        let proxy_tree = [
            proc_node(501, 500, 500, &["ssh", "bastion", "nc", "target", "22"]),
            proc_node(500, 100, 500, &["ssh", "-o", "ProxyCommand=ssh bastion nc %h %p", "app@target"]),
        ];
        assert_eq!(detect_active_ssh(100, 500, &proxy_tree).as_deref(), Some("app@target"));
    }

    #[test]
    fn supports_exec_ssh_root_pid_and_rejects_vanished_zero_pgrp() {
        let exec_root = [proc_node(100, 1, 100, &["ssh", "ubuntu@exec-host"])];
        assert_eq!(detect_active_ssh(100, 100, &exec_root).as_deref(), Some("ubuntu@exec-host"));

        let vanished_bg = [proc_node(401, 100, 0, &["ssh", "ubuntu@bg-host"])];
        assert_eq!(detect_active_ssh(100, 100, &vanished_bg), None);
        assert_eq!(detect_active_ssh(100, 0, &vanished_bg).as_deref(), Some("ubuntu@bg-host"));

        let exec_sshuttle = [
            proc_node(100, 1, 100, &["python3", "/usr/bin/sshuttle", "-r", "vpn@gw", "0/0"]),
            proc_node(101, 100, 100, &["ssh", "vpn@gw"]),
        ];
        assert_eq!(detect_active_ssh(100, 100, &exec_sshuttle), None);
    }

    #[test]
    fn ssh_detect_module_maintains_grade_b_mi() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("ssh_detect.rs"));
        assert!(mi >= 58.0, "ssh_detect.rs MI = {mi:.2}");
    }
}
