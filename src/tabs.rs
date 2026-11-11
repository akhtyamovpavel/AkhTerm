//! Launch presets and the workspace saved between AkhTerm runs.

use std::ffi::c_int;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::ffi::{flock, GTK_ORIENTATION_HORIZONTAL, GTK_ORIENTATION_VERTICAL};
use crate::paths::{state_dir, write_state_file};
use crate::proc_radar::shell_quote;

const MAX_SPLIT_DEPTH: usize = 24;
const MAX_EXTRA_PANES: usize = 31;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitOrientation {
    Horizontal,
    Vertical,
}

impl SplitOrientation {
    pub fn from_gtk(orient: c_int) -> Self {
        if orient == GTK_ORIENTATION_VERTICAL {
            Self::Vertical
        } else {
            Self::Horizontal
        }
    }

    pub fn to_gtk(self) -> c_int {
        match self {
            Self::Horizontal => GTK_ORIENTATION_HORIZONTAL,
            Self::Vertical => GTK_ORIENTATION_VERTICAL,
        }
    }

    fn as_tag(self) -> &'static str {
        match self {
            Self::Horizontal => "H",
            Self::Vertical => "V",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "H" => Some(Self::Horizontal),
            "V" => Some(Self::Vertical),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitTree {
    Primary,
    Pane(String),
    Split {
        orientation: SplitOrientation,
        first: Box<SplitTree>,
        second: Box<SplitTree>,
    },
}

impl SplitTree {
    pub fn split(orientation: SplitOrientation, first: Self, second: Self) -> Self {
        Self::Split {
            orientation,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    pub fn secondary_cwds(&self, out: &mut Vec<String>) {
        match self {
            Self::Primary => {}
            Self::Pane(cwd) => out.push(cwd.clone()),
            Self::Split { first, second, .. } => {
                first.secondary_cwds(out);
                second.secondary_cwds(out);
            }
        }
    }

    fn counts(&self) -> (usize, usize) {
        match self {
            Self::Primary => (1, 0),
            Self::Pane(_) => (0, 1),
            Self::Split { first, second, .. } => {
                let (p1, s1) = first.counts();
                let (p2, s2) = second.counts();
                (p1 + p2, s1 + s2)
            }
        }
    }

    pub fn is_valid(&self) -> bool {
        let (primaries, extras) = self.counts();
        matches!(self, Self::Split { .. }) && primaries == 1 && (1..=MAX_EXTRA_PANES).contains(&extras)
    }

    fn sanitized(&self) -> Self {
        match self {
            Self::Primary => Self::Primary,
            Self::Pane(cwd) => Self::Pane(cwd.replace('\0', "")),
            Self::Split { orientation, first, second } => {
                Self::split(*orientation, first.sanitized(), second.sanitized())
            }
        }
    }

    fn push_tokens(&self, out: &mut Vec<String>) {
        match self {
            Self::Primary => out.push("P".to_owned()),
            Self::Pane(cwd) => {
                out.push("L".to_owned());
                out.push(cwd.replace('\0', ""));
            }
            Self::Split { orientation, first, second } => {
                out.push(orientation.as_tag().to_owned());
                first.push_tokens(out);
                second.push_tokens(out);
            }
        }
    }

    fn decode_tokens<'a>(fields: &mut impl Iterator<Item = &'a str>, depth: usize) -> Option<Self> {
        if depth > MAX_SPLIT_DEPTH {
            return None;
        }
        let tag = fields.next()?;
        if tag == "P" {
            return Some(Self::Primary);
        }
        if tag == "L" {
            return Some(Self::Pane(fields.next()?.to_owned()));
        }
        let orientation = SplitOrientation::from_tag(tag)?;
        let first = Self::decode_tokens(fields, depth + 1)?;
        let second = Self::decode_tokens(fields, depth + 1)?;
        Some(Self::split(orientation, first, second))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabMeta {
    pub pinned_title: Option<String>,
    pub splits: Option<SplitTree>,
}

impl TabMeta {
    pub fn is_empty(&self) -> bool {
        self.pinned_title.is_none() && self.splits.is_none()
    }
}

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

fn sanitize_meta_title(raw: &str) -> Option<String> {
    let clean = raw.replace('\0', "");
    (!clean.is_empty()).then_some(clean)
}

fn normalize_entry(tab: &Tab, entry: &mut TabMeta) {
    entry.pinned_title = entry.pinned_title.as_deref().and_then(sanitize_meta_title);
    let keep_splits = matches!(tab, Tab::Local { .. })
        && entry.splits.as_ref().is_some_and(SplitTree::is_valid);
    entry.splits = if keep_splits {
        entry.splits.as_ref().map(SplitTree::sanitized)
    } else {
        None
    };
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabsState {
    pub active: usize,
    pub tabs: Vec<Tab>,
    pub meta: Vec<TabMeta>,
}

impl TabsState {
    pub fn new(active: usize, tabs: Vec<Tab>) -> Self {
        Self { active, tabs, meta: Vec::new() }
    }

    pub fn with_meta(active: usize, tabs: Vec<Tab>, mut meta: Vec<TabMeta>) -> Self {
        meta.truncate(tabs.len());
        for (idx, entry) in meta.iter_mut().enumerate() {
            normalize_entry(&tabs[idx], entry);
        }
        while meta.last().is_some_and(TabMeta::is_empty) {
            meta.pop();
        }
        Self { active, tabs, meta }
    }

    pub fn pinned_at(&self, index: usize) -> Option<&str> {
        self.meta.get(index)?.pinned_title.as_deref()
    }

    pub fn splits_at(&self, index: usize) -> Option<&SplitTree> {
        self.meta.get(index)?.splits.as_ref()
    }

    pub fn encode(&self) -> String {
        // NUL delimiters preserve tabs, newlines and quotes in paths and tmux names.
        let mut owned = vec!["akhterm-tabs-v1".to_owned(), self.active.to_string()];
        for tab in &self.tabs {
            push_tab_tokens(tab, &mut owned);
        }
        for (idx, entry) in self.meta.iter().enumerate().take(self.tabs.len()) {
            push_meta_tokens(idx, &self.tabs[idx], entry, &mut owned);
        }
        owned.push(String::new());
        owned.join("\0")
    }

    pub fn decode(data: &str) -> Option<Self> {
        let mut fields = data.strip_suffix('\0')?.split('\0');
        if fields.next()? != "akhterm-tabs-v1" {
            return None;
        }
        let active: usize = fields.next()?.parse().ok()?;
        let (tabs, meta) = decode_body(&mut fields)?;
        if active >= tabs.len().max(1) {
            return None;
        }
        Some(Self::with_meta(active, tabs, meta))
    }
}

fn push_tab_tokens(tab: &Tab, out: &mut Vec<String>) {
    match tab {
        Tab::Local { profile, cwd } => {
            out.push(profile.clone());
            out.push(cwd.replace('\0', ""));
        }
        Tab::Remote(remote) => {
            let target = remote.tmux_session.as_deref().unwrap_or("");
            out.push("ssh".to_owned());
            out.push(remote.host.clone());
            out.push(target.replace('\0', ""));
        }
    }
}

fn push_meta_tokens(idx: usize, tab: &Tab, entry: &TabMeta, out: &mut Vec<String>) {
    let idx_str = idx.to_string();
    if let Some(title) = entry.pinned_title.as_deref().and_then(sanitize_meta_title) {
        out.push("meta:pinned".to_owned());
        out.push(idx_str.clone());
        out.push(title);
    }
    if let (Tab::Local { .. }, Some(tree)) = (tab, &entry.splits) {
        if tree.is_valid() {
            out.push("meta:split".to_owned());
            out.push(idx_str);
            tree.push_tokens(out);
        }
    }
}

fn decode_body<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
) -> Option<(Vec<Tab>, Vec<TabMeta>)> {
    let mut tabs = Vec::new();
    let mut meta = Vec::new();
    let mut seen_meta = false;
    while let Some(key) = fields.next() {
        if key == "meta:pinned" || key == "meta:split" {
            seen_meta = true;
            if meta.len() < tabs.len() {
                meta.resize_with(tabs.len(), TabMeta::default);
            }
            decode_meta_entry(key, &tabs, &mut meta, fields)?;
        } else if !seen_meta {
            tabs.push(decode_tab(key, fields)?);
        } else {
            return None;
        }
    }
    Some((tabs, meta))
}

fn decode_meta_entry<'a>(
    key: &str,
    tabs: &[Tab],
    meta: &mut [TabMeta],
    fields: &mut impl Iterator<Item = &'a str>,
) -> Option<()> {
    let idx: usize = fields.next()?.parse().ok()?;
    let tab = tabs.get(idx)?;
    if key == "meta:pinned" {
        let title = fields.next()?;
        if title.is_empty() {
            return None;
        }
        meta[idx].pinned_title = Some(title.to_owned());
        return Some(());
    }
    if matches!(tab, Tab::Remote(_)) {
        return None;
    }
    let tree = SplitTree::decode_tokens(fields, 0)?;
    if !tree.is_valid() {
        return None;
    }
    meta[idx].splits = Some(tree);
    Some(())
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
    use std::os::unix::fs::PermissionsExt;

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
        let state = TabsState::new(2, tabs);
        assert_eq!(TabsState::decode(&state.encode()), Some(state));
    }

    #[test]
    fn legacy_tabs_v1_payload_and_reserved_profile_names_load_cleanly() {
        let legacy = "akhterm-tabs-v1\x001\x00pinned\x00/tmp/a\x00split\x00/tmp/b\x00ssh\x00gpu\x00main\x00";
        let decoded = TabsState::decode(legacy).expect("legacy tabs-v1 loads");
        assert_eq!(decoded.active, 1);
        assert_eq!(decoded.tabs.len(), 3);
        assert!(decoded.meta.is_empty());
        assert_eq!(decoded.encode(), legacy);
    }

    #[test]
    fn empty_workspace_round_trips_byte_for_byte() {
        let state = TabsState::new(0, Vec::new());
        let encoded = state.encode();
        assert_eq!(encoded, "akhterm-tabs-v1\x000\x00");
        assert_eq!(TabsState::decode(&encoded), Some(state));
    }

    #[test]
    fn closed_tab_stays_closed_and_empty_saved_workspace_stays_empty() {
        let dir = std::env::temp_dir().join(format!("akhterm_store_test_{}", std::process::id()));
        let path = dir.join("tabs-v1");
        let mut store = TabStore { path: path.clone(), last_written: None, retry_at: Instant::now() };
        let t1 = Tab::Local { profile: "shell".into(), cwd: "/tmp/one".into() };
        let t2 = Tab::Local { profile: "shell".into(), cwd: "/tmp/two".into() };
        store.checkpoint(TabsState::with_meta(1, vec![t1.clone(), t2], vec![TabMeta { pinned_title: Some("Один".into()), splits: None }]));
        store.checkpoint(TabsState::with_meta(0, vec![t1.clone()], vec![TabMeta { pinned_title: Some("Один".into()), splits: None }]));
        let after_one_close = read_state(&path).unwrap().unwrap();
        assert_eq!(after_one_close.tabs, vec![t1]);
        assert_eq!(after_one_close.pinned_at(0), Some("Один"));

        store.checkpoint(TabsState::with_meta(0, Vec::new(), Vec::new()));
        assert_eq!(fs::read_to_string(&path).unwrap(), "akhterm-tabs-v1\x000\x00");
        assert_eq!(read_state(&path).unwrap(), Some(TabsState::new(0, Vec::new())));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn split_tree_and_pinned_titles_round_trip_with_atomic_0600_write() {
        let tree = SplitTree::split(
            SplitOrientation::Horizontal,
            SplitTree::Pane("/tmp/левая панель".to_owned()),
            SplitTree::split(
                SplitOrientation::Vertical,
                SplitTree::Primary,
                SplitTree::Pane("/tmp/правая\tнижняя\nпапка".to_owned()),
            ),
        );
        let tabs = vec![
            Tab::Local { profile: "shell".into(), cwd: "/tmp/основная".into() },
            Tab::remote("prod", Some("deploy")),
        ];
        let meta = vec![
            TabMeta { pinned_title: Some("Бэкенд · ЦУ".into()), splits: Some(tree.clone()) },
            TabMeta { pinned_title: Some("Прод деплой".into()), splits: None },
        ];
        let state = TabsState::with_meta(1, tabs, meta);
        let dir = std::env::temp_dir().join(format!("akhterm_tabs_test_{}", std::process::id()));
        let file = dir.join("tabs-v1");
        write_state_file(&file, &state.encode()).unwrap();
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let loaded = read_state(&file).unwrap();
        let _ = fs::remove_file(&file);
        let _ = fs::remove_dir(&dir);
        assert_eq!(loaded.as_ref(), Some(&state));
        assert_eq!(state.pinned_at(0), Some("Бэкенд · ЦУ"));
        assert_eq!(state.splits_at(0), Some(&tree));
    }

    #[test]
    fn rejects_incomplete_or_invalid_workspaces() {
        assert_eq!(TabsState::decode("akhterm-tabs-v1\x000\x00ssh\x00devbox\x00"), None);
        assert_eq!(TabsState::decode("akhterm-tabs-v1\x001\x00"), None);
        assert_eq!(TabsState::decode("akhterm-tabs-v1\x000\x00invalid profile\x00/\x00"), None);
        assert_eq!(TabsState::decode("akhterm-tabs-v1\x000\x00meta:pinned\x000\x00x\x00"), None);
        assert_eq!(
            TabsState::decode("akhterm-tabs-v1\x000\x00ssh\x00box\x00\x00meta:split\x000\x00H\x00P\x00L\x00/tmp\x00"),
            None
        );
        assert_eq!(
            TabsState::decode("akhterm-tabs-v1\x000\x00shell\x00/tmp\x00meta:split\x000\x00H\x00L\x00/a\x00L\x00/b\x00"),
            None
        );
    }

    #[test]
    fn tabs_module_maintains_grade_b_mi_score() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("tabs.rs"));
        assert!(mi >= 58.0, "tabs.rs MI {mi:.2} < 58.0");
    }
}
