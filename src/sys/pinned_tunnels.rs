//! Atomic `0600` XDG-state persistence for pinned SSH tunnels (`pinned_tunnels_v1.json`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::paths;
use crate::sys::json_lite::{format_pretty, parse_json, JsonVal};

const PINNED_FILE_NAME: &str = "pinned_tunnels_v1.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedTunnel {
    pub host: String,
    pub remote_port: u16,
    pub local_port: u16,
    pub scheme: String,
    pub process: String,
}

pub fn pinned_tunnels_path() -> PathBuf {
    paths::state_dir().join(PINNED_FILE_NAME)
}

fn normalize_scheme(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("https") { "https".to_owned() } else { "http".to_owned() }
}

fn entry_to_json(item: &PinnedTunnel) -> JsonVal {
    JsonVal::Object(vec![
        ("host".to_owned(), JsonVal::Str(item.host.clone())),
        ("remote_port".to_owned(), JsonVal::Number(item.remote_port.to_string())),
        ("local_port".to_owned(), JsonVal::Number(item.local_port.to_string())),
        ("scheme".to_owned(), JsonVal::Str(normalize_scheme(&item.scheme))),
        ("process".to_owned(), JsonVal::Str(item.process.clone())),
    ])
}

fn json_to_entry(val: &JsonVal) -> Option<PinnedTunnel> {
    let host = val.get("host")?.as_str()?.trim();
    let remote_port = u16::try_from(val.get("remote_port")?.as_u64()?).ok()?;
    if host.is_empty() || remote_port == 0 {
        return None;
    }
    let local_port = val
        .get("local_port")
        .and_then(JsonVal::as_u64)
        .and_then(|v| u16::try_from(v).ok())
        .unwrap_or(remote_port);
    let scheme = normalize_scheme(val.get("scheme").and_then(JsonVal::as_str).unwrap_or("http"));
    let process = val.get("process").and_then(JsonVal::as_str).unwrap_or("").to_owned();
    Some(PinnedTunnel { host: host.to_owned(), remote_port, local_port, scheme, process })
}

pub fn load_pinned_from(path: &Path) -> Vec<PinnedTunnel> {
    let Ok(raw) = fs::read_to_string(path) else { return Vec::new() };
    let Ok(root) = parse_json(&raw) else { return Vec::new() };
    let Some(arr) = root.get("tunnels").and_then(JsonVal::as_array) else { return Vec::new() };
    let mut out = Vec::new();
    for item in arr.iter().filter_map(json_to_entry) {
        if !out.iter().any(|p: &PinnedTunnel| p.host == item.host && p.remote_port == item.remote_port) {
            out.push(item);
        }
    }
    out
}

pub fn save_pinned_to(path: &Path, items: &[PinnedTunnel]) -> io::Result<()> {
    let payload = JsonVal::Object(vec![
        ("version".to_owned(), JsonVal::Number("1".to_owned())),
        ("tunnels".to_owned(), JsonVal::Array(items.iter().map(entry_to_json).collect())),
    ]);
    paths::write_state_file(path, &format!("{}\n", format_pretty(&payload)))
}

pub fn pin_tunnel_at(path: &Path, mut entry: PinnedTunnel) -> io::Result<Vec<PinnedTunnel>> {
    entry.host = entry.host.trim().to_owned();
    if entry.host.is_empty() || entry.remote_port == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid tunnel target"));
    }
    entry.scheme = normalize_scheme(&entry.scheme);
    let mut list = load_pinned_from(path);
    if let Some(slot) = list.iter_mut().find(|p| p.host == entry.host && p.remote_port == entry.remote_port) {
        *slot = entry;
    } else {
        list.push(entry);
    }
    save_pinned_to(path, &list)?;
    Ok(list)
}

pub fn unpin_tunnel_at(path: &Path, host: &str, remote_port: u16) -> io::Result<bool> {
    let mut list = load_pinned_from(path);
    let before = list.len();
    list.retain(|p| !(p.host == host && p.remote_port == remote_port));
    if list.len() == before {
        return Ok(false);
    }
    save_pinned_to(path, &list)?;
    Ok(true)
}

pub fn pin_tunnel(entry: PinnedTunnel) -> io::Result<Vec<PinnedTunnel>> {
    pin_tunnel_at(&pinned_tunnels_path(), entry)
}

pub fn unpin_tunnel(host: &str, remote_port: u16) -> io::Result<bool> {
    unpin_tunnel_at(&pinned_tunnels_path(), host, remote_port)
}

pub fn list_pinned_tunnels() -> Vec<PinnedTunnel> {
    load_pinned_from(&pinned_tunnels_path())
}

pub fn list_pinned_for_host(host: &str) -> Vec<PinnedTunnel> {
    list_pinned_tunnels().into_iter().filter(|p| p.host == host).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    static XDG_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn sample(host: &str, remote_port: u16, local_port: u16, process: &str) -> PinnedTunnel {
        PinnedTunnel {
            host: host.to_owned(),
            remote_port,
            local_port,
            scheme: "http".to_owned(),
            process: process.to_owned(),
        }
    }

    #[test]
    fn persists_pinned_tunnels_atomically_with_0600_in_xdg_state_home() {
        let _guard = XDG_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("akhterm_pin_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let prev = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", &root) };

        let pinned = pin_tunnel(sample("gpu01", 8000, 18000, "uvicorn")).unwrap();
        assert_eq!(pinned.len(), 1);
        pin_tunnel(sample("gpu01", 5173, 5173, "vite")).unwrap();
        pin_tunnel(sample("gpu02", 8000, 8000, "node")).unwrap();
        pin_tunnel(sample("gpu01", 8000, 18001, "fastapi")).unwrap();

        let path = pinned_tunnels_path();
        assert_eq!(path, root.join("akhterm").join("pinned_tunnels_v1.json"));
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let gpu01 = list_pinned_for_host("gpu01");
        assert_eq!(gpu01.len(), 2);
        assert_eq!(gpu01[0].local_port, 18001);
        assert_eq!(gpu01[0].process, "fastapi");
        assert_eq!(list_pinned_for_host("gpu02").len(), 1);

        assert!(unpin_tunnel("gpu01", 5173).unwrap());
        assert!(!unpin_tunnel("gpu01", 5173).unwrap());
        assert_eq!(list_pinned_for_host("gpu01").len(), 1);

        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tolerates_missing_or_corrupt_pinned_tunnels_file() {
        let dir = std::env::temp_dir().join(format!("akhterm_pin_corrupt_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pinned_tunnels_v1.json");
        assert!(load_pinned_from(&path).is_empty());
        fs::write(&path, "{broken json").unwrap();
        assert!(load_pinned_from(&path).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
