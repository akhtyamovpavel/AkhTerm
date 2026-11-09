//! Process tree radar (/proc/<pid>), ~/.ssh/config parser, multi-server tmux tracker, and link launcher.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use crate::paths::{home_dir, shorten_home, state_dir, write_state_file};
use crate::tabs::{is_ssh_host, Tab};

#[derive(Clone, Debug, Default)]
pub struct ProcInfo {
    pub ssh_host: String,
    pub has_tmux: String,
    pub agent: String,
    pub fg_cmd: String,
    pub cwd: String,
}

#[derive(Clone, Debug)]
pub struct SshHostEntry {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default)]
pub struct RemoteTmuxItem {
    pub host: String,
    pub session_name: String,
    pub display_line: String,
    pub is_active: bool,
}

#[derive(Clone, Debug, Default)]
pub struct BoxTelemetry {
    pub gpu_lines: Vec<String>,
    pub tmux_items: Vec<RemoteTmuxItem>,
    pub active_by_host: Vec<(String, String, String, String, String)>, // (host, sess, win, cmd, path)
}

pub fn load_ssh_hosts(watch: &[String], startup: &[Tab]) -> Vec<SshHostEntry> {
    let home = home_dir();
    let ssh_cfg = format!("{home}/.ssh/config");
    let conf_path = state_dir().join("servers.conf");

    let saved_enabled: Option<HashSet<String>> = fs::read_to_string(&conf_path).ok().map(|txt| {
        txt.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect()
    });

    let watched = saved_enabled.unwrap_or_else(|| watch.iter().cloned().collect());
    let mut entries: Vec<SshHostEntry> = Vec::new();
    let mut cur_aliases: Vec<String> = Vec::new();
    let mut cur_hostname = String::new();
    let mut cur_user = String::new();

    let flush_entry = |aliases: &mut Vec<String>,
                       hname: &mut String,
                       usr: &mut String,
                       out: &mut Vec<SshHostEntry>| {
        for alias in aliases.drain(..) {
            if !is_ssh_host(&alias)
                || alias.starts_with('!')
                || alias.contains('*')
                || alias.contains('?')
                || alias == "github.com"
                || alias == "gitlab.com"
                || alias == "bitbucket.org"
            {
                continue;
            }
            if out.iter().any(|e| e.alias == alias) {
                continue;
            }
            let enabled = watched.contains(&alias);
            out.push(SshHostEntry {
                alias,
                hostname: hname.clone(),
                user: usr.clone(),
                enabled,
            });
        }
        hname.clear();
        usr.clear();
    };

    if let Ok(content) = fs::read_to_string(&ssh_cfg) {
        for raw_line in content.lines() {
            let line = raw_line.split('#').next().unwrap_or_default();
            let mut parts = line.split_whitespace();
            let Some(key) = parts.next() else {
                continue;
            };
            if key.eq_ignore_ascii_case("Host") {
                flush_entry(&mut cur_aliases, &mut cur_hostname, &mut cur_user, &mut entries);
                for pat in parts {
                    cur_aliases.push(pat.to_string());
                }
            } else if key.eq_ignore_ascii_case("HostName") {
                if let Some(val) = parts.next() {
                    cur_hostname = val.to_string();
                }
            } else if key.eq_ignore_ascii_case("User") {
                if let Some(val) = parts.next() {
                    cur_user = val.to_string();
                }
            }
        }
        flush_entry(&mut cur_aliases, &mut cur_hostname, &mut cur_user, &mut entries);
    }

    for alias in watch.iter().chain(startup.iter().filter_map(|tab| match tab {
        Tab::Remote(remote) => Some(&remote.host),
        Tab::Local { .. } => None,
    })) {
        if entries.iter().any(|entry| entry.alias == *alias) {
            continue;
        }
        entries.push(SshHostEntry {
            alias: alias.clone(),
            hostname: String::new(),
            user: String::new(),
            enabled: watched.contains(alias),
        });
    }

    entries
}

pub fn save_ssh_hosts(hosts: &[SshHostEntry]) {
    let lines: Vec<&str> = hosts
        .iter()
        .filter(|h| h.enabled)
        .map(|h| h.alias.as_str())
        .collect();
    if let Err(error) = write_state_file(&state_dir().join("servers.conf"), &lines.join("\n")) {
        eprintln!("AkhTerm: cannot save SSH settings: {error}");
    }
}

pub fn inspect_proc_tree(root_pid: i32) -> ProcInfo {
    let mut info = ProcInfo::default();
    if root_pid <= 0 {
        return info;
    }
    if let Ok(cwd_path) = fs::read_link(format!("/proc/{root_pid}/cwd")) {
        info.cwd = shorten_home(&cwd_path.to_string_lossy());
    }

    let mut visited = HashSet::new();
    let mut queue = vec![root_pid];
    let mut cmds: Vec<Vec<String>> = Vec::new();

    while let Some(pid) = queue.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) {
            let parts: Vec<String> = raw
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect();
            if !parts.is_empty() && pid != root_pid {
                if let Ok(cwd_p) = fs::read_link(format!("/proc/{pid}/cwd")) {
                    info.cwd = shorten_home(&cwd_p.to_string_lossy());
                }
                cmds.push(parts);
            }
        }
        let children_file = format!("/proc/{pid}/task/{pid}/children");
        if let Ok(ch_str) = fs::read_to_string(children_file) {
            for tok in ch_str.split_whitespace() {
                if let Ok(cpid) = tok.parse::<i32>() {
                    queue.push(cpid);
                }
            }
        }
    }

    for parts in &cmds {
        let joined = parts.join(" ");
        let low = joined.to_lowercase();
        let exe = Path::new(&parts[0])
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        if exe == "ssh" {
            for tok in &parts[1..] {
                if !tok.starts_with('-') && !tok.contains(' ') {
                    info.ssh_host = tok.clone();
                    break;
                }
            }
        }
        if exe == "tmux" || low.contains("tmux ") {
            info.has_tmux = "tmux".to_string();
        }
        if matches!(exe.as_str(), "deepseek" | "deepseek-cli") {
            info.agent = "DeepSeek".to_string();
        } else if exe == "agy" || low.contains("/agy") || low.contains("antigravity") {
            info.agent = "AGY".to_string();
        } else if exe == "codex" || low.contains("codex") {
            info.agent = "Codex".to_string();
        } else if exe == "dsh" || low.contains("/dsh") {
            info.agent = "DSH".to_string();
        } else if exe == "kimi" || low.contains("kimi") {
            info.agent = "Kimi".to_string();
        } else if exe == "claude" || low.contains("claude") {
            info.agent = "Claude".to_string();
        }
        if !["zsh", "bash", "sh"].contains(&exe.as_str()) {
            info.fg_cmd = exe;
        }
    }

    info
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn switch_remote_tmux_session(host: &str, session_name: &str) {
    let h = host.to_string();
    let target = shell_quote(session_name);
    let command = format!("tmux switch-client -t {target} 2>/dev/null");
    thread::spawn(move || {
        let _ = Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=2",
                "-o",
                "ClearAllForwardings=yes",
                "--",
                &h,
                &command,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

pub fn query_ssh_servers(hosts: &[String]) -> BoxTelemetry {
    let mut out = BoxTelemetry::default();
    if hosts.is_empty() {
        out.gpu_lines.push("Опрос SSH выключен".to_owned());
        return out;
    }
    let remote_cmd = concat!(
        "tmux set -g set-titles on 2>/dev/null; ",
        "tmux set -g set-titles-string 'AKHTERM_TMUX:#{session_name}|#{window_name}|#{pane_current_command}|#{pane_current_path}' 2>/dev/null; ",
        "tmux list-sessions -F '#{session_name} (#{session_windows} окн.)' 2>/dev/null; ",
        "echo '---CLIENTS---'; ",
        "tmux list-clients -F '#{session_name}|#{window_name}|#{pane_current_command}|#{pane_current_path}' 2>/dev/null | tail -n 1; ",
        "echo '---GPU---'; ",
        "nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total --format=csv,noheader 2>/dev/null | head -n 1"
    );

    for host in hosts {
        let res = Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=2",
                "-o",
                "ClearAllForwardings=yes",
                "--",
                host,
                remote_cmd,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();

        let Ok(output) = res else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let p1: Vec<&str> = stdout.split("---CLIENTS---").collect();
        let mut active_sess = String::new();

        if p1.len() > 1 {
            let p2: Vec<&str> = p1[1].split("---GPU---").collect();
            let client_line = p2[0].trim();
            let cparts: Vec<&str> = client_line.split('|').collect();
            if cparts.len() >= 4 {
                active_sess = cparts[0].to_string();
                out.active_by_host.push((
                    host.clone(),
                    cparts[0].to_string(),
                    cparts[1].to_string(),
                    cparts[2].to_string(),
                    cparts[3].to_string(),
                ));
            }
            if p2.len() > 1 && !p2[1].trim().is_empty() {
                out.gpu_lines.push(format!("[{host}] {}", p2[1].trim()));
            }
        }

        for line in p1[0].lines().map(str::trim).filter(|s| !s.is_empty()) {
            let sname = line.rsplit_once(" (").map_or(line, |(name, _)| name).to_string();
            let is_active = !active_sess.is_empty() && sname == active_sess;
            out.tmux_items.push(RemoteTmuxItem {
                host: host.clone(),
                session_name: sname,
                display_line: format!("[{host}] {line}"),
                is_active,
            });
        }
    }

    if out.gpu_lines.is_empty() {
        out.gpu_lines.push("GPU: нет активного ответа от SSH".to_string());
    }
    out
}

pub fn open_external_link(raw_url: &str) {
    let trimmed = raw_url
        .trim()
        .trim_end_matches(|c: char| ".,;:!?)]}\"'".contains(c));
    if trimmed.is_empty() {
        return;
    }

    let home = home_dir();
    let mut target = if trimmed.starts_with("localhost:")
        || trimmed.starts_with("127.0.0.1:")
        || trimmed.starts_with("0.0.0.0:")
    {
        format!("http://{trimmed}")
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("file://") {
        rest.to_string()
    } else {
        trimmed.to_string()
    };

    if target.starts_with('/') && !Path::new(&target).exists() {
        if let Some((base, _line)) = target.rsplit_once(':') {
            if Path::new(base).exists() {
                target = base.to_string();
            } else if let Some((base2, _col)) = base.rsplit_once(':') {
                if Path::new(base2).exists() {
                    target = base2.to_string();
                }
            }
        }
    }

    let _ = Command::new("xdg-open")
        .arg(&target)
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn upload_bytes_to_ssh(ssh_host: &str, remote_path: &str, bytes: &[u8]) -> bool {
    let Ok(mut child) = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=4",
            "-o",
            "ClearAllForwardings=yes",
            "--",
            ssh_host,
            &format!("cat > {remote_path}"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        if stdin.write_all(bytes).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

pub fn try_paste_clipboard_image(ssh_host: &str, force_image: bool) -> Option<(String, String)> {
    let targets_out = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let targets = String::from_utf8_lossy(&targets_out.stdout);
    let has_png = targets.lines().any(|l| l.trim() == "image/png");
    let has_text = targets
        .lines()
        .any(|l| matches!(l.trim(), "UTF8_STRING" | "STRING" | "text/plain"));

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 1. Direct screenshot / copied image in clipboard (image/png)
    if has_png && (force_image || !has_text) {
        let img_out = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "image/png", "-o"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let bytes = img_out.stdout;
        if bytes.is_empty() {
            return None;
        }
        let target_path = format!("/tmp/akhterm_paste_{ts}.png");
        let _ = fs::write(&target_path, &bytes);
        let kb = (bytes.len() / 1024).max(1);

        if !ssh_host.is_empty() {
            if upload_bytes_to_ssh(ssh_host, &target_path, &bytes) {
                return Some((
                    format!("{target_path} "),
                    format!("🖼 Скриншот ({kb} КБ) загружен по SSH на {ssh_host}:{target_path}"),
                ));
            }
            return None;
        }
        return Some((
            format!("{target_path} "),
            format!("🖼 Скриншот ({kb} КБ) сохранён в {target_path}"),
        ));
    }

    // 2. Copied local image file path or URI pasted into a remote SSH session
    if !ssh_host.is_empty() && has_text {
        let txt_out = Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let raw_txt = String::from_utf8_lossy(&txt_out.stdout);
        let first_line = raw_txt.lines().next()?.trim();
        let clean_path = first_line.strip_prefix("file://").unwrap_or(first_line);
        let p = Path::new(clean_path);
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())?;
        if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") && p.is_file() {
            let bytes = fs::read(p).ok()?;
            let kb = (bytes.len() / 1024).max(1);
            let target_path = format!("/tmp/akhterm_paste_{ts}.{ext}");
            if upload_bytes_to_ssh(ssh_host, &target_path, &bytes) {
                return Some((
                    format!("{target_path} "),
                    format!("🖼 Файл ({kb} КБ) загружен по SSH на {ssh_host}:{target_path}"),
                ));
            }
        }
    }

    None
}
