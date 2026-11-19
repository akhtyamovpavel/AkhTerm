//! Live local (/proc/net/tcp{,6} + /proc/<pid>/fd) and remote SSH listening port radar.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::paths::strip_link_punctuation;
use crate::proc_radar::read_proc_cmdline;
use crate::telemetry::ssh_connection::SshRoute;

pub const REMOTE_PORT_TTL: Duration = Duration::from_secs(8);
pub const DEFAULT_EPHEMERAL_RANGE: (u16, u16) = (32_768, 60_999);
const IANA_DYNAMIC_PORT_START: u16 = 49_152;
const MAX_TREE_PIDS: usize = 256;
const REMOTE_PROBE_SCRIPT: &str = concat!(
    "if command -v ss >/dev/null 2>&1; then ",
    "ss -tlnp 2>/dev/null || ss -tln 2>/dev/null; ",
    "else ",
    "echo '---PROC_TCP---'; cat /proc/net/tcp 2>/dev/null; ",
    "echo '---PROC_TCP6---'; cat /proc/net/tcp6 2>/dev/null; ",
    "fi"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindScope {
    Loopback,
    Wildcard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListeningSocket {
    pub port: u16,
    pub inode: u64,
    pub scope: BindScope,
    pub ipv6: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedPort {
    pub port: u16,
    pub process: String,
    pub scheme: String,
    pub scope: BindScope,
}

impl DetectedPort {
    pub fn http(port: u16, process: String, scope: BindScope) -> Self {
        Self { port, process, scheme: "http".to_owned(), scope }
    }
}

fn classify_ipv4(ip: Ipv4Addr) -> Option<BindScope> {
    if ip == Ipv4Addr::LOCALHOST {
        Some(BindScope::Loopback)
    } else if ip.is_unspecified() {
        Some(BindScope::Wildcard)
    } else {
        None
    }
}

fn parse_ipv4_hex_scope(hex: &str) -> Option<BindScope> {
    if hex.len() != 8 {
        return None;
    }
    let word = u32::from_str_radix(hex, 16).ok()?;
    classify_ipv4(Ipv4Addr::from(word.to_ne_bytes()))
}

fn parse_ipv6_hex_bytes(hex: &str) -> Option<[u8; 16]> {
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for i in 0..4 {
        let word = u32::from_str_radix(&hex[i * 8..(i + 1) * 8], 16).ok()?;
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&word.to_ne_bytes());
    }
    Some(bytes)
}

fn parse_ipv6_hex_scope(hex: &str) -> Option<BindScope> {
    let ip = Ipv6Addr::from(parse_ipv6_hex_bytes(hex)?);
    if ip == Ipv6Addr::LOCALHOST {
        return Some(BindScope::Loopback);
    }
    if ip.is_unspecified() {
        return Some(BindScope::Wildcard);
    }
    ip.to_ipv4_mapped().and_then(classify_ipv4)
}

fn parse_hex_port(hex: &str) -> Option<u16> {
    u16::from_str_radix(hex, 16).ok().filter(|&p| p > 0)
}

fn parse_addr_scope(addr_hex: &str, ipv6: bool) -> Option<BindScope> {
    if ipv6 { parse_ipv6_hex_scope(addr_hex) } else { parse_ipv4_hex_scope(addr_hex) }
}

fn parse_proc_net_row(line: &str, ipv6: bool) -> Option<ListeningSocket> {
    let mut fields = line.split_ascii_whitespace();
    let local = fields.nth(1)?;
    if !fields.nth(1)?.eq_ignore_ascii_case("0A") {
        return None;
    }
    let inode = fields.nth(5)?.parse::<u64>().ok()?;
    let (addr_hex, port_hex) = local.split_once(':')?;
    let scope = parse_addr_scope(addr_hex, ipv6)?;
    let port = parse_hex_port(port_hex)?;
    Some(ListeningSocket { port, inode, scope, ipv6 })
}

pub fn parse_proc_net_tcp(content: &str) -> Vec<ListeningSocket> {
    content.lines().skip(1).filter_map(|l| parse_proc_net_row(l, false)).collect()
}

pub fn parse_proc_net_tcp6(content: &str) -> Vec<ListeningSocket> {
    content.lines().skip(1).filter_map(|l| parse_proc_net_row(l, true)).collect()
}

pub fn read_local_listening_sockets() -> Vec<ListeningSocket> {
    let mut out = Vec::new();
    if let Ok(raw) = fs::read_to_string("/proc/net/tcp") {
        out.extend(parse_proc_net_tcp(&raw));
    }
    if let Ok(raw) = fs::read_to_string("/proc/net/tcp6") {
        out.extend(parse_proc_net_tcp6(&raw));
    }
    out
}

pub fn is_local_port_listening(port: u16) -> bool {
    read_local_listening_sockets().iter().any(|s| s.port == port)
}

pub fn parse_ephemeral_port_range(raw: &str) -> Option<(u16, u16)> {
    let mut parts = raw.split_ascii_whitespace();
    let lo = parts.next()?.parse::<u16>().ok()?;
    let hi = parts.next()?.parse::<u16>().ok()?;
    (lo > 0 && lo <= hi).then_some((lo, hi))
}

pub fn read_ephemeral_port_range() -> (u16, u16) {
    fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .ok()
        .and_then(|raw| parse_ephemeral_port_range(&raw))
        .unwrap_or(DEFAULT_EPHEMERAL_RANGE)
}

pub fn is_ephemeral_port(port: u16, range: (u16, u16)) -> bool {
    (port >= range.0 && port <= range.1) || port >= IANA_DYNAMIC_PORT_START
}

pub fn filter_radar_ports(ports: Vec<DetectedPort>, range: (u16, u16)) -> Vec<DetectedPort> {
    ports.into_iter().filter(|p| !is_ephemeral_port(p.port, range)).collect()
}

fn push_children(pid: i32, queue: &mut Vec<i32>) {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else { return };
    for task in tasks.flatten() {
        let Ok(raw) = fs::read_to_string(task.path().join("children")) else { continue };
        queue.extend(raw.split_whitespace().filter_map(|t| t.parse::<i32>().ok()));
    }
}

pub fn collect_tree_pids(roots: &[i32]) -> Vec<i32> {
    let mut seen = HashSet::new();
    let mut queue: Vec<i32> = roots.iter().copied().filter(|&p| p > 0).collect();
    let mut out = Vec::new();
    while let Some(pid) = queue.pop() {
        if out.len() >= MAX_TREE_PIDS || !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        push_children(pid, &mut queue);
    }
    out
}

fn parse_socket_inode(link: &Path) -> Option<u64> {
    let text = link.to_string_lossy();
    let inner = text.strip_prefix("socket:[")?.strip_suffix(']')?;
    inner.parse::<u64>().ok()
}

pub fn read_pid_socket_inodes(pid: i32) -> Vec<u64> {
    let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| fs::read_link(e.path()).ok())
        .filter_map(|target| parse_socket_inode(&target))
        .collect()
}

fn basename_of(raw: &str) -> &str {
    Path::new(raw).file_name().and_then(|s| s.to_str()).unwrap_or(raw)
}

fn is_script_runtime(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    low.starts_with("python") || matches!(low.as_str(), "node" | "nodejs" | "bun" | "deno" | "ruby" | "java")
}

fn runtime_script_hint(argv: &[String]) -> Option<String> {
    let mut iter = argv.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "-m" {
            return iter.next().map(|s| basename_of(s).to_owned());
        }
        if !arg.starts_with('-') && !arg.is_empty() {
            return Some(basename_of(arg).to_owned());
        }
    }
    None
}

pub fn process_label_from_argv(argv: &[String]) -> String {
    let Some(first) = argv.first() else { return String::new() };
    let exe = basename_of(first);
    if is_script_runtime(exe) {
        if let Some(hint) = runtime_script_hint(argv).filter(|h| !h.is_empty()) {
            return hint;
        }
    }
    exe.to_owned()
}

fn resolve_pid_label(pid: i32) -> String {
    if let Some(argv) = read_proc_cmdline(pid) {
        let label = process_label_from_argv(&argv);
        if !label.is_empty() {
            return label;
        }
    }
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

fn upsert_detected_port(map: &mut HashMap<u16, DetectedPort>, item: DetectedPort) {
    map.entry(item.port)
        .and_modify(|cur| {
            if cur.process.is_empty() && !item.process.is_empty() {
                cur.process.clone_from(&item.process);
            }
            if cur.scheme == "http" && item.scheme == "https" {
                cur.scheme = "https".to_owned();
            }
        })
        .or_insert(item);
}

fn finalize_port_map(map: HashMap<u16, DetectedPort>) -> Vec<DetectedPort> {
    let mut out: Vec<DetectedPort> = map.into_values().collect();
    out.sort_by_key(|p| p.port);
    out
}

fn collect_pid_ports(
    pid: i32,
    by_inode: &HashMap<u64, &ListeningSocket>,
    found: &mut HashMap<u16, DetectedPort>,
) {
    let mut label: Option<String> = None;
    for inode in read_pid_socket_inodes(pid) {
        let Some(sock) = by_inode.get(&inode) else { continue };
        let process = label.get_or_insert_with(|| resolve_pid_label(pid)).clone();
        upsert_detected_port(found, DetectedPort::http(sock.port, process, sock.scope));
    }
}

pub fn detect_ports_for_pids_with_table(
    root_pids: &[i32],
    sockets: &[ListeningSocket],
) -> Vec<DetectedPort> {
    let by_inode: HashMap<u64, &ListeningSocket> =
        sockets.iter().filter(|s| s.inode > 0).map(|s| (s.inode, s)).collect();
    if by_inode.is_empty() {
        return Vec::new();
    }
    let mut found = HashMap::new();
    for pid in collect_tree_pids(root_pids) {
        collect_pid_ports(pid, &by_inode, &mut found);
    }
    finalize_port_map(found)
}

#[cfg(test)]
pub fn detect_local_ports_for_pids(root_pids: &[i32]) -> Vec<DetectedPort> {
    let sockets = read_local_listening_sockets();
    detect_ports_for_pids_with_table(root_pids, &sockets)
}

fn classify_ss_host(raw_host: &str) -> Option<BindScope> {
    let trimmed = raw_host.trim_start_matches('[').trim_end_matches(']');
    let host = trimmed.split('%').next().unwrap_or(trimmed);
    match host {
        "127.0.0.1" | "::1" | "::ffff:127.0.0.1" => Some(BindScope::Loopback),
        "0.0.0.0" | "*" | "::" | "::ffff:0.0.0.0" => Some(BindScope::Wildcard),
        _ => None,
    }
}

fn extract_ss_process(line: &str) -> String {
    let Some(rest) = line.split("users:((\"").nth(1) else {
        return String::new();
    };
    rest.split('"').next().unwrap_or("").to_owned()
}

fn is_remote_system_ssh(port: u16, process: &str) -> bool {
    port == 22 && (process.is_empty() || process == "sshd")
}

fn extract_ss_local_col<'a>(cols: &[&'a str]) -> Option<&'a str> {
    match cols.first().copied() {
        Some("LISTEN") => cols.get(3).copied(),
        Some("tcp" | "tcp6") if cols.get(1) == Some(&"LISTEN") => cols.get(4).copied(),
        _ => None,
    }
}

fn parse_ss_line(line: &str) -> Option<DetectedPort> {
    let cols: Vec<&str> = line.split_ascii_whitespace().collect();
    let (host_part, port_part) = extract_ss_local_col(&cols)?.rsplit_once(':')?;
    let scope = classify_ss_host(host_part)?;
    let port = port_part.parse::<u16>().ok().filter(|&p| p > 0)?;
    let process = extract_ss_process(line);
    if is_remote_system_ssh(port, &process) || is_ephemeral_port(port, DEFAULT_EPHEMERAL_RANGE) {
        return None;
    }
    Some(DetectedPort::http(port, process, scope))
}

pub fn parse_ss_tln(output: &str) -> Vec<DetectedPort> {
    let mut map = HashMap::new();
    for item in output.lines().filter_map(parse_ss_line) {
        upsert_detected_port(&mut map, item);
    }
    finalize_port_map(map)
}

fn sockets_to_remote_ports(sockets: Vec<ListeningSocket>) -> Vec<DetectedPort> {
    let mut map = HashMap::new();
    for sock in sockets {
        if sock.port == 22 || is_ephemeral_port(sock.port, DEFAULT_EPHEMERAL_RANGE) {
            continue;
        }
        upsert_detected_port(&mut map, DetectedPort::http(sock.port, String::new(), sock.scope));
    }
    finalize_port_map(map)
}

pub fn parse_remote_probe_output(output: &str) -> Vec<DetectedPort> {
    if let Some((_, rest)) = output.split_once("---PROC_TCP---") {
        let (tcp4, tcp6) = rest.split_once("---PROC_TCP6---").unwrap_or((rest, ""));
        let mut socks = parse_proc_net_tcp(tcp4.trim_start());
        socks.extend(parse_proc_net_tcp6(tcp6.trim_start()));
        return sockets_to_remote_ports(socks);
    }
    parse_ss_tln(output)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalUrlTarget {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub suffix: String,
}

fn split_scheme(raw: &str) -> (&str, &str) {
    if let Some(rest) = raw.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = raw.strip_prefix("http://") {
        ("http", rest)
    } else {
        ("http", raw)
    }
}

fn strip_local_host(rest: &str) -> Option<(&str, &str)> {
    for host in ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"] {
        if let Some(after) = rest.strip_prefix(host) {
            if let Some(port_and_tail) = after.strip_prefix(':') {
                return Some((host, port_and_tail));
            }
        }
    }
    None
}

fn split_port_and_suffix(raw: &str) -> Option<(u16, String)> {
    let digits_len = raw.bytes().take_while(u8::is_ascii_digit).count();
    if digits_len == 0 || digits_len > 5 {
        return None;
    }
    let port = raw[..digits_len].parse::<u16>().ok().filter(|&p| p > 0)?;
    let suffix = &raw[digits_len..];
    if !suffix.is_empty() && !matches!(suffix.as_bytes()[0], b'/' | b'?' | b'#') {
        return None;
    }
    Some((port, suffix.to_owned()))
}

pub fn parse_local_url(raw: &str) -> Option<LocalUrlTarget> {
    let clean = strip_link_punctuation(raw);
    let (scheme, rest) = split_scheme(clean);
    let (host, port_tail) = strip_local_host(rest)?;
    let (port, suffix) = split_port_and_suffix(port_tail)?;
    Some(LocalUrlTarget { scheme: scheme.to_owned(), host: host.to_owned(), port, suffix })
}

pub fn rewrite_local_url(raw: &str, local_port: u16) -> Option<String> {
    let target = parse_local_url(raw)?;
    Some(format!("{}://127.0.0.1:{}{}", target.scheme, local_port, target.suffix))
}

fn is_url_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | '`' | '│' | '┃' | '┆' | '┊' | '║')
}

fn token_to_detected_port(raw_tok: &str) -> Option<DetectedPort> {
    let token = if raw_tok.contains("[::1]:") { raw_tok } else { raw_tok.trim_matches(['[', ']']) };
    let target = parse_local_url(token)?;
    let scope = if target.host == "0.0.0.0" { BindScope::Wildcard } else { BindScope::Loopback };
    Some(DetectedPort { port: target.port, process: String::new(), scheme: target.scheme, scope })
}

pub fn extract_local_urls_from_text(text: &str) -> Vec<DetectedPort> {
    let mut map = HashMap::new();
    for item in text.split(is_url_separator).filter_map(token_to_detected_port) {
        upsert_detected_port(&mut map, item);
    }
    finalize_port_map(map)
}

pub fn merge_detected_ports(primary: Vec<DetectedPort>, secondary: Vec<DetectedPort>) -> Vec<DetectedPort> {
    let mut map = HashMap::new();
    for item in primary.into_iter().chain(secondary) {
        upsert_detected_port(&mut map, item);
    }
    finalize_port_map(map)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePortTarget {
    pub host: String,
    pub ssh_pid: i32,
}

#[derive(Default)]
struct RemotePortState {
    targets: HashMap<String, i32>,
    cache: HashMap<String, Vec<DetectedPort>>,
    last_checked: HashMap<String, Instant>,
    force_hosts: HashSet<String>,
    wake_seq: u64,
}

#[derive(Clone, Default)]
pub struct RemotePortScheduler {
    inner: Arc<(Mutex<RemotePortState>, Condvar)>,
}

fn route_for_remote_port_probe(host: &str, ssh_pid: i32) -> Option<SshRoute> {
    if ssh_pid > 0 {
        if let Some(route) = SshRoute::from_client(ssh_pid, host) {
            return Some(route);
        }
    }
    SshRoute::for_host(host)
}

fn probe_remote_host_ports(host: &str, ssh_pid: i32) -> Option<Vec<DetectedPort>> {
    let route = route_for_remote_port_probe(host, ssh_pid)?;
    let child = route.spawn(REMOTE_PROBE_SCRIPT, Stdio::null(), "5").ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_remote_probe_output(&String::from_utf8_lossy(&output.stdout)))
}

impl RemotePortScheduler {
    pub fn sync_targets(&self, targets: &[RemotePortTarget]) {
        let Ok(mut st) = self.inner.0.lock() else { return };
        let mut next: HashMap<String, i32> = HashMap::new();
        for t in targets {
            if !t.host.is_empty() {
                next.entry(t.host.clone()).and_modify(|p| *p = (*p).max(t.ssh_pid)).or_insert(t.ssh_pid);
            }
        }
        let changed = next.keys().any(|h| !st.targets.contains_key(h));
        st.targets = next;
        if changed {
            st.wake_seq = st.wake_seq.wrapping_add(1);
            self.inner.1.notify_all();
        }
    }

    pub fn trigger_refresh(&self, host: &str) {
        if host.is_empty() {
            return;
        }
        let Ok(mut st) = self.inner.0.lock() else { return };
        st.force_hosts.insert(host.to_owned());
        st.wake_seq = st.wake_seq.wrapping_add(1);
        self.inner.1.notify_all();
    }

    pub fn ports_for_host(&self, host: &str) -> Vec<DetectedPort> {
        let Ok(st) = self.inner.0.lock() else { return Vec::new() };
        st.cache.get(host).cloned().unwrap_or_default()
    }

    #[cfg(test)]
    pub fn seed_cache_for_test(&self, host: &str, ports: Vec<DetectedPort>) {
        if let Ok(mut st) = self.inner.0.lock() {
            st.cache.insert(host.to_owned(), ports);
        }
    }

    fn collect_due_jobs(&self, now: Instant) -> Vec<(String, i32)> {
        let Ok(mut st) = self.inner.0.lock() else { return Vec::new() };
        let state = &mut *st;
        let mut due = Vec::new();
        for (host, &pid) in &state.targets {
            let forced = state.force_hosts.remove(host);
            let stale = state.last_checked.get(host).is_none_or(|t| now.saturating_duration_since(*t) >= REMOTE_PORT_TTL);
            if forced || stale {
                due.push((host.clone(), pid));
            }
        }
        due
    }

    pub fn refresh_due(&self, now: Instant) -> usize {
        let due = self.collect_due_jobs(now);
        let mut updated = 0usize;
        for (host, pid) in due {
            let result = probe_remote_host_ports(&host, pid);
            let Ok(mut st) = self.inner.0.lock() else { continue };
            st.last_checked.insert(host.clone(), Instant::now());
            if let Some(ports) = result {
                if st.cache.get(&host) != Some(&ports) {
                    st.cache.insert(host, ports);
                    updated += 1;
                }
            }
        }
        updated
    }

    pub fn wait_for_turn(&self, timeout: Duration) {
        let Ok(st) = self.inner.0.lock() else { return };
        let seq = st.wake_seq;
        let _ = self.inner.1.wait_timeout_while(st, timeout, |s| s.wake_seq == seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parses_proc_net_tcp_ipv4_endianness_and_listen_state() {
        let sample = concat!(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
            "   0: 0100007F:1F40 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1001 1 0\n",
            "   1: 00000000:1435 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1002 1 0\n",
            "   2: 0100007F:0BB8 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 1003 1 0\n",
            "   3: 0501A8C0:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1004 1 0\n",
            "   4: 3500007F:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000   101        0 1005 1 0\n",
        );
        let socks = parse_proc_net_tcp(sample);
        assert_eq!(socks.len(), 2);
        assert_eq!(socks[0], ListeningSocket { port: 8000, inode: 1001, scope: BindScope::Loopback, ipv6: false });
        assert_eq!(socks[1], ListeningSocket { port: 5173, inode: 1002, scope: BindScope::Wildcard, ipv6: false });
    }

    #[test]
    fn parses_proc_net_tcp6_loopback_wildcard_and_v4_mapped() {
        let sample = concat!(
            "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
            "   0: 00000000000000000000000001000000:0BB8 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 2001 1 0\n",
            "   1: 00000000000000000000000000000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00:00000000 1000 0 2002 1 0\n",
            "   2: 0000000000000000FFFF00000100007F:2328 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 2003 1 0\n",
            "   3: 000080FE000000000000000001000000:2710 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 2004 1 0\n",
        );
        let socks = parse_proc_net_tcp6(sample);
        assert_eq!(socks.len(), 3);
        assert_eq!(socks[0], ListeningSocket { port: 3000, inode: 2001, scope: BindScope::Loopback, ipv6: true });
        assert_eq!(socks[1], ListeningSocket { port: 8080, inode: 2002, scope: BindScope::Wildcard, ipv6: true });
        assert_eq!(socks[2], ListeningSocket { port: 9000, inode: 2003, scope: BindScope::Loopback, ipv6: true });
    }

    #[test]
    fn parses_ss_tln_and_tlnp_and_proc_fallback() {
        let ss_out = concat!(
            "State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process\n",
            "LISTEN 0      128    0.0.0.0:22          0.0.0.0:*         users:((\"sshd\",pid=10,fd=3))\n",
            "LISTEN 0      511    127.0.0.1:8000      0.0.0.0:*         users:((\"uvicorn\",pid=42,fd=7))\n",
            "LISTEN 0      511    [::1]:5173          [::]:*            users:((\"node\",pid=99,fd=12))\n",
            "tcp    LISTEN 0 128  *:3000              *:*\n",
            "LISTEN 0      128    127.0.0.1%lo:8888   0.0.0.0:*\n",
            "LISTEN 0      128    192.168.1.50:9999   0.0.0.0:*\n",
        );
        let ports = parse_remote_probe_output(ss_out);
        assert_eq!(ports.iter().map(|p| (p.port, p.process.as_str())).collect::<Vec<_>>(), vec![
            (3000, ""), (5173, "node"), (8000, "uvicorn"), (8888, ""),
        ]);
    }

    #[test]
    fn parses_remote_proc_tcp_fallback_output() {
        let fallback = concat!(
            "---PROC_TCP---\n",
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
            "   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0 0 100 1 0\n",
            "   1: 0100007F:1F40 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000 0 101 1 0\n",
            "---PROC_TCP6---\n",
            "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n",
            "   0: 00000000000000000000000001000000:1435 00000000000000000000000000000000:0000 0A 0:0 0:0 0 1000 0 102 1 0\n",
        );
        let fb = parse_remote_probe_output(fallback);
        assert_eq!(fb.iter().map(|p| p.port).collect::<Vec<_>>(), vec![5173, 8000]);
    }

    #[test]
    fn parses_and_rewrites_local_urls() {
        let u = parse_local_url("https://localhost:8443/api/v1?x=1#top,").unwrap();
        assert_eq!((u.scheme.as_str(), u.port, u.suffix.as_str()), ("https", 8443, "/api/v1?x=1#top"));
        assert_eq!(rewrite_local_url("https://localhost:8443/api/v1?x=1", 49152).as_deref(), Some("https://127.0.0.1:49152/api/v1?x=1"));
        assert_eq!(rewrite_local_url("0.0.0.0:5173/dashboard", 5174).as_deref(), Some("http://127.0.0.1:5174/dashboard"));
        assert_eq!(rewrite_local_url("http://[::1]:3000/app", 3001).as_deref(), Some("http://127.0.0.1:3001/app"));
        assert!(parse_local_url("https://example.com:8000/test").is_none());
        assert!(parse_local_url("http://localhost:0/bad").is_none());
    }

    #[test]
    fn extracts_and_merges_urls_from_terminal_text() {
        let log = "INFO: Uvicorn running on http://0.0.0.0:8000 (Press CTRL+C)\n  Local: https://127.0.0.1:5173/\n  IPv6: http://[::1]:3000/home\n";
        let extracted = extract_local_urls_from_text(log);
        assert_eq!(extracted.iter().map(|p| (p.port, p.scheme.as_str())).collect::<Vec<_>>(), vec![
            (3000, "http"), (5173, "https"), (8000, "http"),
        ]);
        let merged = merge_detected_ports(
            vec![DetectedPort { port: 5173, process: "vite".into(), scheme: "http".into(), scope: BindScope::Loopback }],
            extracted,
        );
        let p5173 = merged.iter().find(|p| p.port == 5173).unwrap();
        assert_eq!(p5173.process, "vite");
        assert_eq!(p5173.scheme, "https");
    }

    #[test]
    fn derives_clean_process_labels_from_argv() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(process_label_from_argv(&s(&["/usr/bin/python3", "-m", "uvicorn", "app:app"])), "uvicorn");
        assert_eq!(process_label_from_argv(&s(&["node", "/proj/node_modules/.bin/vite", "--port", "5173"])), "vite");
        assert_eq!(process_label_from_argv(&s(&["/usr/local/bin/caddy", "run"])), "caddy");
    }

    #[test]
    fn filters_linux_and_iana_ephemeral_ports_while_keeping_dev_ports() {
        let range = parse_ephemeral_port_range("32768\t60999\n").unwrap();
        assert_eq!(range, DEFAULT_EPHEMERAL_RANGE);
        let sample = vec![
            DetectedPort::http(3000, "vite".into(), BindScope::Loopback),
            DetectedPort::http(8000, "uvicorn".into(), BindScope::Wildcard),
            DetectedPort::http(11434, "ollama".into(), BindScope::Loopback),
            DetectedPort::http(33849, "agy".into(), BindScope::Loopback),
            DetectedPort::http(45141, "language_server".into(), BindScope::Loopback),
            DetectedPort::http(61500, "client".into(), BindScope::Loopback),
        ];
        let kept: Vec<u16> = filter_radar_ports(sample, range).into_iter().map(|p| p.port).collect();
        assert_eq!(kept, vec![3000, 8000, 11434]);
        let ss_out = concat!(
            "State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process\n",
            "LISTEN 0      128    127.0.0.1:33849     0.0.0.0:*         users:((\"agy\",pid=7,fd=9))\n",
            "LISTEN 0      128    127.0.0.1:8000      0.0.0.0:*         users:((\"uvicorn\",pid=8,fd=4))\n",
        );
        assert_eq!(parse_remote_probe_output(ss_out).into_iter().map(|p| p.port).collect::<Vec<_>>(), vec![8000]);
    }

    #[test]
    fn detects_real_ephemeral_tcp_listener_bound_by_current_process() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let pid = std::process::id() as i32;
        let detected = detect_local_ports_for_pids(&[pid]);
        let hit = detected.iter().find(|p| p.port == port);
        assert!(hit.is_some(), "expected port {port} in {detected:?}");
        assert_eq!(hit.unwrap().scope, BindScope::Loopback);
        assert!(is_local_port_listening(port));
        drop(listener);
    }
}
