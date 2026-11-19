//! Process tree radar (/proc/<pid>), ~/.ssh/config parser, multi-server tmux tracker, and link launcher.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

pub use crate::clipboard_paste::{clipboard_has_direct_image, try_paste_clipboard_image};
use crate::clipboard_paste::uri_to_local_path;
use crate::paths::{
    clean_joined_path, expand_home, home_dir, shorten_home, state_dir, strip_anchor_or_line,
    strip_link_punctuation, write_state_file,
};
use crate::ssh_detect::{active_ssh_client, exe_basename, ProcEntry};
use crate::tabs::Tab;

#[derive(Clone, Debug, Default)]
pub struct ProcInfo {
    pub ssh_host: String,
    pub ssh_pid: i32,
    pub has_tmux: String,
    pub tmux_session: String,
    pub tmux_window: String,
    pub active_pane_pid: i32,
    pub agent: String,
    pub fg_cmd: String,
    pub foreground_shell: bool,
    pub cwd: String,
}

impl ProcInfo {
    pub fn shell_owns_pane(&self, tmux: Option<&(String, String, String, String)>) -> bool {
        self.foreground_shell || tmux.is_some_and(|(_, _, cmd, _)| crate::tmux_local::is_shell_name(cmd))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[allow(dead_code)]
pub fn load_ssh_hosts(watch: &[String], startup: &[Tab]) -> Vec<SshHostEntry> {
    crate::ssh_config::load_ssh_hosts(watch, startup)
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

pub(crate) fn read_proc_groups(pid: i32) -> Option<(i32, i32)> {
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

pub(crate) fn read_proc_cmdline(pid: i32) -> Option<Vec<String>> {
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

fn apply_proc_cmd(info: &mut ProcInfo, parts: &[String]) {
    let exe = exe_basename(&parts[0]);
    if exe == "tmux" {
        info.has_tmux = "tmux".to_string();
    }
    if !matches!(exe.as_str(), "zsh" | "bash" | "sh") {
        info.fg_cmd = exe;
    }
}

pub fn inspect_proc_tree(root_pid: i32) -> ProcInfo {
    inspect_proc_tree_inner(root_pid, true)
}

fn is_foreground_shell(tpgid: i32, procs: &[ProcEntry]) -> bool {
    procs.iter().any(|p| {
        p.pid == tpgid && crate::tmux_local::is_shell_name(&exe_basename(&p.argv[0]))
    })
}

fn apply_foreground_cmds(root_pid: i32, tpgid: i32, procs: &[ProcEntry], info: &mut ProcInfo) {
    for entry in procs.iter().filter(|p| tpgid <= 0 || p.pgrp == tpgid) {
        if entry.pid != root_pid {
            apply_proc_cmd(info, &entry.argv);
        } else if exe_basename(&entry.argv[0]) == "tmux" {
            info.has_tmux = "tmux".to_string();
        }
    }
}

fn apply_proc_identity(root_pid: i32, tpgid: i32, procs: &[ProcEntry], info: &mut ProcInfo) {
    if let Some((pid, host)) = active_ssh_client(root_pid, tpgid, procs) {
        info.ssh_host = host;
        info.ssh_pid = pid;
    }
    if let Some(agent) = crate::agent_detect::select_tree_agent(root_pid, tpgid, procs) {
        info.agent = agent.to_owned();
    }
}

fn populate_proc_tree_info(
    root_pid: i32,
    tpgid: i32,
    visited: &HashSet<i32>,
    procs: &[ProcEntry],
) -> ProcInfo {
    let mut info = ProcInfo::default();
    if let Some(cwd) = resolve_foreground_cwd(root_pid, Some(visited)) {
        info.cwd = shorten_home(&cwd);
    }
    apply_proc_identity(root_pid, tpgid, procs, &mut info);
    info.foreground_shell = info.agent.is_empty() && is_foreground_shell(tpgid, procs);
    apply_foreground_cmds(root_pid, tpgid, procs, &mut info);
    info
}

fn inspect_proc_tree_inner(root_pid: i32, allow_tmux: bool) -> ProcInfo {
    if root_pid <= 0 {
        return ProcInfo::default();
    }
    let (visited, procs) = collect_proc_tree(root_pid);
    let tpgid = read_proc_groups(root_pid).map_or(0, |(_, tp)| tp);
    let mut info = populate_proc_tree_info(root_pid, tpgid, &visited, &procs);
    if allow_tmux && info.ssh_host.is_empty() && !info.has_tmux.is_empty() {
        crate::tmux_local::resolve_local_tmux_into(&visited, &mut info, |p| {
            inspect_proc_tree_inner(p, false)
        });
    }
    info
}

pub fn detect_local_agent(root_pid: i32) -> Option<String> {
    if root_pid <= 0 {
        return None;
    }
    let info = inspect_proc_tree(root_pid);
    (!info.agent.is_empty()).then_some(info.agent)
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn switch_remote_tmux_session(host: &str, session_name: &str) {
    let h = host.to_string();
    let command = format!("tmux switch-client -t {} 2>/dev/null", shell_quote(session_name));
    thread::spawn(move || {
        let _ = Command::new("ssh")
            .args([
                "-o", "BatchMode=yes", "-o", "ConnectTimeout=2",
                "-o", "ClearAllForwardings=yes", "--", &h, &command,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

fn remote_tmux_gpu_cmd() -> &'static str {
    concat!(
        "tmux set -g set-titles on 2>/dev/null; ",
        "tmux set-option -g set-clipboard on 2>/dev/null; ",
        "tmux set -g set-titles-string 'AKHTERM_TMUX:v2:#{pid}:#{pane_id}|#{session_name}|#{window_index}:#{window_name}|#{pane_current_command}|#{pane_current_path}' 2>/dev/null; ",
        "tmux list-sessions -F '#{session_name} (#{session_windows} окн.)' 2>/dev/null; ",
        "echo '---CLIENTS---'; ",
        "tmux list-clients -F '#{session_name}|#{window_index}:#{window_name}|#{pane_current_command}|#{pane_current_path}' 2>/dev/null | tail -n 1; ",
        "echo '---GPU---'; ",
        "nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total --format=csv,noheader 2>/dev/null | head -n 1"
    )
}

fn run_ssh_server_query(host: &str) -> Option<String> {
    let output = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=2",
            "-o", "ClearAllForwardings=yes",
            "--", host, remote_tmux_gpu_cmd(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn record_active_client(host: &str, client_line: &str, out: &mut BoxTelemetry) -> String {
    let cparts: Vec<&str> = client_line.split('|').collect();
    if cparts.len() < 4 {
        return String::new();
    }
    out.active_by_host.push((
        host.to_owned(),
        cparts[0].to_string(),
        cparts[1].to_string(),
        cparts[2].to_string(),
        cparts[3].to_string(),
    ));
    cparts[0].to_string()
}

fn parse_client_and_gpu(host: &str, tail: &str, out: &mut BoxTelemetry) -> String {
    let mut parts = tail.split("---GPU---");
    let active_sess = record_active_client(host, parts.next().unwrap_or("").trim(), out);
    if let Some(gpu) = parts.next().map(str::trim).filter(|s| !s.is_empty()) {
        out.gpu_lines.push(format!("[{host}] {gpu}"));
    }
    active_sess
}

fn parse_host_sessions(host: &str, block: &str, active_sess: &str, out: &mut BoxTelemetry) {
    for line in block.lines().map(str::trim).filter(|s| !s.is_empty()) {
        let sname = line.rsplit_once(" (").map_or(line, |(name, _)| name).to_string();
        let is_active = !active_sess.is_empty() && sname == active_sess;
        out.tmux_items.push(RemoteTmuxItem {
            host: host.to_owned(),
            session_name: sname,
            display_line: format!("[{host}] {line}"),
            is_active,
        });
    }
}

fn apply_host_telemetry(host: &str, stdout: &str, out: &mut BoxTelemetry) {
    let mut parts = stdout.split("---CLIENTS---");
    let sessions = parts.next().unwrap_or("");
    let active_sess = parts
        .next()
        .map_or_else(String::new, |tail| parse_client_and_gpu(host, tail, out));
    parse_host_sessions(host, sessions, &active_sess, out);
}

pub fn query_ssh_servers(hosts: &[String]) -> BoxTelemetry {
    let mut out = BoxTelemetry::default();
    if hosts.is_empty() {
        out.gpu_lines.push("Опрос SSH выключен".to_owned());
        return out;
    }
    for host in hosts {
        if let Some(stdout) = run_ssh_server_query(host) {
            apply_host_telemetry(host, &stdout, &mut out);
        }
    }
    if out.gpu_lines.is_empty() {
        out.gpu_lines.push("GPU: нет активного ответа от SSH".to_string());
    }
    out
}

fn resolve_abs_file_target(path_str: &str) -> String {
    if Path::new(path_str).exists() {
        return path_str.to_owned();
    }
    let candidate = strip_anchor_or_line(path_str);
    if candidate != path_str && Path::new(candidate).exists() {
        return candidate.to_owned();
    }
    path_str.to_owned()
}

fn resolve_relative_file(trimmed: &str, cwd: Option<&str>) -> Option<String> {
    let dir = cwd.map(str::trim).filter(|d| !d.is_empty())?;
    let base_dir = PathBuf::from(expand_home(dir));
    let rel_path = strip_anchor_or_line(trimmed);
    if rel_path.is_empty() {
        return None;
    }
    let resolved = clean_joined_path(&base_dir, rel_path);
    resolved.exists().then(|| resolved.to_string_lossy().into_owned())
}

fn resolve_file_uri_target(trimmed: &str) -> String {
    if let Some(full) = crate::file_drop::decode_file_uri(trimmed).filter(|p| p.exists()) {
        return full.to_string_lossy().into_owned();
    }
    let base_uri = trimmed.split(['#', '?']).next().unwrap_or(trimmed);
    let local = crate::file_drop::decode_file_uri(base_uri)
        .or_else(|| uri_to_local_path(base_uri))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| base_uri.trim_start_matches("file://").to_owned());
    resolve_abs_file_target(&local)
}

fn is_local_http_host(trimmed: &str) -> bool {
    ["localhost:", "127.0.0.1:", "0.0.0.0:"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

fn resolve_url_or_abs_link(trimmed: &str) -> Option<String> {
    if trimmed.starts_with("file://") {
        return Some(resolve_file_uri_target(trimmed));
    }
    if trimmed.contains("://") {
        return Some(trimmed.to_owned());
    }
    if is_local_http_host(trimmed) {
        return Some(format!("http://{trimmed}"));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return Some(resolve_abs_file_target(&format!("{}/{rest}", home_dir())));
    }
    trimmed.starts_with('/').then(|| resolve_abs_file_target(trimmed))
}

pub fn resolve_link_target(raw_url: &str, cwd: Option<&str>) -> Option<String> {
    let trimmed = strip_link_punctuation(raw_url);
    if trimmed.is_empty() {
        return None;
    }
    resolve_url_or_abs_link(trimmed).or_else(|| resolve_relative_file(trimmed, cwd))
}

pub fn open_external_link(raw_url: &str) {
    if let Some(target) = resolve_link_target(raw_url, None) {
        let _ = crate::platform::spawn_desktop_opener(&target);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::clipboard_paste::take_glib_string;
    use crate::clipboard_paste::tests::{
        verify_image_clipboard_paste, verify_non_image_uri_paste_and_plain_text_guard,
    };
    use crate::ffi::*;
    use crate::paths::split_path_line_col;
    use std::ffi::{c_char, c_int};
    use std::process::Child;
    use std::ptr;
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

    fn wait_for_broadway_socket(dir: &Path) {
        let gdk_sock = dir.join("broadway8.socket");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !gdk_sock.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn init_broadway_gtk(dir: &Path, child: &mut Child) -> bool {
        std::env::set_var("XDG_RUNTIME_DIR", dir);
        std::env::set_var("GDK_BACKEND", "broadway");
        std::env::set_var("BROADWAY_DISPLAY", ":7");
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } != 0 {
            return true;
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(dir);
        false
    }

    fn init_broadway_once() -> Option<BroadwayServer> {
        let dir = std::env::temp_dir().join(format!("akhterm_bway_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).ok()?;
        let mut child = spawn_broadway(&dir)?;
        wait_for_broadway_socket(&dir);
        if !init_broadway_gtk(&dir, &mut child) {
            return None;
        }
        unsafe { atexit(cleanup_broadway) };
        Some(BroadwayServer { child: Mutex::new(Some(child)), dir })
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

    fn feed_and_copy_vte_sample(tab1: *mut GtkWidget, sample: &str) {
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
    }

    fn verify_text_clipboard_paste(tab1: *mut GtkWidget, tab2: *mut GtkWidget) {
        let sample = "привет из первой вкладки";
        feed_and_copy_vte_sample(tab1, sample);
        assert!(!clipboard_has_direct_image());
        assert!(try_paste_clipboard_image("", false).is_none());
        assert!(try_paste_clipboard_image("", true).is_none());
        assert!(try_paste_clipboard_image("unreachable-ssh-host", false).is_none());
        let clip = unsafe { gtk_clipboard_get(GDK_SELECTION_CLIPBOARD) };
        let copied = take_glib_string(unsafe { gtk_clipboard_wait_for_text(clip) }).unwrap();
        assert!(copied.contains(sample), "copied: {copied:?}");
        unsafe { vte_terminal_paste_clipboard(tab2) };
        pump_gtk_events();
    }

    #[test]
    fn vte_copy_and_paste_between_tabs_does_not_deadlock() {
        with_headless_gtk(|| {
            let (win, tab1, tab2) = create_two_vte_window();
            verify_text_clipboard_paste(tab1, tab2);
            verify_image_clipboard_paste();
            verify_non_image_uri_paste_and_plain_text_guard();
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
        });
    }

    #[test]
    fn proc_radar_maintains_mi_target() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("proc_radar.rs"));
        assert!(mi >= 60.0, "proc_radar.rs MI {mi:.2} < 60.0");
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

    fn seed_relative_link_fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let _ = fs::remove_dir_all(root);
        let cyr_dir = root.join("учёба").join("курс");
        let sub_dir = root.join("crates").join("cli");
        fs::create_dir_all(&cyr_dir).unwrap();
        fs::create_dir_all(&sub_dir).unwrap();
        let py_file = cyr_dir.join("модуль.py");
        let rs_file = root.join("crates").join("lib.rs");
        fs::write(&py_file, b"print(1)\n").unwrap();
        fs::write(&rs_file, b"pub fn ok() {}\n").unwrap();
        (sub_dir, py_file, rs_file)
    }

    fn assert_uri_link_cases(root: &Path, py_file: &Path) {
        let enc_uri = format!("file://{}/%D1%83%D1%87%D1%91%D0%B1%D0%B0/%D0%BA%D1%83%D1%80%D1%81/%D0%BC%D0%BE%D0%B4%D1%83%D0%BB%D1%8C.py#L5-L9", root.display());
        assert_eq!(resolve_link_target(&enc_uri, None).as_deref(), py_file.to_str());
        let mipt_pdf = root.join("ПП_МФТИ_003_договор.pdf");
        fs::write(&mipt_pdf, b"%PDF-1.7\n").unwrap();
        let mipt_uri = format!("file://{}/%D0%9F%D0%9F_%D0%9C%D0%A4%D0%A2%D0%98_003_%D0%B4%D0%BE%D0%B3%D0%BE%D0%B2%D0%BE%D1%80.pdf", root.display());
        assert_eq!(resolve_link_target(&mipt_uri, None).as_deref(), mipt_pdf.to_str());
        assert_eq!(strip_link_punctuation("~/Development/Курс_(ЦУ))"), "~/Development/Курс_(ЦУ)");
    }

    #[test]
    fn resolves_relative_links_against_session_cwd_and_checks_existence() {
        let root = std::env::temp_dir().join(format!("akhterm_rel_link_{}", std::process::id()));
        let (sub_dir, py_file, rs_file) = seed_relative_link_fixture(&root);
        let (root_s, sub_s) = (root.to_string_lossy(), sub_dir.to_string_lossy());
        assert_eq!(resolve_link_target("./учёба/курс/модуль.py:42:5", Some(&root_s)).as_deref(), py_file.to_str());
        assert_eq!(resolve_link_target("./учёба/курс/модуль.py#L10-L20", Some(&root_s)).as_deref(), py_file.to_str());
        assert_eq!(resolve_link_target("../lib.rs:10", Some(&sub_s)).as_deref(), rs_file.to_str());
        assert_eq!(resolve_link_target("учёба/нет_файла.py:3", Some(&root_s)), None);
        assert_eq!(resolve_link_target("crates/lib.rs:10", None), None);
        assert_eq!(resolve_link_target("localhost:8080/health", None).as_deref(), Some("http://localhost:8080/health"));
        assert_eq!(resolve_link_target(&format!("{}:99:2", rs_file.display()), None).as_deref(), rs_file.to_str());
        assert_eq!(resolve_link_target(&format!("{}#L12-L34", py_file.display()), None).as_deref(), py_file.to_str());
        assert_uri_link_cases(&root, &py_file);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_proc_stat_groups_with_spaces_and_parens() {
        let sample = "4210 (gitstatusd (bg)) S 4200 4200 4200 34818 5123 4194304 0 0";
        assert_eq!(parse_stat_groups(sample), Some((4200, 5123)));
        assert_eq!(parse_stat_groups("broken stat"), None);
    }

    fn spawn_bg_cd_shell(dir_a: &Path, dir_b: &Path) -> Child {
        let script = "cd \"$1\" && sleep 10 & bg=$!; cd \"$2\" && echo ready && read _ ; kill $bg 2>/dev/null";
        Command::new("sh")
            .args(["-c", script, "--"])
            .arg(dir_a)
            .arg(dir_b)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    }

    #[test]
    fn background_child_does_not_freeze_shell_cwd_after_cd() {
        use std::io::{BufRead, BufReader};
        let base = std::env::temp_dir().join(format!("akhterm_cwd_{}", std::process::id()));
        let (dir_a, dir_b) = (base.join("old_repo"), base.join("new_project"));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();
        let mut child = spawn_bg_cd_shell(&dir_a, &dir_b);
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "ready");
        let pid = child.id() as i32;
        assert_eq!(inspect_proc_tree(pid).cwd, shorten_home(&dir_b.to_string_lossy()));
        assert_eq!(foreground_cwd(pid).as_deref(), dir_b.to_str());
        drop(child.stdin.take());
        let _ = child.wait();
        let _ = fs::remove_dir_all(&base);
    }
}
