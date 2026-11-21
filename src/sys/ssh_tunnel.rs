//! Managed background `ssh -N -L` port forwarding with auto port picking, dedup, and Drop cleanup.

use std::io;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::telemetry::ssh_connection::SshRoute;

#[allow(unused_imports)]
pub use crate::sys::pinned_tunnels::{
    list_pinned_for_host, list_pinned_tunnels, pin_tunnel, pinned_tunnels_path, unpin_tunnel,
    PinnedTunnel,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TunnelStatus {
    Active,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelRequest {
    pub host: String,
    pub ssh_pid: i32,
    pub remote_port: u16,
    pub scheme: String,
    pub process: String,
    pub session_id: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelInfo {
    pub id: u64,
    pub host: String,
    pub remote_port: u16,
    pub local_port: u16,
    pub scheme: String,
    pub process: String,
    pub session_ids: Vec<usize>,
    pub pid: u32,
    pub status: TunnelStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelEnsureResult {
    pub info: TunnelInfo,
    pub reused: bool,
}

struct ManagedTunnel {
    info: TunnelInfo,
    ssh_pid: i32,
    child: Option<Child>,
}

pub struct SshTunnelManager {
    next_id: u64,
    tunnels: Vec<ManagedTunnel>,
    #[cfg(test)]
    test_spawn: Option<(String, Vec<String>)>,
}

impl Default for SshTunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn build_tunnel_argv(local_port: u16, remote_port: u16, route_args: &[String]) -> Vec<String> {
    let forward = format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}");
    let mut argv = vec![
        "-N".to_owned(),
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
        "-o".to_owned(),
        "ServerAliveInterval=15".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=3".to_owned(),
        "-o".to_owned(),
        "ControlMaster=no".to_owned(),
        "-L".to_owned(),
        forward,
    ];
    argv.extend(route_args.iter().cloned());
    argv
}

pub fn pick_local_port_avoiding(preferred: u16, reserved: &[u16]) -> io::Result<u16> {
    if preferred > 0 && !reserved.contains(&preferred) {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", preferred)) {
            return Ok(listener.local_addr()?.port());
        }
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
pub fn pick_local_port(preferred: u16) -> io::Result<u16> {
    pick_local_port_avoiding(preferred, &[])
}

fn resolve_tunnel_route(host: &str, ssh_pid: i32) -> io::Result<SshRoute> {
    let route = (ssh_pid > 0)
        .then(|| SshRoute::from_client(ssh_pid, host))
        .flatten()
        .or_else(|| SshRoute::for_host(host));
    route.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid SSH host"))
}

fn terminate_child(child: &mut Option<Child>) {
    if let Some(mut proc_child) = child.take() {
        let _ = proc_child.kill();
        let _ = proc_child.wait();
    }
}

fn child_is_alive(child: &mut Option<Child>) -> bool {
    matches!(child.as_mut().and_then(|c| c.try_wait().ok()), Some(None))
}

fn spawn_ssh_child(program: &str, argv: &[String], cwd: &Path) -> io::Result<Child> {
    Command::new(program)
        .args(argv)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn attach_session_id(ids: &mut Vec<usize>, sid: usize) {
    if sid > 0 && !ids.contains(&sid) {
        ids.push(sid);
    }
}

fn normalize_scheme(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("https") { "https".to_owned() } else { "http".to_owned() }
}

fn apply_reconnected_child(entry: &mut ManagedTunnel, ssh_pid: i32, local_port: u16, child: Child) -> TunnelInfo {
    entry.ssh_pid = ssh_pid;
    entry.info.local_port = local_port;
    entry.info.pid = child.id();
    entry.info.status = TunnelStatus::Active;
    entry.child = Some(child);
    entry.info.clone()
}

fn pinned_to_request(item: PinnedTunnel, ssh_pid: i32, session_id: usize) -> TunnelRequest {
    TunnelRequest {
        host: item.host, ssh_pid, remote_port: item.remote_port,
        scheme: item.scheme, process: item.process, session_id,
    }
}

impl SshTunnelManager {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            tunnels: Vec::new(),
            #[cfg(test)]
            test_spawn: None,
        }
    }

    #[cfg(test)]
    pub fn with_test_command(program: &str, args: &[&str]) -> Self {
        Self {
            next_id: 1,
            tunnels: Vec::new(),
            test_spawn: Some((program.to_owned(), args.iter().map(|s| (*s).to_owned()).collect())),
        }
    }

    #[cfg(test)]
    pub fn set_test_command(&mut self, program: &str, args: &[&str]) {
        self.test_spawn = Some((program.to_owned(), args.iter().map(|s| (*s).to_owned()).collect()));
    }

    pub fn prune_exited(&mut self) -> usize {
        let mut disconnected = 0usize;
        for entry in &mut self.tunnels {
            if entry.info.status == TunnelStatus::Active && !child_is_alive(&mut entry.child) {
                entry.child = None;
                entry.info.status = TunnelStatus::Disconnected;
                entry.info.pid = 0;
                disconnected += 1;
            }
        }
        disconnected
    }

    fn reserved_local_ports(&self, skip_id: Option<u64>) -> Vec<u16> {
        self.tunnels
            .iter()
            .filter(|t| t.info.status == TunnelStatus::Active && Some(t.info.id) != skip_id)
            .map(|t| t.info.local_port)
            .collect()
    }

    fn find_live_match(&mut self, host: &str, remote_port: u16, sid: usize) -> Option<TunnelInfo> {
        for entry in &mut self.tunnels {
            if entry.info.host == host
                && entry.info.remote_port == remote_port
                && entry.info.status == TunnelStatus::Active
                && child_is_alive(&mut entry.child)
            {
                attach_session_id(&mut entry.info.session_ids, sid);
                return Some(entry.info.clone());
            }
        }
        None
    }

    fn find_disconnected_id(&self, host: &str, remote_port: u16) -> Option<u64> {
        self.tunnels
            .iter()
            .find(|t| t.info.host == host && t.info.remote_port == remote_port && t.info.status == TunnelStatus::Disconnected)
            .map(|t| t.info.id)
    }

    fn spawn_for_route(&self, route: &SshRoute, local_port: u16, remote_port: u16) -> io::Result<Child> {
        let argv = build_tunnel_argv(local_port, remote_port, route.transport_args());
        #[cfg(test)]
        if let Some((prog, args)) = &self.test_spawn {
            let _ = &argv;
            return spawn_ssh_child(prog, args, route.working_dir());
        }
        spawn_ssh_child("ssh", &argv, route.working_dir())
    }

    #[allow(dead_code)]
    pub fn reconnect_tunnel(&mut self, id: u64) -> io::Result<TunnelInfo> {
        self.reconnect_tunnel_with_ssh_pid(id, 0)
    }

    fn respawn_tunnel_at(&mut self, pos: usize, ssh_pid: i32) -> io::Result<TunnelInfo> {
        let (id, host, rport, pref) = {
            let i = &self.tunnels[pos].info;
            (i.id, i.host.clone(), i.remote_port, i.local_port)
        };
        let eff_pid = if ssh_pid > 0 { ssh_pid } else { self.tunnels[pos].ssh_pid };
        let route = resolve_tunnel_route(&host, eff_pid)?;
        let local_port = pick_local_port_avoiding(pref, &self.reserved_local_ports(Some(id)))?;
        let child = self.spawn_for_route(&route, local_port, rport)?;
        Ok(apply_reconnected_child(&mut self.tunnels[pos], eff_pid, local_port, child))
    }

    pub fn reconnect_tunnel_with_ssh_pid(&mut self, id: u64, ssh_pid: i32) -> io::Result<TunnelInfo> {
        self.prune_exited();
        let pos = self.tunnels.iter().position(|t| t.info.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "tunnel not found"))?;
        if self.tunnels[pos].info.status == TunnelStatus::Active && child_is_alive(&mut self.tunnels[pos].child) {
            return Ok(self.tunnels[pos].info.clone());
        }
        self.respawn_tunnel_at(pos, ssh_pid)
    }

    pub fn ensure_tunnel(&mut self, req: TunnelRequest) -> io::Result<TunnelEnsureResult> {
        let preferred = req.remote_port;
        self.ensure_tunnel_with_local_port(req, preferred)
    }

    fn try_reconnect_disconnected(&mut self, req: &TunnelRequest) -> Option<io::Result<TunnelEnsureResult>> {
        let id = self.find_disconnected_id(&req.host, req.remote_port)?;
        if let Some(entry) = self.tunnels.iter_mut().find(|t| t.info.id == id) {
            attach_session_id(&mut entry.info.session_ids, req.session_id);
        }
        Some(self.reconnect_tunnel_with_ssh_pid(id, req.ssh_pid).map(|info| TunnelEnsureResult { info, reused: false }))
    }

    pub fn ensure_tunnel_with_local_port(
        &mut self,
        req: TunnelRequest,
        preferred_local: u16,
    ) -> io::Result<TunnelEnsureResult> {
        self.prune_exited();
        if let Some(existing) = self.find_live_match(&req.host, req.remote_port, req.session_id) {
            return Ok(TunnelEnsureResult { info: existing, reused: true });
        }
        if let Some(res) = self.try_reconnect_disconnected(&req) {
            return res;
        }
        let route = resolve_tunnel_route(&req.host, req.ssh_pid)?;
        let local_port = pick_local_port_avoiding(preferred_local, &self.reserved_local_ports(None))?;
        let child = self.spawn_for_route(&route, local_port, req.remote_port)?;
        Ok(TunnelEnsureResult { info: self.record_tunnel(req, local_port, child), reused: false })
    }

    pub fn restore_pinned_for_host(
        &mut self,
        host: &str,
        ssh_pid: i32,
        session_id: usize,
    ) -> (Vec<TunnelInfo>, Vec<(u16, String)>) {
        let (mut restored, mut errors) = (Vec::new(), Vec::new());
        for item in list_pinned_for_host(host) {
            let rport = item.remote_port;
            let pref = item.local_port;
            let req = pinned_to_request(item, ssh_pid, session_id);
            match self.ensure_tunnel_with_local_port(req, pref) {
                Ok(res) if !res.reused => restored.push(res.info),
                Ok(_) => {}
                Err(err) => errors.push((rport, err.to_string())),
            }
        }
        (restored, errors)
    }

    fn record_tunnel(&mut self, req: TunnelRequest, local_port: u16, child: Child) -> TunnelInfo {
        let id = self.next_id;
        self.next_id += 1;
        let mut session_ids = Vec::new();
        attach_session_id(&mut session_ids, req.session_id);
        let ssh_pid = req.ssh_pid;
        let info = TunnelInfo {
            id,
            host: req.host,
            remote_port: req.remote_port,
            local_port,
            scheme: normalize_scheme(&req.scheme),
            process: req.process,
            session_ids,
            pid: child.id(),
            status: TunnelStatus::Active,
        };
        self.tunnels.push(ManagedTunnel { info: info.clone(), ssh_pid, child: Some(child) });
        info
    }

    pub fn list_tunnels_snapshot(&self) -> Vec<TunnelInfo> {
        self.tunnels.iter().map(|t| t.info.clone()).collect()
    }

    pub fn is_tunnel_alive(&mut self, id: u64) -> bool {
        self.prune_exited();
        self.tunnels
            .iter_mut()
            .any(|t| t.info.id == id && t.info.status == TunnelStatus::Active && child_is_alive(&mut t.child))
    }

    pub fn stop_tunnel(&mut self, id: u64) -> bool {
        let Some(pos) = self.tunnels.iter().position(|t| t.info.id == id) else {
            return false;
        };
        let mut removed = self.tunnels.remove(pos);
        terminate_child(&mut removed.child);
        true
    }

    pub fn stop_tunnels_for_session(&mut self, sid: usize) -> usize {
        if sid == 0 {
            return 0;
        }
        let mut stopped = 0usize;
        let mut idx = 0usize;
        while idx < self.tunnels.len() {
            let entry = &mut self.tunnels[idx];
            if entry.info.session_ids.contains(&sid) {
                entry.info.session_ids.retain(|&s| s != sid);
                if entry.info.session_ids.is_empty() {
                    let mut removed = self.tunnels.remove(idx);
                    terminate_child(&mut removed.child);
                    stopped += 1;
                    continue;
                }
            }
            idx += 1;
        }
        stopped
    }

    pub fn stop_all(&mut self) -> usize {
        let count = self.tunnels.len();
        for mut entry in self.tunnels.drain(..) {
            terminate_child(&mut entry.child);
        }
        count
    }
}

impl Drop for SshTunnelManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    fn sample_req(host: &str, remote_port: u16, session_id: usize) -> TunnelRequest {
        TunnelRequest {
            host: host.to_owned(),
            ssh_pid: 0,
            remote_port,
            scheme: "http".to_owned(),
            process: "uvicorn".to_owned(),
            session_id,
        }
    }

    fn pid_alive(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    #[test]
    fn builds_safe_ssh_tunnel_argv_reusing_route_options() {
        let raw: Vec<String> = "ssh -t -p 2222 -i keys/id -J bastion gpu01 tmux"
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let route = SshRoute::from_argv(&raw, PathBuf::from("/tmp")).unwrap();
        let argv = build_tunnel_argv(18000, 8000, route.transport_args());
        assert_eq!(argv[0], "-N");
        assert!(argv.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(argv.windows(2).any(|w| w == ["-o", "ExitOnForwardFailure=yes"]));
        assert!(argv.windows(2).any(|w| w == ["-o", "ServerAliveInterval=15"]));
        assert!(argv.windows(2).any(|w| w == ["-o", "ServerAliveCountMax=3"]));
        assert!(argv.windows(2).any(|w| w == ["-L", "127.0.0.1:18000:127.0.0.1:8000"]));
        assert!(argv.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(argv.windows(2).any(|w| w == ["-i", "keys/id"]));
        assert!(argv.windows(2).any(|w| w == ["-J", "bastion"]));
        assert_eq!(&argv[argv.len() - 2..], &["--", "gpu01"]);
    }

    #[test]
    fn local_port_picker_falls_back_on_collision() {
        let busy = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy_port = busy.local_addr().unwrap().port();
        let picked = pick_local_port(busy_port).unwrap();
        assert_ne!(picked, busy_port);
        assert!(picked > 0);
        drop(busy);
        let free = pick_local_port(picked).unwrap();
        assert!(free > 0);
    }

    #[test]
    fn deduplicates_tunnels_and_cleans_up_per_session_and_on_drop() {
        let mut mgr = SshTunnelManager::with_test_command("sleep", &["60"]);
        let first = mgr.ensure_tunnel(sample_req("dev-box", 8000, 10)).unwrap();
        assert!(!first.reused);
        let pid1 = first.info.pid;
        assert!(pid_alive(pid1));

        let dup = mgr.ensure_tunnel(sample_req("dev-box", 8000, 20)).unwrap();
        assert!(dup.reused);
        assert_eq!(dup.info.id, first.info.id);
        assert_eq!(dup.info.session_ids, vec![10, 20]);

        let second = mgr.ensure_tunnel(sample_req("dev-box", 5173, 10)).unwrap();
        let pid2 = second.info.pid;
        assert_ne!(second.info.id, first.info.id);

        assert_eq!(mgr.stop_tunnels_for_session(10), 1);
        assert!(!pid_alive(pid2));
        assert!(pid_alive(pid1));
        assert!(mgr.is_tunnel_alive(first.info.id));

        assert!(mgr.stop_tunnel(first.info.id));
        assert!(!pid_alive(pid1));

        let third = mgr.ensure_tunnel(sample_req("dev-box", 3000, 30)).unwrap();
        let pid3 = third.info.pid;
        assert!(pid_alive(pid3));
        drop(mgr);
        assert!(!pid_alive(pid3));
    }

    #[test]
    fn unexpected_exit_marks_disconnected_and_reconnect_respawns_tunnel() {
        let mut mgr = SshTunnelManager::with_test_command("true", &[]);
        let res = mgr.ensure_tunnel(sample_req("dev-box", 9000, 1)).unwrap();
        assert_eq!(res.info.status, TunnelStatus::Active);
        for _ in 0..40 {
            if !mgr.is_tunnel_alive(res.info.id) {
                break;
            }
            thread::sleep(Duration::from_millis(15));
        }
        let snap = mgr.list_tunnels_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].status, TunnelStatus::Disconnected);
        assert_eq!(snap[0].pid, 0);

        mgr.set_test_command("sleep", &["60"]);
        let blocker = TcpListener::bind(("127.0.0.1", snap[0].local_port)).ok();
        let reconnected = mgr.reconnect_tunnel(res.info.id).unwrap();
        assert_eq!(reconnected.id, res.info.id);
        assert_eq!(reconnected.status, TunnelStatus::Active);
        assert!(pid_alive(reconnected.pid));
        if blocker.is_some() {
            assert_ne!(reconnected.local_port, snap[0].local_port);
        }
        drop(blocker);

        assert!(mgr.stop_tunnel(res.info.id));
        assert!(mgr.list_tunnels_snapshot().is_empty());
    }
}
