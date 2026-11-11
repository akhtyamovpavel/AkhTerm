//! Process tree radar (/proc/<pid>), ~/.ssh/config parser, multi-server tmux tracker, and link launcher.

use std::collections::HashSet;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr;
use std::thread;

use crate::ffi::*;
use crate::paths::{expand_home, home_dir, shorten_home, state_dir, write_state_file};
use crate::ssh_detect::{detect_active_ssh, exe_basename, ProcEntry};
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

fn parse_stat_groups(stat: &str) -> Option<(i32, i32)> {
    let (_, rest) = stat.rsplit_once(") ")?;
    let mut fields = rest.split_whitespace();
    let pgrp = fields.nth(2)?.parse().ok()?;
    let tpgid = fields.nth(2)?.parse().ok()?;
    Some((pgrp, tpgid))
}

fn read_proc_groups(pid: i32) -> Option<(i32, i32)> {
    if pid <= 0 {
        return None;
    }
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat_groups(&stat)
}

fn read_proc_cwd(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let path = fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    path.into_os_string().into_string().ok()
}

fn read_proc_cmdline(pid: i32) -> Option<Vec<String>> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let parts: Vec<String> = raw
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    (!parts.is_empty()).then_some(parts)
}

fn push_proc_children(pid: i32, queue: &mut Vec<(i32, i32)>) {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else { return };
    for task in tasks.flatten() {
        if let Ok(ch_str) = fs::read_to_string(task.path().join("children")) {
            queue.extend(ch_str.split_whitespace().filter_map(|t| t.parse::<i32>().ok().map(|c| (c, pid))));
        }
    }
}

fn read_proc_entry(pid: i32, ppid: i32) -> Option<ProcEntry> {
    if pid <= 0 {
        return None;
    }
    let argv = read_proc_cmdline(pid)?;
    let pgrp = read_proc_groups(pid).map_or(0, |(pg, _)| pg);
    Some(ProcEntry { pid, ppid, pgrp, argv })
}

fn collect_proc_tree(root_pid: i32) -> (HashSet<i32>, Vec<ProcEntry>) {
    let mut visited = HashSet::new();
    let mut queue = vec![(root_pid, 0)];
    let mut procs = Vec::new();
    while let Some((pid, ppid)) = queue.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Some(entry) = read_proc_entry(pid, ppid) {
            procs.push(entry);
        }
        push_proc_children(pid, &mut queue);
    }
    (visited, procs)
}

fn fallback_group_cwd(root_pid: i32, tpgid: i32, tree: Option<&HashSet<i32>>) -> Option<String> {
    let owned;
    let pids = match tree {
        Some(set) => set,
        None => {
            owned = collect_proc_tree(root_pid).0;
            &owned
        }
    };
    pids.iter()
        .copied()
        .filter(|&pid| pid != root_pid)
        .filter(|&pid| read_proc_groups(pid).is_some_and(|(pg, _)| pg == tpgid))
        .find_map(read_proc_cwd)
}

fn resolve_foreground_cwd(root_pid: i32, tree: Option<&HashSet<i32>>) -> Option<String> {
    if let Some((root_pgrp, tpgid)) = read_proc_groups(root_pid) {
        if tpgid > 0 && tpgid != root_pid && tpgid != root_pgrp {
            if let Some(cwd) = read_proc_cwd(tpgid) {
                return Some(cwd);
            }
            if let Some(cwd) = fallback_group_cwd(root_pid, tpgid, tree) {
                return Some(cwd);
            }
        }
    }
    read_proc_cwd(root_pid)
}

pub fn foreground_cwd(root_pid: i32) -> Option<String> {
    if root_pid <= 0 {
        return None;
    }
    resolve_foreground_cwd(root_pid, None)
}

fn detect_proc_agent(exe: &str, low: &str) -> Option<&'static str> {
    if matches!(exe, "deepseek" | "deepseek-cli") {
        Some("DeepSeek")
    } else if exe == "agy" || low.contains("/agy") || low.contains("antigravity") {
        Some("AGY")
    } else if exe == "codex" || low.contains("codex") {
        Some("Codex")
    } else if exe == "dsh" || low.contains("/dsh") {
        Some("DSH")
    } else if exe == "kimi" || low.contains("kimi") {
        Some("Kimi")
    } else if exe == "claude" || low.contains("claude") {
        Some("Claude")
    } else {
        None
    }
}

fn apply_proc_cmd(info: &mut ProcInfo, parts: &[String]) {
    let low = parts.join(" ").to_lowercase();
    let exe = exe_basename(&parts[0]);
    if exe != "ssh" && (exe == "tmux" || low.contains("tmux ")) {
        info.has_tmux = "tmux".to_string();
    }
    if let Some(agent) = detect_proc_agent(&exe, &low) {
        info.agent = agent.to_string();
    }
    if !matches!(exe.as_str(), "zsh" | "bash" | "sh") {
        info.fg_cmd = exe;
    }
}

pub fn inspect_proc_tree(root_pid: i32) -> ProcInfo {
    let mut info = ProcInfo::default();
    if root_pid <= 0 {
        return info;
    }
    let (visited, procs) = collect_proc_tree(root_pid);
    if let Some(cwd) = resolve_foreground_cwd(root_pid, Some(&visited)) {
        info.cwd = shorten_home(&cwd);
    }
    let tpgid = read_proc_groups(root_pid).map_or(0, |(_, tp)| tp);
    info.ssh_host = detect_active_ssh(root_pid, tpgid, &procs).unwrap_or_default();
    for entry in &procs {
        if entry.pid != root_pid {
            apply_proc_cmd(&mut info, &entry.argv);
        }
    }
    info
}

pub fn detect_local_agent(root_pid: i32) -> Option<String> {
    if root_pid <= 0 {
        return None;
    }
    let mut info = inspect_proc_tree(root_pid);
    if info.agent.is_empty() {
        if let Some(parts) = read_proc_cmdline(root_pid) {
            apply_proc_cmd(&mut info, &parts);
        }
    }
    (!info.agent.is_empty()).then_some(info.agent)
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

fn strip_link_punctuation(raw: &str) -> &str {
    raw.trim().trim_end_matches(|c: char| ".,;:!?)]}\"'".contains(c))
}

fn parse_line_num(part: &str) -> Option<u32> {
    (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        .then(|| part.parse::<u32>().ok())
        .flatten()
}

pub fn split_path_line_col(raw: &str) -> (&str, Option<u32>, Option<u32>) {
    let clean = strip_link_punctuation(raw);
    let Some((head, tail)) = clean.rsplit_once(':') else {
        return (clean, None, None);
    };
    let Some(n1) = parse_line_num(tail) else {
        return (clean, None, None);
    };
    if let Some((head2, line_s)) = head.rsplit_once(':') {
        if let Some(n2) = parse_line_num(line_s) {
            return (head2, Some(n2), Some(n1));
        }
    }
    (head, Some(n1), None)
}

fn clean_joined_path(base: &Path, rel: &str) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in base.join(rel).components() {
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

fn resolve_abs_file_target(path_str: &str) -> String {
    if Path::new(path_str).exists() {
        return path_str.to_owned();
    }
    let (base, line, _) = split_path_line_col(path_str);
    if line.is_some() && Path::new(base).exists() {
        return base.to_owned();
    }
    path_str.to_owned()
}

fn resolve_relative_file(trimmed: &str, cwd: Option<&str>) -> Option<String> {
    let dir = cwd.map(str::trim).filter(|d| !d.is_empty())?;
    let base_dir = PathBuf::from(expand_home(dir));
    let (rel_path, _, _) = split_path_line_col(trimmed);
    if rel_path.is_empty() {
        return None;
    }
    let resolved = clean_joined_path(&base_dir, rel_path);
    resolved.exists().then(|| resolved.to_string_lossy().into_owned())
}

pub fn resolve_link_target(raw_url: &str, cwd: Option<&str>) -> Option<String> {
    let trimmed = strip_link_punctuation(raw_url);
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("file://") {
        let local = uri_to_local_path(trimmed)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| trimmed.trim_start_matches("file://").to_owned());
        return Some(resolve_abs_file_target(&local));
    }
    if trimmed.contains("://") {
        return Some(trimmed.to_owned());
    }
    if trimmed.starts_with("localhost:")
        || trimmed.starts_with("127.0.0.1:")
        || trimmed.starts_with("0.0.0.0:")
    {
        return Some(format!("http://{trimmed}"));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return Some(resolve_abs_file_target(&format!("{}/{rest}", home_dir())));
    }
    if trimmed.starts_with('/') {
        return Some(resolve_abs_file_target(trimmed));
    }
    resolve_relative_file(trimmed, cwd)
}

fn spawn_xdg_open(target: &str) {
    let _ = Command::new("xdg-open")
        .arg(target)
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

pub fn open_external_link(raw_url: &str) {
    if let Some(target) = resolve_link_target(raw_url, None) {
        spawn_xdg_open(&target);
    }
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

#[derive(Clone, Copy)]
struct ClipboardTargets {
    has_image: bool,
    has_text: bool,
    has_uri: bool,
}

fn take_glib_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    unsafe { g_free(ptr as *mut c_void) };
    Some(text)
}

fn clipboard_targets(clipboard: *mut GtkClipboard) -> Option<ClipboardTargets> {
    let mut targets: *mut GdkAtom = ptr::null_mut();
    let mut count: c_int = 0;
    let ok = unsafe { gtk_clipboard_wait_for_targets(clipboard, &mut targets, &mut count) };
    if ok == 0 || targets.is_null() || count <= 0 {
        unsafe { g_free(targets as *mut c_void) };
        return None;
    }
    let info = unsafe {
        ClipboardTargets {
            has_image: gtk_targets_include_image(targets, count, 0) != 0,
            has_text: gtk_targets_include_text(targets, count) != 0,
            has_uri: gtk_targets_include_uri(targets, count) != 0,
        }
    };
    unsafe { g_free(targets as *mut c_void) };
    Some(info)
}

fn pixbuf_to_png(pixbuf: *mut GdkPixbuf) -> Option<Vec<u8>> {
    let mut buf: *mut c_char = ptr::null_mut();
    let mut size: usize = 0;
    let saved = unsafe {
        gdk_pixbuf_save_to_bufferv(
            pixbuf,
            &mut buf,
            &mut size,
            c"png".as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if saved == 0 || buf.is_null() || size == 0 {
        unsafe { g_free(buf as *mut c_void) };
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf as *const u8, size).to_vec() };
    unsafe { g_free(buf as *mut c_void) };
    Some(bytes)
}

fn clipboard_png_bytes(clipboard: *mut GtkClipboard) -> Option<Vec<u8>> {
    let pixbuf = unsafe { gtk_clipboard_wait_for_image(clipboard) };
    if pixbuf.is_null() {
        return None;
    }
    let bytes = pixbuf_to_png(pixbuf);
    unsafe { g_object_unref(pixbuf) };
    bytes
}

fn clipboard_first_uri(clipboard: *mut GtkClipboard) -> Option<String> {
    let uris = unsafe { gtk_clipboard_wait_for_uris(clipboard) };
    if uris.is_null() {
        return None;
    }
    let first = unsafe {
        (!(*uris).is_null()).then(|| CStr::from_ptr(*uris).to_string_lossy().into_owned())
    };
    unsafe { g_strfreev(uris) };
    first
}

fn clipboard_text_or_uri(clipboard: *mut GtkClipboard, targets: ClipboardTargets) -> Option<String> {
    if targets.has_text {
        let text = take_glib_string(unsafe { gtk_clipboard_wait_for_text(clipboard) });
        if let Some(t) = text.filter(|s| !s.trim().is_empty()) {
            return Some(t);
        }
    }
    targets.has_uri.then(|| clipboard_first_uri(clipboard)).flatten()
}

fn uri_to_local_path(uri: &str) -> Option<PathBuf> {
    let c_uri = CString::new(uri).ok()?;
    let ptr = unsafe { g_filename_from_uri(c_uri.as_ptr(), ptr::null_mut(), ptr::null_mut()) };
    take_glib_string(ptr).map(PathBuf::from)
}

fn parse_clipboard_image_file(raw: &str) -> Option<(PathBuf, String)> {
    let first_line = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let path = if first_line.starts_with("file://") {
        uri_to_local_path(first_line)
            .unwrap_or_else(|| PathBuf::from(first_line.trim_start_matches("file://")))
    } else {
        PathBuf::from(first_line)
    };
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") && path.is_file() {
        Some((path, ext))
    } else {
        None
    }
}

fn save_or_upload_image(
    ssh_host: &str,
    bytes: &[u8],
    ext: &str,
    kind: &str,
    ts: u64,
) -> Option<(String, String)> {
    let target_path = format!("/tmp/akhterm_paste_{ts}.{ext}");
    let kb = (bytes.len() / 1024).max(1);
    if !ssh_host.is_empty() {
        if kind == "Скриншот" {
            let _ = fs::write(&target_path, bytes);
        }
        return upload_bytes_to_ssh(ssh_host, &target_path, bytes).then(|| {
            (
                format!("{target_path} "),
                format!("🖼 {kind} ({kb} КБ) загружен по SSH на {ssh_host}:{target_path}"),
            )
        });
    }
    fs::write(&target_path, bytes).ok()?;
    Some((
        format!("{target_path} "),
        format!("🖼 {kind} ({kb} КБ) сохранён в {target_path}"),
    ))
}

pub fn try_paste_clipboard_image(ssh_host: &str, force_image: bool) -> Option<(String, String)> {
    let clipboard = unsafe { gtk_clipboard_get(GDK_SELECTION_CLIPBOARD) };
    if clipboard.is_null() {
        return None;
    }
    let targets = clipboard_targets(clipboard)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if targets.has_image && (force_image || !targets.has_text) {
        let bytes = clipboard_png_bytes(clipboard)?;
        return save_or_upload_image(ssh_host, &bytes, "png", "Скриншот", ts);
    }

    if !ssh_host.is_empty() && (targets.has_text || targets.has_uri) {
        let raw = clipboard_text_or_uri(clipboard, targets)?;
        let (path, ext) = parse_clipboard_image_file(&raw)?;
        let bytes = fs::read(path).ok()?;
        return save_or_upload_image(ssh_host, &bytes, &ext, "Файл", ts);
    }

    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::Child;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    extern "C" {
        fn atexit(cb: extern "C" fn()) -> c_int;
    }

    struct BroadwayServer {
        child: Mutex<Option<Child>>,
        dir: PathBuf,
    }

    static GTK_LOCK: Mutex<()> = Mutex::new(());
    static BROADWAY: OnceLock<Option<BroadwayServer>> = OnceLock::new();

    extern "C" fn cleanup_broadway() {
        if let Some(Some(srv)) = BROADWAY.get() {
            if let Ok(mut guard) = srv.child.lock() {
                if let Some(mut child) = guard.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            let _ = fs::remove_dir_all(&srv.dir);
        }
    }

    fn spawn_broadway(dir: &Path) -> Option<Child> {
        let http_sock = dir.join("http.sock");
        Command::new("broadwayd")
            .arg("-u")
            .arg(&http_sock)
            .arg(":7")
            .env("XDG_RUNTIME_DIR", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }

    fn init_broadway_once() -> Option<BroadwayServer> {
        let dir = std::env::temp_dir().join(format!("akhterm_bway_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).ok()?;
        let mut child = spawn_broadway(&dir)?;
        let gdk_sock = dir.join("broadway8.socket");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !gdk_sock.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        std::env::set_var("GDK_BACKEND", "broadway");
        std::env::set_var("BROADWAY_DISPLAY", ":7");
        let ok = unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) };
        if ok == 0 {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_dir_all(&dir);
            return None;
        }
        unsafe { atexit(cleanup_broadway) };
        Some(BroadwayServer {
            child: Mutex::new(Some(child)),
            dir,
        })
    }

    pub(crate) fn with_headless_gtk<F: FnOnce()>(f: F) {
        let _guard = GTK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if BROADWAY.get_or_init(init_broadway_once).is_some() {
            f();
        }
    }

    pub(crate) fn pump_gtk_events() {
        for _ in 0..60 {
            if unsafe { gtk_events_pending() } == 0 {
                break;
            }
            unsafe { gtk_main_iteration_do(0) };
        }
    }

    fn create_two_vte_window() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
            let tab1 = vte_terminal_new();
            let tab2 = vte_terminal_new();
            gtk_box_pack_start(vbox, tab1, 1, 1, 0);
            gtk_box_pack_start(vbox, tab2, 1, 1, 0);
            gtk_container_add(win, vbox);
            gtk_widget_show_all(win);
            pump_gtk_events();
            (win, tab1, tab2)
        }
    }

    fn verify_image_clipboard_paste() {
        unsafe {
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            let pix = gdk_pixbuf_new(0, 1, 8, 4, 4);
            gtk_clipboard_set_image(clip, pix);
            g_object_unref(pix);
            pump_gtk_events();
        }
        let (pasted_img, status) = try_paste_clipboard_image("", false).expect("png saved");
        let saved_path = pasted_img.trim();
        let png_bytes = fs::read(saved_path).unwrap();
        assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(status.contains("Скриншот"));
        let _ = fs::remove_file(saved_path);
    }

    #[test]
    fn parses_local_image_paths_and_encoded_file_uris() {
        let dir = std::env::temp_dir().join(format!("akhterm_clip_uri_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let img = dir.join("Снимок экрана 1.png");
        let txt = dir.join("notes.txt");
        fs::write(&img, b"\x89PNG\r\n\x1a\n").unwrap();
        fs::write(&txt, b"plain").unwrap();

        let (resolved, ext) = parse_clipboard_image_file(&img.to_string_lossy()).unwrap();
        assert_eq!(resolved, img);
        assert_eq!(ext, "png");

        let encoded = format!(
            "file://{}/%D0%A1%D0%BD%D0%B8%D0%BC%D0%BE%D0%BA%20%D1%8D%D0%BA%D1%80%D0%B0%D0%BD%D0%B0%201.png\r\n",
            dir.to_string_lossy()
        );
        let (from_uri, ext_uri) = parse_clipboard_image_file(&encoded).unwrap();
        assert_eq!(from_uri, img);
        assert_eq!(ext_uri, "png");
        assert!(parse_clipboard_image_file(&txt.to_string_lossy()).is_none());
        assert!(parse_clipboard_image_file("cargo test --offline").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn vte_copy_and_paste_between_tabs_does_not_deadlock() {
        with_headless_gtk(|| {
            let (win, tab1, tab2) = create_two_vte_window();
            let sample = "привет из первой вкладки";
            unsafe {
                vte_terminal_feed(tab1, sample.as_ptr() as *const c_char, sample.len() as isize);
            }
            thread::sleep(Duration::from_millis(40));
            pump_gtk_events();
            unsafe {
                vte_terminal_select_all(tab1);
                vte_terminal_copy_clipboard_format(tab1, 1);
                pump_gtk_events();
            }

            assert!(try_paste_clipboard_image("", false).is_none());
            assert!(try_paste_clipboard_image("", true).is_none());
            assert!(try_paste_clipboard_image("unreachable-ssh-host", false).is_none());

            let clip = unsafe { gtk_clipboard_get(GDK_SELECTION_CLIPBOARD) };
            let copied = take_glib_string(unsafe { gtk_clipboard_wait_for_text(clip) }).unwrap();
            assert!(copied.contains(sample), "copied: {copied:?}");
            unsafe { vte_terminal_paste_clipboard(tab2) };
            pump_gtk_events();

            verify_image_clipboard_paste();
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
        });
    }

    #[test]
    fn parses_relative_path_line_col_including_cyrillic() {
        assert_eq!(split_path_line_col("src/session.rs:241"), ("src/session.rs", Some(241), None));
        assert_eq!(split_path_line_col("./app/main.py:42:5:"), ("./app/main.py", Some(42), Some(5)));
        assert_eq!(split_path_line_col("crates/core/lib.rs)"), ("crates/core/lib.rs", None, None));
        assert_eq!(
            split_path_line_col("учёба/модуль.py:18:3,"),
            ("учёба/модуль.py", Some(18), Some(3))
        );
    }

    #[test]
    fn resolves_relative_links_against_session_cwd_and_checks_existence() {
        let root = std::env::temp_dir().join(format!("akhterm_rel_link_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cyr_dir = root.join("учёба").join("курс");
        let sub_dir = root.join("crates").join("cli");
        fs::create_dir_all(&cyr_dir).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        let py_file = cyr_dir.join("модуль.py");
        let rs_file = root.join("crates").join("lib.rs");
        fs::write(&py_file, b"print(1)\n").unwrap();
        fs::write(&rs_file, b"pub fn ok() {}\n").unwrap();

        let root_s = root.to_string_lossy();
        let sub_s = sub_dir.to_string_lossy();
        assert_eq!(
            resolve_link_target("./учёба/курс/модуль.py:42:5", Some(&root_s)).as_deref(),
            py_file.to_str()
        );
        assert_eq!(
            resolve_link_target("../lib.rs:10", Some(&sub_s)).as_deref(),
            rs_file.to_str()
        );
        assert_eq!(resolve_link_target("учёба/нет_файла.py:3", Some(&root_s)), None);
        assert_eq!(resolve_link_target("crates/lib.rs:10", None), None);
        assert_eq!(
            resolve_link_target("localhost:8080/health", None).as_deref(),
            Some("http://localhost:8080/health")
        );
        assert_eq!(
            resolve_link_target(&format!("{}:99:2", rs_file.display()), None).as_deref(),
            rs_file.to_str()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_proc_stat_groups_with_spaces_and_parens() {
        let sample = "4210 (gitstatusd (bg)) S 4200 4200 4200 34818 5123 4194304 0 0";
        assert_eq!(parse_stat_groups(sample), Some((4200, 5123)));
        assert_eq!(parse_stat_groups("broken stat"), None);
    }

    #[test]
    fn background_child_does_not_freeze_shell_cwd_after_cd() {
        use std::io::{BufRead, BufReader};
        let base = std::env::temp_dir().join(format!("akhterm_cwd_{}", std::process::id()));
        let dir_a = base.join("old_repo");
        let dir_b = base.join("new_project");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();

        let script = "cd \"$1\" && sleep 10 & bg=$!; cd \"$2\" && echo ready && read _ ; kill $bg 2>/dev/null";
        let mut child = Command::new("sh")
            .args(["-c", script, "--"])
            .arg(&dir_a)
            .arg(&dir_b)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();

        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "ready");

        let pid = child.id() as i32;
        let expected = shorten_home(&dir_b.to_string_lossy());
        assert_eq!(inspect_proc_tree(pid).cwd, expected);
        assert_eq!(foreground_cwd(pid).as_deref(), dir_b.to_str());

        drop(child.stdin.take());
        let _ = child.wait();
        let _ = fs::remove_dir_all(&base);
    }
}
