//! Device pairing, SHA-256 token persistence (0600), rate limiting, and bind-address discovery.

use std::ffi::{c_char, c_int, CStr};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::remote::sha256::{ct_eq, random_hex, sha256_hex};
use crate::sys::json_lite::{format_pretty, parse_json, JsonVal};

pub const PAIRING_TTL_SECS: u64 = 120;
pub const MAX_PAIR_FAILURES: u32 = 5;
pub const PAIR_LOCKOUT_SECS: u64 = 60;
const MAX_DEVICES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedDevice {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub token_hash: String,
    pub created_unix: u64,
    pub last_seen_unix: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindCandidate {
    pub iface: String,
    pub ip: String,
    pub is_vpn: bool,
}

impl BindCandidate {
    pub fn display_label(&self) -> String {
        if self.ip == "127.0.0.1" {
            return "127.0.0.1 (localhost)".to_owned();
        }
        format!("{} ({})", self.ip, self.iface)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairError {
    NoActiveCode,
    Expired,
    InvalidCode,
    RateLimited,
    Internal(String),
}

#[derive(Clone, Debug)]
struct PairingSession {
    code: String,
    expires_at: Instant,
}

pub struct RemoteAuthStore {
    store_path: PathBuf,
    devices: Vec<PairedDevice>,
    pairing: Option<PairingSession>,
    fail_count: u32,
    locked_until: Option<Instant>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn sanitize_label(raw: &str, fallback: &str, max_len: usize) -> String {
    let clean: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(max_len)
        .collect();
    let trimmed = clean.trim();
    if trimmed.is_empty() { fallback.to_owned() } else { trimmed.to_owned() }
}

impl RemoteAuthStore {
    pub fn load(store_path: PathBuf) -> Self {
        let devices = read_devices_file(&store_path).unwrap_or_default();
        Self { store_path, devices, pairing: None, fail_count: 0, locked_until: None }
    }

    pub fn devices(&self) -> &[PairedDevice] {
        &self.devices
    }

    pub fn active_pairing_code(&self, now: Instant) -> Option<(&str, u64)> {
        let session = self.pairing.as_ref()?;
        if now >= session.expires_at {
            return None;
        }
        let rem = session.expires_at.saturating_duration_since(now).as_secs().max(1);
        Some((session.code.as_str(), rem))
    }

    pub fn start_pairing(&mut self, now: Instant) -> io::Result<String> {
        let code = random_hex(16)?;
        self.pairing = Some(PairingSession {
            code: code.clone(),
            expires_at: now + Duration::from_secs(PAIRING_TTL_SECS),
        });
        self.fail_count = 0;
        self.locked_until = None;
        Ok(code)
    }

    pub fn complete_pairing(
        &mut self,
        code: &str,
        device_name: &str,
        platform: &str,
        now: Instant,
    ) -> Result<(PairedDevice, String), PairError> {
        if self.locked_until.is_some_and(|until| now < until) {
            return Err(PairError::RateLimited);
        }
        self.verify_pairing_code(code, now)?;
        self.pairing = None;
        self.fail_count = 0;
        self.locked_until = None;
        self.mint_device(device_name, platform)
    }

    fn verify_pairing_code(&mut self, code: &str, now: Instant) -> Result<(), PairError> {
        let Some(session) = self.pairing.as_ref() else {
            return Err(PairError::NoActiveCode);
        };
        if now >= session.expires_at {
            self.pairing = None;
            return Err(PairError::Expired);
        }
        let trimmed = code.trim().to_ascii_lowercase();
        if ct_eq(trimmed.as_bytes(), session.code.as_bytes()) {
            return Ok(());
        }
        self.record_pair_failure(now)
    }

    fn record_pair_failure(&mut self, now: Instant) -> Result<(), PairError> {
        self.fail_count += 1;
        if self.fail_count >= MAX_PAIR_FAILURES {
            self.pairing = None;
            self.fail_count = 0;
            self.locked_until = Some(now + Duration::from_secs(PAIR_LOCKOUT_SECS));
            return Err(PairError::RateLimited);
        }
        Err(PairError::InvalidCode)
    }

    fn mint_device(&mut self, device_name: &str, platform: &str) -> Result<(PairedDevice, String), PairError> {
        let raw_token = random_hex(32).map_err(|e| PairError::Internal(e.to_string()))?;
        let id_hex = random_hex(6).map_err(|e| PairError::Internal(e.to_string()))?;
        let now_u = unix_now();
        let dev = PairedDevice {
            device_id: format!("dev-{id_hex}"),
            device_name: sanitize_label(device_name, "Mobile Device", 64),
            platform: sanitize_label(platform, "android", 24).to_ascii_lowercase(),
            token_hash: sha256_hex(raw_token.as_bytes()),
            created_unix: now_u,
            last_seen_unix: now_u,
        };
        if self.devices.len() >= MAX_DEVICES {
            self.devices.remove(0);
        }
        self.devices.push(dev.clone());
        let _ = write_devices_atomic(&self.store_path, &self.devices);
        Ok((dev, raw_token))
    }

    pub fn verify_token(&mut self, raw_token: &str) -> Option<String> {
        let trimmed = raw_token.trim();
        if trimmed.len() < 32 {
            return None;
        }
        let digest = sha256_hex(trimmed.as_bytes());
        let found = self
            .devices
            .iter_mut()
            .find(|d| ct_eq(d.token_hash.as_bytes(), digest.as_bytes()))?;
        found.last_seen_unix = unix_now();
        Some(found.device_id.clone())
    }

    pub fn revoke_device(&mut self, device_id: &str) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.device_id != device_id);
        let removed = self.devices.len() != before;
        if removed {
            let _ = write_devices_atomic(&self.store_path, &self.devices);
        }
        removed
    }

    pub fn revoke_by_token(&mut self, raw_token: &str) -> bool {
        let digest = sha256_hex(raw_token.trim().as_bytes());
        let before = self.devices.len();
        self.devices.retain(|d| !ct_eq(d.token_hash.as_bytes(), digest.as_bytes()));
        let removed = self.devices.len() != before;
        if removed {
            let _ = write_devices_atomic(&self.store_path, &self.devices);
        }
        removed
    }
}

fn device_to_json(d: &PairedDevice) -> JsonVal {
    JsonVal::Object(vec![
        ("device_id".into(), JsonVal::Str(d.device_id.clone())),
        ("device_name".into(), JsonVal::Str(d.device_name.clone())),
        ("platform".into(), JsonVal::Str(d.platform.clone())),
        ("token_hash".into(), JsonVal::Str(d.token_hash.clone())),
        ("created_unix".into(), JsonVal::Number(d.created_unix.to_string())),
        ("last_seen_unix".into(), JsonVal::Number(d.last_seen_unix.to_string())),
    ])
}

fn device_from_json(v: &JsonVal) -> Option<PairedDevice> {
    Some(PairedDevice {
        device_id: v.get("device_id")?.as_str()?.to_owned(),
        device_name: v.get("device_name")?.as_str()?.to_owned(),
        platform: v.get("platform").and_then(JsonVal::as_str).unwrap_or("android").to_owned(),
        token_hash: v.get("token_hash")?.as_str()?.to_owned(),
        created_unix: v.get("created_unix").and_then(JsonVal::as_u64).unwrap_or(0),
        last_seen_unix: v.get("last_seen_unix").and_then(JsonVal::as_u64).unwrap_or(0),
    })
}

fn read_devices_file(path: &Path) -> io::Result<Vec<PairedDevice>> {
    let raw = fs::read_to_string(path)?;
    let val = parse_json(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let items = val.get("devices").and_then(JsonVal::as_array).unwrap_or(&[]);
    Ok(items.iter().filter_map(device_from_json).collect())
}

fn write_devices_atomic(path: &Path, devices: &[PairedDevice]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let root = JsonVal::Object(vec![
        ("version".into(), JsonVal::Number("1".into())),
        ("devices".into(), JsonVal::Array(devices.iter().map(device_to_json).collect())),
    ]);
    let content = format!("{}\n", format_pretty(&root));
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&tmp)?;
    if let Err(err) = file.write_all(content.as_bytes()).and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

pub fn default_devices_path() -> PathBuf {
    crate::sys::paths::state_dir().join("remote-devices.json")
}

pub fn local_hostname() -> String {
    if let Ok(raw) = fs::read_to_string("/etc/hostname") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    std::env::var("HOSTNAME").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "akhterm-host".to_owned())
}

#[repr(C)]
struct IfAddrs {
    ifa_next: *mut IfAddrs,
    ifa_name: *mut c_char,
    ifa_flags: u32,
    ifa_addr: *mut SockAddr,
}

#[repr(C)]
struct SockAddr {
    sa_family: u16,
    sa_data: [u8; 14],
}

const AF_INET: u16 = 2;
const IFF_UP: u32 = 1;

extern "C" {
    fn getifaddrs(ifap: *mut *mut IfAddrs) -> c_int;
    fn freeifaddrs(ifa: *mut IfAddrs);
}

fn is_vpn_iface(name: &str) -> bool {
    name.starts_with("wg") || name.starts_with("tailscale") || name.starts_with("tun") || name.starts_with("zt")
}

fn is_usable_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified() && !ip.is_link_local() && !ip.is_multicast()
}

fn read_ifaddr_ipv4(node: &IfAddrs) -> Option<BindCandidate> {
    if (node.ifa_flags & IFF_UP) == 0 || node.ifa_addr.is_null() || node.ifa_name.is_null() {
        return None;
    }
    let sa = unsafe { &*node.ifa_addr };
    if sa.sa_family != AF_INET {
        return None;
    }
    let ip = Ipv4Addr::new(sa.sa_data[2], sa.sa_data[3], sa.sa_data[4], sa.sa_data[5]);
    if !is_usable_ipv4(ip) {
        return None;
    }
    let iface = unsafe { CStr::from_ptr(node.ifa_name) }.to_string_lossy().into_owned();
    Some(BindCandidate { is_vpn: is_vpn_iface(&iface), iface, ip: ip.to_string() })
}

pub fn detect_bind_candidates() -> Vec<BindCandidate> {
    let mut head: *mut IfAddrs = std::ptr::null_mut();
    let mut out = Vec::new();
    if unsafe { getifaddrs(&mut head) } == 0 {
        let mut cur = head;
        while !cur.is_null() {
            let node = unsafe { &*cur };
            if let Some(cand) = read_ifaddr_ipv4(node) {
                if !out.iter().any(|c: &BindCandidate| c.ip == cand.ip) {
                    out.push(cand);
                }
            }
            cur = node.ifa_next;
        }
        unsafe { freeifaddrs(head) };
    }
    if !out.iter().any(|c| c.ip == "127.0.0.1") {
        out.push(BindCandidate { iface: "lo".into(), ip: "127.0.0.1".into(), is_vpn: false });
    }
    out.sort_by_key(|c| if c.is_vpn { 0 } else if c.ip != "127.0.0.1" { 1 } else { 2 });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("akhterm-remote-auth-{tag}-{}.json", std::process::id()))
    }

    #[test]
    fn pairing_flow_persists_hashed_token_and_revokes() {
        let path = temp_store("flow");
        let _ = fs::remove_file(&path);
        let now = Instant::now();
        let mut store = RemoteAuthStore::load(path.clone());
        let code = store.start_pairing(now).unwrap();
        let (dev, token) = store.complete_pairing(&code, "Pixel 9", "android", now).unwrap();
        assert_eq!(store.verify_token(&token).as_deref(), Some(dev.device_id.as_str()));
        assert_eq!(store.complete_pairing(&code, "Replay", "android", now), Err(PairError::NoActiveCode));

        let raw_file = fs::read_to_string(&path).unwrap();
        assert!(!raw_file.contains(&token), "raw token must never be stored on disk");
        assert!(raw_file.contains(&dev.token_hash));

        let mut reloaded = RemoteAuthStore::load(path.clone());
        assert_eq!(reloaded.verify_token(&token).as_deref(), Some(dev.device_id.as_str()));
        assert!(reloaded.revoke_device(&dev.device_id));
        assert!(reloaded.verify_token(&token).is_none());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn pairing_enforces_expiry_and_rate_limit() {
        let path = temp_store("rate");
        let _ = fs::remove_file(&path);
        let now = Instant::now();
        let mut store = RemoteAuthStore::load(path.clone());
        let code = store.start_pairing(now).unwrap();
        let expired = now + Duration::from_secs(PAIRING_TTL_SECS + 1);
        assert_eq!(store.complete_pairing(&code, "Phone", "android", expired), Err(PairError::Expired));

        let code2 = store.start_pairing(now).unwrap();
        for _ in 0..(MAX_PAIR_FAILURES - 1) {
            assert_eq!(store.complete_pairing("deadbeef", "Bad", "android", now), Err(PairError::InvalidCode));
        }
        assert_eq!(store.complete_pairing("deadbeef", "Bad", "android", now), Err(PairError::RateLimited));
        assert_eq!(store.complete_pairing(&code2, "Phone", "android", now), Err(PairError::RateLimited));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn bind_candidates_always_include_loopback() {
        let cands = detect_bind_candidates();
        assert!(cands.iter().any(|c| c.ip == "127.0.0.1"));
    }
}
