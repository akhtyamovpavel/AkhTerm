//! Atomic JSON persistence (`0600`) for AkhTerm Remote settings alongside `config.conf`.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::sys::json_lite::{format_pretty, parse_json, JsonVal};

pub const DEFAULT_REMOTE_BIND: &str = "127.0.0.1";
pub const DEFAULT_REMOTE_PORT: u16 = 7474;
pub const MIN_REMOTE_PORT: u16 = 1024;
pub const MAX_REMOTE_PORT: u16 = 65535;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteSettings {
    pub enabled: bool,
    pub bind_ip: String,
    pub port: u16,
    pub mute_when_focused: bool,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        default_remote_settings()
    }
}

pub fn default_remote_settings() -> RemoteSettings {
    RemoteSettings {
        enabled: false,
        bind_ip: DEFAULT_REMOTE_BIND.to_owned(),
        port: DEFAULT_REMOTE_PORT,
        mute_when_focused: true,
    }
}

impl RemoteSettings {
    pub fn normalized(&self) -> Self {
        normalize_remote_settings(self)
    }
}

fn normalize_remote_settings(s: &RemoteSettings) -> RemoteSettings {
    let trimmed = s.bind_ip.trim();
    let bind_ip = if trimmed.is_empty() { DEFAULT_REMOTE_BIND } else { trimmed };
    RemoteSettings {
        enabled: s.enabled,
        bind_ip: bind_ip.to_owned(),
        port: s.port.clamp(MIN_REMOTE_PORT, MAX_REMOTE_PORT),
        mute_when_focused: s.mute_when_focused,
    }
}

pub fn default_remote_config_path() -> PathBuf {
    crate::sys::paths::config_path().with_file_name("remote.json")
}

fn parse_remote_json(val: &JsonVal) -> RemoteSettings {
    let enabled = val.get("enabled").and_then(JsonVal::as_bool).unwrap_or(false);
    let bind_ip = val.get("bind_ip").and_then(JsonVal::as_str).unwrap_or(DEFAULT_REMOTE_BIND).to_owned();
    let port = val.get("port").and_then(JsonVal::as_u64).map_or(DEFAULT_REMOTE_PORT, |p| p as u16);
    let mute_when_focused = val.get("mute_when_focused").and_then(JsonVal::as_bool).unwrap_or(true);
    RemoteSettings { enabled, bind_ip, port, mute_when_focused }.normalized()
}

pub fn load_remote_settings_from(path: &Path) -> RemoteSettings {
    let Ok(raw) = fs::read_to_string(path) else { return RemoteSettings::default() };
    parse_json(&raw).map_or_else(|_| RemoteSettings::default(), |v| parse_remote_json(&v))
}

pub fn load_remote_settings() -> RemoteSettings {
    load_remote_settings_from(&default_remote_config_path())
}

fn serialize_remote_settings(settings: &RemoteSettings) -> String {
    let norm = settings.normalized();
    let val = JsonVal::Object(vec![
        ("enabled".into(), JsonVal::Bool(norm.enabled)),
        ("bind_ip".into(), JsonVal::Str(norm.bind_ip)),
        ("port".into(), JsonVal::Number(norm.port.to_string())),
        ("mute_when_focused".into(), JsonVal::Bool(norm.mute_when_focused)),
    ]);
    format!("{}\n", format_pretty(&val))
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub fn save_remote_settings_to(path: &Path, settings: &RemoteSettings) -> io::Result<()> {
    ensure_parent_dir(path)?;
    let content = serialize_remote_settings(settings);
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&tmp)?;
    if let Err(err) = file.write_all(content.as_bytes()).and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

pub fn save_remote_settings(settings: &RemoteSettings) -> io::Result<()> {
    save_remote_settings_to(&default_remote_config_path(), settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_settings_round_trip_and_default_disabled() {
        let path = std::env::temp_dir().join(format!("akhterm-remote-cfg-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        assert!(!load_remote_settings_from(&path).enabled);
        let cfg = RemoteSettings { enabled: true, bind_ip: "10.0.0.2".into(), port: 8484, mute_when_focused: false };
        save_remote_settings_to(&path, &cfg).unwrap();
        assert_eq!(load_remote_settings_from(&path), cfg);
        let _ = fs::remove_file(&path);
    }
}
