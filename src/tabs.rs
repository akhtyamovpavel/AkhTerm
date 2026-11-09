//! Launch presets and the workspace saved between AkhTerm runs.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::ffi::flock;
use crate::paths::{state_dir, write_state_file};
use crate::proc_radar::shell_quote;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteConnection {
    pub host: String,
    pub tmux_session: Option<String>,
}

impl RemoteConnection {
    pub fn argv(&self) -> Vec<String> {
        let mut argv = [
            "ssh",
            "-t",
            "-oConnectTimeout=10",
            "-oServerAliveInterval=15",
            "-oServerAliveCountMax=3",
            "-oControlPath=none",
            "--",
        ]
        .map(str::to_owned)
        .to_vec();
        argv.push(self.host.clone());
        if let Some(name) = &self.tmux_session {
            let target = shell_quote(name);
            argv.push(format!("tmux new-session -A -s {target}"));
        }
        argv
    }

    pub fn title(&self) -> String {
        match &self.tmux_session {
            Some(name) => format!("{} · tmux:{name}", self.host),
            None => format!("ssh {}", self.host),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tab {
    Local { profile: String, cwd: String },
    Remote(RemoteConnection),
}

impl Tab {
    pub fn local(profile: &str) -> Self {
        Self::Local {
            profile: profile.to_owned(),
            cwd: String::new(),
        }
    }

    pub fn remote(host: &str, tmux_session: Option<&str>) -> Self {
        Self::Remote(RemoteConnection {
            host: host.to_owned(),
            tmux_session: tmux_session.map(str::to_owned),
        })
    }
}

pub fn is_ssh_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && !host.chars().any(|c| c.is_whitespace() || c.is_control())
}

pub fn is_profile_key(key: &str) -> bool {
    !key.is_empty() && key != "ssh" && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabsState {
    pub active: usize,
    pub tabs: Vec<Tab>,
}

impl TabsState {
    fn encode(&self) -> String {
        // NUL delimiters preserve tabs, newlines and quotes in paths and tmux names.
        let active = self.active.to_string();
        let mut fields = vec!["akhterm-tabs-v1", &active];
        for tab in &self.tabs {
            match tab {
                Tab::Local { profile, cwd } => fields.extend([profile.as_str(), cwd]),
                Tab::Remote(remote) => {
                    let target = remote.tmux_session.as_deref().unwrap_or("");
                    fields.extend(["ssh", &remote.host, target]);
                }
            }
        }
        fields.push("");
        fields.join("\0")
    }

    fn decode(data: &str) -> Option<Self> {
        let mut fields = data.strip_suffix('\0')?.split('\0');
        if fields.next()? != "akhterm-tabs-v1" {
            return None;
        }
        let active: usize = fields.next()?.parse().ok()?;
        let mut tabs = Vec::new();
        while let Some(key) = fields.next() {
            tabs.push(decode_tab(key, &mut fields)?);
        }
        if active >= tabs.len().max(1) {
            return None;
        }
        Some(Self { active, tabs })
    }
}

fn decode_tab<'a>(key: &str, fields: &mut impl Iterator<Item = &'a str>) -> Option<Tab> {
    let value = fields.next()?;
    if key == "ssh" {
        let name = fields.next()?;
        if !is_ssh_host(value) {
            return None;
        }
        return Some(Tab::remote(
            value,
            Some(name).filter(|name| !name.is_empty()),
        ));
    }
    if !is_profile_key(key) {
        return None;
    }
    Some(Tab::Local {
        profile: key.to_owned(),
        cwd: value.to_owned(),
    })
}

pub struct TabStore {
    path: PathBuf,
    last_written: Option<TabsState>,
    retry_at: Instant,
}

pub fn lock_workspace() -> io::Result<File> {
    let directory = state_dir();
    fs::create_dir_all(&directory)?;
    let path = directory.join("workspace.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)?;
    const LOCK_EXCLUSIVE: i32 = 2;
    const LOCK_NONBLOCKING: i32 = 4;
    if unsafe { flock(file.as_raw_fd(), LOCK_EXCLUSIVE | LOCK_NONBLOCKING) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

impl TabStore {
    pub fn load() -> (Self, Option<TabsState>) {
        let path = state_dir().join("tabs-v1");
        let saved = match read_state(&path) {
            Ok(saved) => saved,
            Err(error) => {
                eprintln!("AkhTerm: cannot read {}: {error}", path.display());
                None
            }
        };
        let store = Self {
            path,
            last_written: saved.clone(),
            retry_at: Instant::now(),
        };
        (store, saved)
    }

    pub fn checkpoint(&mut self, state: TabsState) {
        if self.last_written.as_ref() == Some(&state) || Instant::now() < self.retry_at {
            return;
        }
        match write_state_file(&self.path, &state.encode()) {
            Ok(()) => self.last_written = Some(state),
            Err(error) => {
                eprintln!("AkhTerm: cannot save {}: {error}", self.path.display());
                self.retry_at = Instant::now() + Duration::from_secs(15);
            }
        }
    }
}

fn read_state(path: &Path) -> io::Result<Option<TabsState>> {
    match fs::read_to_string(path) {
        Ok(data) => TabsState::decode(&data).map(Some).ok_or_else(invalid_state),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn invalid_state() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid tab state")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_order_active_tab_and_special_characters() {
        let local = Tab::Local {
            profile: "workspace".to_owned(),
            cwd: "/work/Папка с пробелом\tи\nпереносом".to_owned(),
        };
        let tabs = vec![
            local,
            Tab::remote("devbox", None),
            Tab::remote("buildbox", Some("work 'queue'")),
        ];
        let state = TabsState { active: 2, tabs };
        assert_eq!(TabsState::decode(&state.encode()), Some(state));
    }

    #[test]
    fn empty_workspace_round_trips() {
        let state = TabsState {
            active: 0,
            tabs: Vec::new(),
        };
        assert_eq!(TabsState::decode(&state.encode()), Some(state));
    }

    #[test]
    fn rejects_incomplete_or_invalid_workspaces() {
        assert_eq!(
            TabsState::decode("akhterm-tabs-v1\x000\x00ssh\x00devbox\x00"),
            None
        );
        assert_eq!(TabsState::decode("akhterm-tabs-v1\x001\x00"), None);
        assert_eq!(
            TabsState::decode("akhterm-tabs-v1\x000\x00invalid profile\x00/\x00"),
            None
        );
    }
}
