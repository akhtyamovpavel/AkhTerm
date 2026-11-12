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
pub const DEFAULT_SPLIT_RATIO: u16 = 500;
pub const MIN_SPLIT_RATIO: u16 = 100;
pub const MAX_SPLIT_RATIO: u16 = 900;
pub const DEFAULT_FONT_SCALE_PERMILLE: u16 = 1000;
pub const MIN_FONT_SCALE: f64 = 0.5;
pub const MAX_FONT_SCALE: f64 = 3.0;

pub fn clamp_split_ratio(ratio: u16) -> u16 {
    ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)
}

pub fn clamp_font_scale(scale: f64) -> f64 {
    if scale.is_finite() {
        scale.clamp(MIN_FONT_SCALE, MAX_FONT_SCALE)
    } else {
        1.0
    }
}

pub fn font_scale_to_permille(scale: f64) -> u16 {
    (clamp_font_scale(scale) * 1000.0).round() as u16
}

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
pub enum SplitLeaf {
    Local(String),
    Remote(RemoteConnection),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitTree {
    Primary,
    Pane(String),
    RemotePane(RemoteConnection),
    Split {
        orientation: SplitOrientation,
        ratio_permille: u16,
        first: Box<SplitTree>,
        second: Box<SplitTree>,
    },
}

impl SplitTree {
    #[cfg(test)]
    pub fn split(orientation: SplitOrientation, first: Self, second: Self) -> Self {
        Self::split_with_ratio(orientation, DEFAULT_SPLIT_RATIO, first, second)
    }

    pub fn split_with_ratio(
        orientation: SplitOrientation,
        ratio_permille: u16,
        first: Self,
        second: Self,
    ) -> Self {
        Self::Split {
            orientation,
            ratio_permille: clamp_split_ratio(ratio_permille),
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    #[cfg(test)]
    pub fn secondary_cwds(&self, out: &mut Vec<String>) {
        match self {
            Self::Primary | Self::RemotePane(_) => {}
            Self::Pane(cwd) => out.push(cwd.clone()),
            Self::Split { first, second, .. } => {
                first.secondary_cwds(out);
                second.secondary_cwds(out);
            }
        }
    }

    pub fn secondary_leaves(&self, out: &mut Vec<SplitLeaf>) {
        match self {
            Self::Primary => {}
            Self::Pane(cwd) => out.push(SplitLeaf::Local(cwd.clone())),
            Self::RemotePane(remote) => out.push(SplitLeaf::Remote(remote.clone())),
            Self::Split { first, second, .. } => {
                first.secondary_leaves(out);
                second.secondary_leaves(out);
            }
        }
    }

    fn counts(&self) -> (usize, usize) {
        match self {
            Self::Primary => (1, 0),
            Self::Pane(_) | Self::RemotePane(_) => (0, 1),
            Self::Split { first, second, .. } => {
                let (p1, s1) = first.counts();
                let (p2, s2) = second.counts();
                (p1 + p2, s1 + s2)
            }
        }
    }

    fn matches_tab_kind(&self, tab: &Tab) -> bool {
        match (self, tab) {
            (Self::Primary, _) => true,
            (Self::Pane(_), Tab::Local { .. }) => true,
            (Self::RemotePane(conn), Tab::Remote(_)) => is_ssh_host(&conn.host),
            (Self::Split { first, second, .. }, _) => {
                first.matches_tab_kind(tab) && second.matches_tab_kind(tab)
            }
            _ => false,
        }
    }

    pub fn is_valid(&self) -> bool {
        let (primaries, extras) = self.counts();
        matches!(self, Self::Split { .. })
            && primaries == 1
            && (1..=MAX_EXTRA_PANES).contains(&extras)
    }

    pub fn is_valid_for(&self, tab: &Tab) -> bool {
        self.is_valid() && self.matches_tab_kind(tab)
    }

    fn sanitized(&self) -> Self {
        match self {
            Self::Primary => Self::Primary,
            Self::Pane(cwd) => Self::Pane(cwd.replace('\0', "")),
            Self::RemotePane(conn) => Self::RemotePane(conn.sanitized()),
            Self::Split { orientation, ratio_permille, first, second } => {
                Self::split_with_ratio(
                    *orientation,
                    *ratio_permille,
                    first.sanitized(),
                    second.sanitized(),
                )
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
            Self::RemotePane(conn) => conn.push_split_tokens(out),
            Self::Split { orientation, ratio_permille, first, second } => {
                out.push(orientation.as_tag().to_owned());
                out.push(clamp_split_ratio(*ratio_permille).to_string());
                first.push_tokens(out);
                second.push_tokens(out);
            }
        }
    }

    fn decode_leaf<'a>(
        tag: &str,
        fields: &mut impl Iterator<Item = &'a str>,
        v2: bool,
    ) -> Option<Option<Self>> {
        match tag {
            "P" => Some(Some(Self::Primary)),
            "L" => Some(fields.next().map(|s| Self::Pane(s.to_owned()))),
            "R" if v2 => Some(RemoteConnection::decode_split_tokens(fields).map(Self::RemotePane)),
            _ => None,
        }
    }

    fn decode_tokens<'a>(
        fields: &mut impl Iterator<Item = &'a str>,
        depth: usize,
        v2: bool,
    ) -> Option<Self> {
        if depth > MAX_SPLIT_DEPTH {
            return None;
        }
        let tag = fields.next()?;
        if let Some(leaf) = Self::decode_leaf(tag, fields, v2) {
            return leaf;
        }
        let orientation = SplitOrientation::from_tag(tag)?;
        let ratio = if v2 { fields.next()?.parse().ok()? } else { DEFAULT_SPLIT_RATIO };
        let first = Self::decode_tokens(fields, depth + 1, v2)?;
        let second = Self::decode_tokens(fields, depth + 1, v2)?;
        Some(Self::split_with_ratio(orientation, ratio, first, second))
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

fn ssh_base_argv(host: &str) -> Vec<String> {
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
    argv.push(host.to_owned());
    argv
}

impl RemoteConnection {
    pub fn sanitized(&self) -> Self {
        let tmux_session = self
            .tmux_session
            .as_deref()
            .map(|s| s.replace('\0', ""))
            .filter(|s| !s.is_empty());
        Self { host: self.host.replace('\0', ""), tmux_session }
    }

    pub fn argv(&self) -> Vec<String> {
        let mut argv = ssh_base_argv(&self.host);
        if let Some(name) = &self.tmux_session {
            let target = shell_quote(name);
            argv.push(format!("tmux new-session -A -s {target}"));
        }
        argv
    }

    pub fn grouped_tmux_argv(&self, pane_seq: usize) -> Vec<String> {
        let mut argv = ssh_base_argv(&self.host);
        if let Some(base) = &self.tmux_session {
            let pane_name = format!("{base}-pane-{pane_seq}");
            let q_pane = shell_quote(&pane_name);
            let q_base = shell_quote(base);
            argv.push(format!(
                "tmux new-session -A -s {q_pane} -t {q_base} \\; set-option destroy-unattached on"
            ));
        }
        argv
    }

    pub fn split_argv(&self, pane_seq: usize) -> Vec<String> {
        if self.tmux_session.is_some() {
            self.grouped_tmux_argv(pane_seq)
        } else {
            self.argv()
        }
    }

    fn push_split_tokens(&self, out: &mut Vec<String>) {
        out.push("R".to_owned());
        out.push(self.host.replace('\0', ""));
        out.push(self.tmux_session.as_deref().unwrap_or("").replace('\0', ""));
    }

    fn decode_split_tokens<'a>(fields: &mut impl Iterator<Item = &'a str>) -> Option<Self> {
        let host = fields.next()?;
        let session = fields.next()?;
        if !is_ssh_host(host) {
            return None;
        }
        Some(Self {
            host: host.to_owned(),
            tmux_session: (!session.is_empty()).then(|| session.to_owned()),
        })
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
    let keep_splits = entry.splits.as_ref().is_some_and(|tree| tree.is_valid_for(tab));
    entry.splits = if keep_splits {
        entry.splits.as_ref().map(SplitTree::sanitized)
    } else {
        None
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabsState {
    pub active: usize,
    pub tabs: Vec<Tab>,
    pub meta: Vec<TabMeta>,
    pub font_scale_permille: u16,
}

impl Default for TabsState {
    fn default() -> Self {
        Self {
            active: 0,
            tabs: Vec::new(),
            meta: Vec::new(),
            font_scale_permille: DEFAULT_FONT_SCALE_PERMILLE,
        }
    }
}

impl TabsState {
    pub fn new(active: usize, tabs: Vec<Tab>) -> Self {
        Self { active, tabs, meta: Vec::new(), font_scale_permille: DEFAULT_FONT_SCALE_PERMILLE }
    }

    pub fn with_meta(active: usize, tabs: Vec<Tab>, mut meta: Vec<TabMeta>) -> Self {
        meta.truncate(tabs.len());
        for (idx, entry) in meta.iter_mut().enumerate() {
            normalize_entry(&tabs[idx], entry);
        }
        while meta.last().is_some_and(TabMeta::is_empty) {
            meta.pop();
        }
        Self { active, tabs, meta, font_scale_permille: DEFAULT_FONT_SCALE_PERMILLE }
    }

    pub fn with_font_scale(mut self, scale: f64) -> Self {
        self.font_scale_permille = font_scale_to_permille(scale);
        self
    }

    pub fn font_scale(&self) -> f64 {
        clamp_font_scale(f64::from(self.font_scale_permille) / 1000.0)
    }

    pub fn pinned_at(&self, index: usize) -> Option<&str> {
        self.meta.get(index)?.pinned_title.as_deref()
    }

    pub fn splits_at(&self, index: usize) -> Option<&SplitTree> {
        self.meta.get(index)?.splits.as_ref()
    }

    pub fn encode(&self) -> String {
        // NUL delimiters preserve tabs, newlines and quotes in paths and tmux names.
        let mut owned = vec!["akhterm-tabs-v2".to_owned(), self.active.to_string()];
        for tab in &self.tabs {
            push_tab_tokens(tab, &mut owned);
        }
        if self.font_scale_permille != DEFAULT_FONT_SCALE_PERMILLE {
            let clamped = font_scale_to_permille(self.font_scale());
            owned.push("meta:scale".to_owned());
            owned.push(clamped.to_string());
        }
        for (idx, entry) in self.meta.iter().enumerate().take(self.tabs.len()) {
            push_meta_tokens(idx, &self.tabs[idx], entry, &mut owned);
        }
        owned.push(String::new());
        owned.join("\0")
    }

    pub fn decode(data: &str) -> Option<Self> {
        let mut fields = data.strip_suffix('\0')?.split('\0');
        let v2 = match fields.next()? {
            "akhterm-tabs-v2" => true,
            "akhterm-tabs-v1" => false,
            _ => return None,
        };
        let active: usize = fields.next()?.parse().ok()?;
        let (tabs, meta, scale) = decode_body(&mut fields, v2)?;
        if active >= tabs.len().max(1) {
            return None;
        }
        let mut state = Self::with_meta(active, tabs, meta);
        state.font_scale_permille = scale;
        Some(state)
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
    if let Some(tree) = &entry.splits {
        if tree.is_valid_for(tab) {
            out.push("meta:split".to_owned());
            out.push(idx_str);
            tree.push_tokens(out);
        }
    }
}

fn is_meta_tag(key: &str, v2: bool) -> bool {
    key == "meta:pinned" || key == "meta:split" || (v2 && key == "meta:scale")
}

fn decode_body<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
    v2: bool,
) -> Option<(Vec<Tab>, Vec<TabMeta>, u16)> {
    let mut tabs = Vec::new();
    let mut meta = Vec::new();
    let mut scale = DEFAULT_FONT_SCALE_PERMILLE;
    let mut seen_meta = false;
    while let Some(key) = fields.next() {
        if is_meta_tag(key, v2) {
            seen_meta = true;
            meta.resize_with(meta.len().max(tabs.len()), TabMeta::default);
            decode_meta_entry(key, &tabs, &mut meta, &mut scale, fields, v2)?;
        } else if !seen_meta {
            tabs.push(decode_tab(key, fields)?);
        } else {
            return None;
        }
    }
    Some((tabs, meta, scale))
}

fn decode_scale_meta<'a>(fields: &mut impl Iterator<Item = &'a str>) -> Option<u16> {
    let raw: u16 = fields.next()?.parse().ok()?;
    Some(font_scale_to_permille(f64::from(raw) / 1000.0))
}

fn decode_meta_entry<'a>(
    key: &str,
    tabs: &[Tab],
    meta: &mut [TabMeta],
    scale: &mut u16,
    fields: &mut impl Iterator<Item = &'a str>,
    v2: bool,
) -> Option<()> {
    if key == "meta:scale" {
        *scale = decode_scale_meta(fields)?;
        return Some(());
    }
    let idx: usize = fields.next()?.parse().ok()?;
    let tab = tabs.get(idx)?;
    if key == "meta:pinned" {
        let title = fields.next().filter(|t| !t.is_empty())?;
        meta[idx].pinned_title = Some(title.to_owned());
        return Some(());
    }
    if !v2 && matches!(tab, Tab::Remote(_)) {
        return None;
    }
    let tree = SplitTree::decode_tokens(fields, 0, v2)?;
    tree.is_valid_for(tab).then(|| meta[idx].splits = Some(tree))
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

fn read_or_log(path: &Path) -> Option<TabsState> {
    match read_state(path) {
        Ok(saved) => saved,
        Err(error) => {
            eprintln!("AkhTerm: cannot read {}: {error}", path.display());
            None
        }
    }
}

fn migrate_v1_to_v2(v1_path: &Path, v2_path: &Path) -> (Option<TabsState>, Option<TabsState>) {
    let Some(migrated) = read_or_log(v1_path) else {
        return (None, None);
    };
    let written = write_state_file(v2_path, &migrated.encode()).is_ok().then(|| migrated.clone());
    (Some(migrated), written)
}

impl TabStore {
    pub fn load() -> (Self, Option<TabsState>) {
        Self::load_from_dir(&state_dir())
    }

    pub fn load_from_dir(dir: &Path) -> (Self, Option<TabsState>) {
        let v2_path = dir.join("tabs-v2");
        let v1_path = dir.join("tabs-v1");
        let (saved, last_written) = match read_state(&v2_path) {
            Ok(Some(state)) => (Some(state.clone()), Some(state)),
            Ok(None) => migrate_v1_to_v2(&v1_path, &v2_path),
            Err(error) => {
                eprintln!("AkhTerm: cannot read {}: {error}", v2_path.display());
                (None, None)
            }
        };
        let store = Self { path: v2_path, last_written, retry_at: Instant::now() };
        (store, saved)
    }

    #[cfg(test)]
    pub fn new_for_test(path: PathBuf) -> Self {
        Self { path, last_written: None, retry_at: Instant::now() }
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
        assert_eq!(decoded.font_scale(), 1.0);
        assert_eq!(
            decoded.encode(),
            "akhterm-tabs-v2\x001\x00pinned\x00/tmp/a\x00split\x00/tmp/b\x00ssh\x00gpu\x00main\x00"
        );
    }

    #[test]
    fn empty_workspace_round_trips_byte_for_byte() {
        let state = TabsState::new(0, Vec::new());
        let encoded = state.encode();
        assert_eq!(encoded, "akhterm-tabs-v2\x000\x00");
        assert_eq!(TabsState::decode(&encoded), Some(state));
    }

    #[test]
    fn closed_tab_stays_closed_and_empty_saved_workspace_stays_empty() {
        let dir = std::env::temp_dir().join(format!("akhterm_store_test_{}", std::process::id()));
        let path = dir.join("tabs-v2");
        let mut store = TabStore { path: path.clone(), last_written: None, retry_at: Instant::now() };
        let t1 = Tab::Local { profile: "shell".into(), cwd: "/tmp/one".into() };
        let t2 = Tab::Local { profile: "shell".into(), cwd: "/tmp/two".into() };
        store.checkpoint(TabsState::with_meta(1, vec![t1.clone(), t2], vec![TabMeta { pinned_title: Some("Один".into()), splits: None }]));
        store.checkpoint(TabsState::with_meta(0, vec![t1.clone()], vec![TabMeta { pinned_title: Some("Один".into()), splits: None }]));
        let after_one_close = read_state(&path).unwrap().unwrap();
        assert_eq!(after_one_close.tabs, vec![t1]);
        assert_eq!(after_one_close.pinned_at(0), Some("Один"));

        store.checkpoint(TabsState::with_meta(0, Vec::new(), Vec::new()));
        assert_eq!(fs::read_to_string(&path).unwrap(), "akhterm-tabs-v2\x000\x00");
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
    fn remote_grouped_tmux_and_plain_ssh_split_argv() {
        let plain = RemoteConnection { host: "devbox".into(), tmux_session: None };
        let plain_argv = plain.split_argv(2);
        assert_eq!(plain_argv.last().map(String::as_str), Some("devbox"));

        let tmux = RemoteConnection { host: "gpu01".into(), tmux_session: Some("work 'main'".into()) };
        let grouped = tmux.grouped_tmux_argv(3);
        assert_eq!(grouped[0], "ssh");
        assert!(grouped.contains(&"-t".to_owned()));
        assert_eq!(grouped[grouped.len() - 2], "gpu01");
        assert_eq!(
            grouped[grouped.len() - 1],
            "tmux new-session -A -s 'work '\\''main'\\''-pane-3' -t 'work '\\''main'\\''' \\; set-option destroy-unattached on"
        );
    }

    fn sample_remote_split() -> SplitTree {
        let conn = RemoteConnection { host: "gpu".into(), tmux_session: Some("dev".into()) };
        SplitTree::split_with_ratio(
            SplitOrientation::Vertical, 340, SplitTree::Primary, SplitTree::RemotePane(conn),
        )
    }

    fn verify_migrated_v2_round_trip(dir: &Path, mut state: TabsState) {
        let r_tree = sample_remote_split();
        state.meta.resize_with(2, TabMeta::default);
        state.meta[1].splits = Some(r_tree.clone());
        let (mut store, _) = TabStore::load_from_dir(dir);
        store.checkpoint(state.with_font_scale(1.35));
        let (_, reloaded) = TabStore::load_from_dir(dir);
        let reloaded = reloaded.expect("reloads v2");
        assert_eq!(reloaded.splits_at(1), Some(&r_tree));
        assert!((reloaded.font_scale() - 1.35).abs() < 0.002);
    }

    #[test]
    fn tabs_v1_migrates_to_tabs_v2_preserving_splits_and_remote_ratios_round_trip() {
        let dir = std::env::temp_dir().join(format!("akhterm_v2_migrate_{}", std::process::id()));
        let (v1_file, v2_file) = (dir.join("tabs-v1"), dir.join("tabs-v2"));
        let v1_payload = "akhterm-tabs-v1\x000\x00shell\x00/tmp/a\x00ssh\x00gpu\x00dev\x00meta:pinned\x000\x00Основа\x00meta:split\x000\x00H\x00P\x00L\x00/tmp/b\x00";
        write_state_file(&v1_file, v1_payload).unwrap();
        let (_, loaded) = TabStore::load_from_dir(&dir);
        let state = loaded.expect("migrates v1");
        assert_eq!(state.pinned_at(0), Some("Основа"));
        assert_eq!(fs::metadata(&v2_file).unwrap().permissions().mode() & 0o777, 0o600);
        verify_migrated_v2_round_trip(&dir, state);
        let _ = fs::remove_file(&v1_file);
        let _ = fs::remove_file(&v2_file);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn tabs_module_maintains_grade_b_mi_score() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("tabs.rs"));
        assert!(mi >= 60.83, "tabs.rs MI {mi:.2} < 60.83");
    }
}
