//! Incremental O(1) agent token/context telemetry, VTE status-bar fallback, and turn timer UI.
#[path = "jsonl_parse.rs"]
mod jsonl_parse;
#[path = "tokens_ui.rs"]
mod tokens_ui;
#[path = "transcript_owner.rs"]
mod transcript_owner;

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::approval::read_vte_visible_text;
use crate::ffi::*;
#[cfg(test)]
use crate::session::{SessionCategory, TermSession};
#[path = "usage.rs"]
pub mod usage;
#[path = "opencode.rs"]
pub mod opencode;
pub use crate::telemetry_agy::parse_vte_context_panel;
use crate::telemetry_agy::{
    discover_agy_by_fd, discover_agy_transcript, is_vte_breakdown_line, metadata_stamp,
    trim_trailing_slash,
};
pub use jsonl_parse::apply_jsonl_line;
pub(crate) use jsonl_parse::{
    is_complete_json_object, json_direct_value, json_obj, json_obj_prefix, json_str, json_u64,
};
pub use tokens_ui::*;

pub const VTE_FALLBACK_LINES: usize = 8;
pub const WARN_CONTEXT_TOKENS: u64 = 120_000;
pub const HIGH_CONTEXT_TOKENS: u64 = 180_000;
pub const WARN_CONTEXT_PCT: u8 = 75;
pub const HIGH_CONTEXT_PCT: u8 = 90;
const DISCOVERY_TTL: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TelemetrySource {
    #[default]
    None,
    Agy,
    Claude,
    Codex,
    OpenCode,
    VteFallback,
}

impl TelemetrySource {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Agy => "AGY",
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::VteFallback => "TUI",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContextAlert {
    #[default]
    Normal,
    Warn,
    High,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenSnapshot {
    pub source: TelemetrySource,
    pub input_tokens: u64,
    pub cache_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub context_tokens: u64,
    pub context_pct: Option<u8>,
    pub requests: crate::telemetry::usage::RequestHistory,
    pub cost_usd: Option<f64>,
}

impl TokenSnapshot {
    pub fn has_data(&self) -> bool {
        let non_zero = self.context_tokens.saturating_add(self.total_tokens()) > 0;
        non_zero || self.context_pct.is_some() || self.cost_usd.is_some()
    }

    pub fn effective_input(&self) -> u64 {
        self.input_tokens.saturating_add(self.cache_write_tokens)
    }

    pub fn total_tokens(&self) -> u64 {
        self.effective_input()
            .saturating_add(self.cache_tokens)
            .saturating_add(self.output_tokens)
    }

    pub fn compact_context_badge(&self) -> Option<String> {
        if self.context_tokens > 0 {
            Some(format!("ctx {}", format_compact_tokens(self.context_tokens)))
        } else {
            self.context_pct.map(|pct| format!("ctx {pct}%"))
        }
    }

    pub fn usage_badge(&self) -> Option<String> {
        self.requests.badge(self.compact_context_badge())
    }

    pub fn alert_level(&self) -> ContextAlert {
        let pct = self.context_pct.unwrap_or(0);
        if self.context_tokens >= HIGH_CONTEXT_TOKENS || pct >= HIGH_CONTEXT_PCT {
            return ContextAlert::High;
        }
        if self.context_tokens >= WARN_CONTEXT_TOKENS || pct >= WARN_CONTEXT_PCT {
            return ContextAlert::Warn;
        }
        ContextAlert::Normal
    }
}

#[derive(Clone, Debug, Default)]
pub struct TranscriptTailer {
    pub path: Option<PathBuf>,
    pub kind: TelemetrySource,
    pub offset: u64,
    pub mtime_len: (u64, u64),
    pub snapshot: TokenSnapshot,
    last_msg_id: String,
    last_msg_usage: (u64, u64, u64, u64),
}

impl TranscriptTailer {
    pub fn reset_for(&mut self, path: PathBuf, kind: TelemetrySource) {
        self.path = Some(path);
        self.kind = kind;
        self.mtime_len = (0, 0);
        self.reset_after_truncate();
    }

    pub fn reset_after_truncate(&mut self) {
        self.offset = 0;
        self.snapshot = TokenSnapshot { source: self.kind, ..Default::default() };
        self.last_msg_id.clear();
        self.last_msg_usage = (0, 0, 0, 0);
    }

    pub fn poll_file(&mut self, path: &Path, kind: TelemetrySource) -> io::Result<bool> {
        if self.path.as_deref() != Some(path) || self.kind != kind {
            self.reset_for(path.to_path_buf(), kind);
        }
        let meta = fs::metadata(path)?;
        let len = meta.len();
        let mtime = metadata_stamp(&meta);
        if len < self.offset {
            self.reset_after_truncate();
        }
        if len == self.offset {
            self.mtime_len = (mtime, len);
            return Ok(false);
        }
        let changed = self.read_appended_lines(path)?;
        self.mtime_len = (mtime, self.offset);
        Ok(changed)
    }

    fn read_appended_lines(&mut self, path: &Path) -> io::Result<bool> {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(file);
        let start_offset = self.offset;
        let mut buf = Vec::with_capacity(1024);
        while read_next_complete_line(&mut reader, &mut buf)? {
            let line = String::from_utf8_lossy(&buf);
            self.consume_line(&line);
            self.offset = self.offset.saturating_add(buf.len() as u64);
        }
        Ok(self.offset != start_offset)
    }

    pub fn consume_line(&mut self, line: &str) {
        apply_jsonl_line(
            self.kind,
            line,
            &mut self.snapshot,
            &mut self.last_msg_id,
            &mut self.last_msg_usage,
        );
    }
}

fn read_next_complete_line(reader: &mut BufReader<File>, buf: &mut Vec<u8>) -> io::Result<bool> {
    buf.clear();
    if reader.read_until(b'\n', buf)? == 0 {
        return Ok(false);
    }
    if buf.ends_with(b"\n") {
        return Ok(true);
    }
    Ok(is_complete_json_object(&String::from_utf8_lossy(buf)))
}

#[derive(Clone, Debug, Default)]
pub struct SessionTelemetry {
    pub tailer: TranscriptTailer,
    pub remote_tailer: crate::remote_telemetry::RemoteTailer,
    pub vte_fallback: TokenSnapshot,
    pub vte_panel_override: Option<(u64, Option<u8>, u64)>,
    pub last_tmux_window: Option<String>,
    previous_screen: Vec<String>,
    effective: TokenSnapshot,
    last_discovery: Option<Instant>,
}

impl SessionTelemetry {
    pub fn snapshot(&self) -> &TokenSnapshot {
        if self.vte_panel_override.is_some() {
            &self.effective
        } else {
            self.active_snapshot()
        }
    }

    pub fn active_snapshot(&self) -> &TokenSnapshot {
        if self.tailer.snapshot.has_data() {
            &self.tailer.snapshot
        } else if self.remote_tailer.snapshot.has_data() {
            &self.remote_tailer.snapshot
        } else {
            &self.vte_fallback
        }
    }

    pub fn active_jsonl_offset(&self) -> u64 {
        if self.tailer.path.is_some() || self.tailer.snapshot.has_data() {
            self.tailer.offset
        } else if !self.remote_tailer.remote_path.is_empty() || self.remote_tailer.snapshot.has_data() {
            self.remote_tailer.remote_offset
        } else {
            0
        }
    }

    pub fn has_jsonl_data(&self) -> bool {
        self.tailer.snapshot.has_data() || self.remote_tailer.snapshot.has_data()
    }

    pub fn effective_snapshot(&self) -> TokenSnapshot {
        let mut snap = self.active_snapshot().clone();
        if let Some((toks, pct, captured_offset)) = self.vte_panel_override {
            if self.active_jsonl_offset() == captured_offset {
                apply_panel_override(&mut snap, toks, pct);
            }
        }
        snap
    }

    pub fn sync_effective(&mut self) {
        self.effective = self.effective_snapshot();
    }

    pub fn reset_transport(&mut self, vte: *mut GtkWidget) {
        *self = Self::default();
        self.previous_screen = read_vte_visible_text(vte).lines().map(|line| line.trim().to_owned()).collect();
    }

    pub fn observe_vte_text(&mut self, visible: &str) {
        if !self.previous_screen.is_empty() {
            let current: std::collections::HashSet<_> = visible.lines().map(str::trim).collect();
            self.previous_screen.retain(|old| current.contains(old.as_str()));
            let fresh: Vec<_> = visible.lines().filter(|line| {
                !self.previous_screen.iter().any(|old| old == line.trim())
            }).collect();
            self.observe_fresh_text(&fresh.join("\n"));
        } else {
            self.observe_fresh_text(visible);
        }
    }

    fn observe_fresh_text(&mut self, visible: &str) {
        if let Some((toks, pct)) = parse_vte_context_panel(visible) {
            self.record_panel_hit(toks, pct);
        } else {
            self.vte_fallback = TokenSnapshot::default();
            self.record_status_fallback(visible);
        }
        self.sync_effective();
    }

    fn record_panel_hit(&mut self, toks: u64, pct: Option<u8>) {
        self.vte_panel_override = Some((toks, pct, self.active_jsonl_offset()));
        if !self.has_jsonl_data() {
            self.vte_fallback = TokenSnapshot {
                source: TelemetrySource::VteFallback,
                context_tokens: toks,
                context_pct: pct,
                ..Default::default()
            };
        }
    }

    fn record_status_fallback(&mut self, visible: &str) {
        if !self.has_jsonl_data() {
            if let Some(parsed) = parse_vte_status_fallback(visible) {
                self.vte_fallback = parsed;
                self.vte_panel_override = None;
            }
        }
    }

    pub fn detected_agent_label(&self) -> Option<&'static str> {
        match self.snapshot().source {
            TelemetrySource::Agy | TelemetrySource::Claude | TelemetrySource::Codex | TelemetrySource::OpenCode => {
                Some(self.snapshot().source.label())
            }
            TelemetrySource::None | TelemetrySource::VteFallback => None,
        }
    }

    pub fn transcript_key(&self) -> Option<String> {
        if self.tailer.snapshot.has_data() {
            if let Some(path) = &self.tailer.path {
                if self.tailer.kind == TelemetrySource::OpenCode {
                    return Some(format!("local:opencode:{}", path.file_name()?.to_str()?));
                }
                let canon = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                return Some(format!("local:{}", canon.display()));
            }
        }
        let remote = &self.remote_tailer;
        if remote.snapshot.has_data() && !remote.remote_path.is_empty() {
            let name = if remote.kind == TelemetrySource::OpenCode {
                Path::new(&remote.remote_path).file_name()?.to_str()?
            } else {
                &remote.remote_path
            };
            return Some(format!("remote:{}:{}", remote.host, name));
        }
        None
    }

    fn should_rediscover(&self, now: Instant) -> bool {
        if self.tailer.path.as_ref().is_none_or(|p| !p.is_file()) {
            return true;
        }
        self.last_discovery
            .is_none_or(|prev| now.saturating_duration_since(prev) >= DISCOVERY_TTL)
    }
}

fn apply_panel_override(snap: &mut TokenSnapshot, toks: u64, pct: Option<u8>) {
    if snap.source == TelemetrySource::None {
        snap.source = TelemetrySource::VteFallback;
    }
    snap.context_tokens = toks;
    if pct.is_some() {
        snap.context_pct = pct;
    }
}

pub struct SessionPollJob {
    pub sid: usize,
    pub root_pid: i32,
    pub vte: *mut GtkWidget,
    pub local_cwd: Option<String>,
    pub has_ssh: bool,
    pub has_tmux: bool,
    pub tmux_window: Option<String>,
    pub agent: String,
    pub is_daemon: bool,
    pub telemetry: SessionTelemetry,
}

#[cfg(test)]
fn session_root_pid(s: &TermSession, active_vte: *mut GtkWidget) -> i32 {
    if s.proc_info.active_pane_pid > 0 && active_vte == s.vte {
        s.proc_info.active_pane_pid
    } else {
        s.active_pid()
    }
}

#[cfg(test)]
fn session_tmux_window(s: &TermSession) -> Option<String> {
    let proc_win = (!s.proc_info.tmux_window.is_empty()).then(|| s.proc_info.tmux_window.clone());
    if !s.proc_info.ssh_host.is_empty() {
        s.remote_tmux.as_ref().map(|t| t.1.clone()).or(proc_win)
    } else {
        proc_win
    }
}

impl SessionPollJob {
    #[cfg(test)]
    pub fn from_session(s: &TermSession) -> Self {
        let active_vte = s.active_vte();
        Self {
            sid: s.id,
            root_pid: session_root_pid(s, active_vte),
            vte: active_vte,
            local_cwd: s.active_local_cwd(),
            has_ssh: !s.proc_info.ssh_host.is_empty(),
            has_tmux: !s.proc_info.has_tmux.is_empty(),
            tmux_window: session_tmux_window(s),
            agent: s.effective_agent(),
            is_daemon: s.category == SessionCategory::Daemon,
            telemetry: s.telemetry.clone(),
        }
    }
}

fn has_live_ssh(job: &SessionPollJob) -> bool {
    job.has_ssh
        || (job.root_pid > 0
            && !crate::proc_radar::inspect_proc_tree(job.root_pid).ssh_host.is_empty())
}

pub fn should_poll_vte_fallback(
    local_cwd: Option<&str>,
    has_tmux: bool,
    agent: &str,
) -> bool {
    local_cwd.is_none() || has_tmux || !agent.is_empty()
}

fn reset_local_tailer(telem: &mut SessionTelemetry) {
    if telem.tailer.path.is_some() || telem.tailer.snapshot.has_data() {
        telem.tailer = TranscriptTailer::default();
        telem.vte_panel_override = None;
        telem.last_discovery = None;
    }
}

fn reset_remote_tailer(telem: &mut SessionTelemetry) {
    if !telem.remote_tailer.remote_path.is_empty() || telem.remote_tailer.snapshot.has_data() {
        telem.remote_tailer = Default::default();
        telem.vte_panel_override = None;
    }
}

fn apply_remote_tailer(telem: &mut SessionTelemetry, rt: crate::remote_telemetry::RemoteTailer) {
    let prev_path = telem.remote_tailer.remote_path.clone();
    telem.remote_tailer = rt;
    if let Some((_, _, captured_offset)) = telem.vte_panel_override {
        let switched = !prev_path.is_empty() && telem.remote_tailer.remote_path != prev_path;
        if telem.remote_tailer.remote_offset != captured_offset || switched {
            telem.vte_panel_override = None;
        }
    }
}

#[cfg(test)]
pub fn run_poll_job(job: SessionPollJob, home: &Path, now: Instant) -> (usize, SessionTelemetry) {
    run_poll_job_with_remote(job, None, home, now)
}

fn sync_tmux_window_change(telem: &mut SessionTelemetry, tmux_window: &Option<String>) {
    if &telem.last_tmux_window != tmux_window {
        reset_local_tailer(telem);
        reset_remote_tailer(telem);
        telem.vte_fallback = TokenSnapshot::default();
        telem.vte_panel_override = None;
        telem.last_tmux_window = tmux_window.clone();
    }
}

pub fn run_poll_job_with_remote(
    mut job: SessionPollJob,
    remote_tailer: Option<crate::remote_telemetry::RemoteTailer>,
    home: &Path,
    now: Instant,
) -> (usize, SessionTelemetry) {
    if job.is_daemon {
        return (job.sid, job.telemetry);
    }
    if remote_tailer.as_ref().is_some_and(|r| r.foreground_shell) {
        let telemetry = SessionTelemetry { remote_tailer: remote_tailer.unwrap(), ..Default::default() };
        return (job.sid, telemetry);
    }
    sync_tmux_window_change(&mut job.telemetry, &job.tmux_window);
    let live_ssh = has_live_ssh(&job);
    poll_transport_tailers(&mut job, live_ssh, remote_tailer, home, now);
    poll_or_clear_vte(&mut job, live_ssh);
    job.telemetry.sync_effective();
    (job.sid, job.telemetry)
}

fn poll_transport_tailers(
    job: &mut SessionPollJob,
    live_ssh: bool,
    remote_tailer: Option<crate::remote_telemetry::RemoteTailer>,
    home: &Path,
    now: Instant,
) {
    if let Some(cwd) = (!live_ssh).then_some(job.local_cwd.as_deref()).flatten() {
        reset_remote_tailer(&mut job.telemetry);
        refresh_local_jsonl(&mut job.telemetry, home, cwd, job.root_pid, job.has_tmux, &job.agent, now);
        return;
    }
    reset_local_tailer(&mut job.telemetry);
    if let Some(rt) = remote_tailer {
        apply_remote_tailer(&mut job.telemetry, rt);
    } else {
        reset_remote_tailer(&mut job.telemetry);
    }
}

fn poll_or_clear_vte(job: &mut SessionPollJob, live_ssh: bool) {
    let local_cwd = (!live_ssh).then_some(job.local_cwd.as_deref()).flatten();
    if job.tmux_window.is_none() && should_poll_vte_fallback(local_cwd, job.has_tmux, &job.agent) {
        refresh_vte_telemetry(&mut job.telemetry, job.vte);
    } else {
        job.telemetry.vte_fallback = TokenSnapshot::default();
        job.telemetry.vte_panel_override = None;
    }
}

fn refresh_vte_telemetry(telem: &mut SessionTelemetry, vte: *mut GtkWidget) {
    if vte.is_null() {
        return;
    }
    let visible = read_vte_visible_text(vte);
    telem.observe_vte_text(&visible);
}

fn invalidate_panel_override_if_advanced(telem: &mut SessionTelemetry, prev_path: Option<&Path>) {
    if let Some((_, _, captured_offset)) = telem.vte_panel_override {
        let switched = prev_path.is_some() && telem.tailer.path.as_deref() != prev_path;
        if telem.tailer.offset != captured_offset || switched {
            telem.vte_panel_override = None;
        }
    }
}

fn is_opencode_context(telem: &SessionTelemetry, agent: &str) -> bool {
    agent == "OpenCode" || telem.tailer.kind == TelemetrySource::OpenCode
}

fn clear_stale_local_telemetry(telem: &mut SessionTelemetry) {
    telem.tailer = TranscriptTailer::default();
    telem.vte_fallback = TokenSnapshot::default();
    telem.vte_panel_override = None;
}

fn refresh_local_jsonl(
    telem: &mut SessionTelemetry,
    home: &Path,
    cwd: &str,
    root_pid: i32,
    has_tmux: bool,
    agent: &str,
    now: Instant,
) {
    let prev_path = telem.tailer.path.clone();
    if is_opencode_context(telem, agent) || telem.should_rediscover(now) {
        telem.last_discovery = Some(now);
        if let Some((path, kind)) = discover_transcript(home, cwd, root_pid, agent) {
            let _ = telem.tailer.poll_file(&path, kind);
            invalidate_panel_override_if_advanced(telem, prev_path.as_deref());
            return;
        }
        if has_tmux || is_opencode_context(telem, agent) {
            clear_stale_local_telemetry(telem);
            return;
        }
    }
    poll_existing_local_tailer(telem, prev_path.as_deref());
}

fn poll_existing_local_tailer(telem: &mut SessionTelemetry, prev_path: Option<&Path>) {
    if let Some(path) = telem.tailer.path.clone() {
        let kind = telem.tailer.kind;
        let _ = telem.tailer.poll_file(&path, kind);
    }
    invalidate_panel_override_if_advanced(telem, prev_path);
}

pub fn encode_claude_cwd(cwd: &str) -> String {
    trim_trailing_slash(cwd.trim())
        .chars()
        .map(|c| if matches!(c, '/' | '.') { '-' } else { c })
        .collect()
}

pub fn discover_transcript(
    home: &Path,
    cwd: &str,
    root_pid: i32,
    agent: &str,
) -> Option<(PathBuf, TelemetrySource)> {
    let norm_cwd = trim_trailing_slash(cwd.trim());
    if norm_cwd.is_empty() {
        return None;
    }
    let low = agent.to_ascii_lowercase();
    if let Some(owned) = discover_owned_by_fd(home, norm_cwd, root_pid, agent, &low) {
        return owned;
    }
    discover_by_agent_name(home, norm_cwd, root_pid, &low)
}

fn discover_owned_by_fd(
    home: &Path,
    norm_cwd: &str,
    root_pid: i32,
    agent: &str,
    low: &str,
) -> Option<Option<(PathBuf, TelemetrySource)>> {
    if root_pid <= 0 || !matches!(low, "claude" | "codex" | "agy") {
        return None;
    }
    let owned = match low {
        "claude" => transcript_owner::claude_fd(root_pid, home).map(|p| (p, TelemetrySource::Claude)),
        "codex" => crate::tmux_local::discover_codex_by_fd(root_pid, home).map(|p| (p, TelemetrySource::Codex)),
        _ => discover_agy_by_fd(root_pid, home).map(|p| (p, TelemetrySource::Agy)),
    };
    let shared = transcript_owner::shared_workspace(root_pid, Path::new(norm_cwd), agent);
    (owned.is_some() || shared).then_some(owned)
}

fn discover_by_agent_name(
    home: &Path,
    norm_cwd: &str,
    root_pid: i32,
    low: &str,
) -> Option<(PathBuf, TelemetrySource)> {
    if low.contains("agy") || low.contains("antigravity") {
        return discover_agy_transcript(home, norm_cwd, root_pid).map(|p| (p, TelemetrySource::Agy));
    }
    if low == "opencode" {
        return crate::telemetry::opencode::discover(root_pid).map(|p| (p, TelemetrySource::OpenCode));
    }
    if low.contains("claude") {
        return discover_claude_transcript(home, norm_cwd).map(|p| (p, TelemetrySource::Claude));
    }
    if low.contains("codex") {
        return discover_codex_transcript(home, norm_cwd).map(|p| (p, TelemetrySource::Codex));
    }
    discover_agy_by_fd(root_pid, home).map(|p| (p, TelemetrySource::Agy))
}

fn is_jsonl_file(path: &Path) -> bool {
    let ext_ok = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"));
    ext_ok && path.is_file()
}

fn newest_jsonl_in_dir(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut best: Option<((u64, String), PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_jsonl_file(&path) {
            continue;
        }
        let key = (
            fs::metadata(&path).map_or(0, |m| metadata_stamp(&m)),
            entry.file_name().to_string_lossy().into_owned(),
        );
        if best.as_ref().is_none_or(|(prev, _)| key > *prev) {
            best = Some((key, path));
        }
    }
    best.map(|(_, path)| path)
}

fn discover_claude_transcript(home: &Path, cwd: &str) -> Option<PathBuf> {
    let projects = home.join(".claude/projects");
    if !projects.is_dir() {
        return None;
    }
    let mut cur = PathBuf::from(cwd);
    for _ in 0..=3 {
        if cur.as_os_str().is_empty() || cur == Path::new("/") || cur == home {
            break;
        }
        let candidate_dir = projects.join(encode_claude_cwd(&cur.to_string_lossy()));
        if let Some(jsonl) = newest_jsonl_in_dir(&candidate_dir) {
            return Some(jsonl);
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

fn collect_jsonl_recursive(dir: &Path, depth: usize, out: &mut Vec<(u64, PathBuf)>) {
    if depth > 5 || out.len() >= 192 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort_by(|a, b| b.cmp(a));
    for path in paths {
        if out.len() >= 192 {
            break;
        }
        visit_codex_entry(&path, depth, out);
    }
}

fn visit_codex_entry(path: &Path, depth: usize, out: &mut Vec<(u64, PathBuf)>) {
    if path.is_dir() {
        collect_jsonl_recursive(path, depth + 1, out);
    } else if is_jsonl_file(path) {
        let stamp = fs::metadata(path).map_or(0, |m| metadata_stamp(&m));
        out.push((stamp, path.to_path_buf()));
    }
}

fn read_codex_header_meta(path: &Path) -> Option<(String, bool)> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file.take(262_144));
    let mut line = String::new();
    for _ in 0..5 {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        if let Some(dir) = extract_header_cwd(&line) {
            return Some((dir, is_codex_subagent_header(&line)));
        }
    }
    None
}

fn extract_header_cwd(line: &str) -> Option<String> {
    let raw = json_str(line, "cwd")
        .or_else(|| json_obj_prefix(line, "payload").and_then(|p| json_str(p, "cwd")))?;
    (!raw.is_empty()).then(|| trim_trailing_slash(&raw).to_owned())
}

fn is_codex_subagent_header(line: &str) -> bool {
    let payload = json_obj_prefix(line, "payload").unwrap_or(line);
    json_obj(payload, "source")
        .and_then(|src| json_direct_value(src, "subagent"))
        .is_some()
}

fn file_has_codex_tokens(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else { return false };
    let len = file.metadata().map_or(0, |m| m.len());
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(65_536)));
    let mut buf = Vec::with_capacity(len.min(65_536) as usize);
    file.read_to_end(&mut buf).is_ok()
        && buf.windows(b"\"token_count\"".len()).any(|w| w == b"\"token_count\"")
}

fn discover_codex_transcript(home: &Path, cwd: &str) -> Option<PathBuf> {
    let root = home.join(".codex/sessions");
    if !root.is_dir() {
        return None;
    }
    let mut files = Vec::new();
    collect_jsonl_recursive(&root, 0, &mut files);
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.truncate(48);
    match_codex_candidates(&files, cwd)
}

fn match_codex_candidates(files: &[(u64, PathBuf)], cwd: &str) -> Option<PathBuf> {
    let cwd_path = Path::new(cwd);
    let (mut exact_any, mut sub_tok, mut sub_any) = (None, None, None);
    for (_, path) in files {
        let Some((hdr_cwd, false)) = read_codex_header_meta(path) else { continue };
        if hdr_cwd == cwd {
            if file_has_codex_tokens(path) {
                return Some(path.clone());
            }
            exact_any.get_or_insert_with(|| path.clone());
        } else if sub_tok.is_none() && cwd_path.starts_with(Path::new(&hdr_cwd)) {
            record_codex_submatch(path, &mut sub_tok, &mut sub_any);
        }
    }
    exact_any.or(sub_tok).or(sub_any)
}

fn record_codex_submatch(path: &Path, sub_tok: &mut Option<PathBuf>, sub_any: &mut Option<PathBuf>) {
    if file_has_codex_tokens(path) {
        *sub_tok = Some(path.to_path_buf());
    } else {
        sub_any.get_or_insert_with(|| path.to_path_buf());
    }
}

pub fn strip_ansi_escapes(raw: &str) -> String {
    if !raw.contains('\x1b') {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            consume_ansi_seq(&mut chars);
        } else {
            out.push(ch);
        }
    }
    out
}

fn consume_ansi_seq(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
        Some(']') => consume_osc_seq(chars),
        Some('(' | ')' | '*' | '+') => {
            let _ = chars.next();
        }
        _ => {}
    }
}

fn consume_osc_seq(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        if c == '\x07' {
            break;
        }
        if c == '\x1b' && chars.peek() == Some(&'\\') {
            let _ = chars.next();
            break;
        }
    }
}

pub(crate) fn clean_status_line(line: &str) -> &str {
    line.trim_matches(|c: char| c.is_whitespace() || "│┃┆┊║╭╮╰╯┌┐└┘├┤┬┴┼─━═".contains(c))
}

pub(crate) fn strip_punct(raw: &str) -> &str {
    raw.trim_matches(|c: char| ",;:()[]".contains(c))
}

fn has_token_scale(raw: &str) -> bool {
    matches!(strip_punct(raw).as_bytes().last(), Some(b'k' | b'K' | b'm' | b'M'))
}

fn split_token_scale(s: &str) -> Option<(&str, f64)> {
    match s.as_bytes().last().copied()? {
        b'k' | b'K' => Some((&s[..s.len() - 1], 1_000.0)),
        b'm' | b'M' => Some((&s[..s.len() - 1], 1_000_000.0)),
        _ => Some((s, 1.0)),
    }
}

pub(crate) fn parse_human_tokens(raw: &str) -> Option<u64> {
    let s = strip_punct(raw);
    if s.is_empty() || s.ends_with('%') || s.starts_with('$') {
        return None;
    }
    let (num_part, mult) = split_token_scale(s)?;
    let clean: String = num_part.chars().filter(|c| !matches!(c, ',' | '_')).collect();
    if clean.is_empty() || (mult == 1.0 && clean.contains('.')) {
        return None;
    }
    if !clean.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let val = clean.parse::<f64>().ok()?;
    (val > 0.0).then_some((val * mult).round() as u64)
}

pub(crate) fn parse_percent_token(raw: &str) -> Option<u8> {
    let body = strip_punct(raw).strip_suffix('%')?;
    let pct = body.parse::<f64>().ok()?;
    (0.0..=100.0).contains(&pct).then_some(pct.round() as u8)
}

fn parse_dollar_token(raw: &str) -> Option<f64> {
    let rest = strip_punct(raw).strip_prefix('$')?;
    let (whole, frac) = rest.split_once('.')?;
    let digits_ok = !whole.is_empty()
        && !frac.is_empty()
        && whole.chars().all(|c| c.is_ascii_digit())
        && frac.chars().all(|c| c.is_ascii_digit());
    digits_ok.then(|| rest.parse::<f64>().ok()).flatten()
}

fn apply_ratio_sides(used_s: &str, limit_s: &str, require_scale: bool, snap: &mut TokenSnapshot) {
    let Some(used) = parse_human_tokens(used_s) else { return };
    let Some(lim) = parse_human_tokens(limit_s).filter(|l| *l > 0) else { return };
    if used > lim.saturating_mul(2) {
        return;
    }
    if require_scale && !has_token_scale(used_s) && !has_token_scale(limit_s) && lim < 4_000 {
        return;
    }
    snap.context_tokens = used;
    snap.context_pct = Some(((used.saturating_mul(100)) / lim).min(100) as u8);
}

fn apply_token_pair(raw: &str, snap: &mut TokenSnapshot) {
    if let Some((used_s, limit_s)) = strip_punct(raw).split_once('/') {
        apply_ratio_sides(used_s, limit_s, false, snap);
    } else if let Some(toks) = parse_human_tokens(raw) {
        snap.context_tokens = toks;
    }
}

fn apply_standalone_ratio(words: &[&str], idx: usize, snap: &mut TokenSnapshot) {
    let w = strip_punct(words[idx]);
    if w == "/" {
        let prev = idx.checked_sub(1).and_then(|i| words.get(i)).copied();
        if let (Some(lhs), Some(&rhs)) = (prev, words.get(idx + 1)) {
            apply_ratio_sides(lhs, rhs, true, snap);
        }
        return;
    }
    let Some((lhs, rhs)) = w.split_once('/') else { return };
    let used_s = if lhs.is_empty() { idx.checked_sub(1).and_then(|i| words.get(i)).copied().unwrap_or("") } else { lhs };
    let lim_s = if rhs.is_empty() { words.get(idx + 1).copied().unwrap_or("") } else { rhs };
    apply_ratio_sides(used_s, lim_s, true, snap);
}

fn apply_value_token(val: &str, snap: &mut TokenSnapshot) {
    if let Some(pct) = parse_percent_token(val) {
        snap.context_pct = Some(pct);
    } else {
        apply_token_pair(val, snap);
    }
}

fn apply_colon_kv(word: &str, snap: &mut TokenSnapshot) {
    let Some((key, val)) = strip_punct(word).split_once(':') else { return };
    if val.is_empty() {
        return;
    }
    match key.to_ascii_lowercase().as_str() {
        "ctx" | "context" | "tokens" | "tok" => apply_value_token(val, snap),
        "cost" => {
            if let Some(c) = parse_dollar_token(val) {
                snap.cost_usd = Some(c);
            }
        }
        _ => {}
    }
}

fn apply_word_at(words: &[&str], idx: usize, snap: &mut TokenSnapshot) {
    let w = words[idx];
    if let Some(cost) = parse_dollar_token(w) {
        snap.cost_usd = Some(cost);
    }
    apply_colon_kv(w, snap);
    apply_standalone_ratio(words, idx, snap);
    apply_keyword_token(words, idx, snap);
}

fn apply_keyword_token(words: &[&str], idx: usize, snap: &mut TokenSnapshot) {
    let low = words[idx].trim_matches(|c: char| ",;()[]".contains(c)).to_ascii_lowercase();
    if matches!(low.as_str(), "tokens" | "token" | "tok" | "токенов") && idx > 0 {
        apply_token_pair(words[idx - 1], snap);
    }
    if matches!(low.as_str(), "tokens:" | "ctx" | "ctx:" | "context:") {
        if let Some(&next_w) = words.get(idx + 1) {
            apply_value_token(next_w, snap);
        }
    }
    if matches!(low.as_str(), "context" | "ctx") && idx > 0 {
        apply_preceding_pct(words, idx, snap);
    }
}

fn apply_preceding_pct(words: &[&str], idx: usize, snap: &mut TokenSnapshot) {
    let Some(pct) = parse_percent_token(words[idx - 1]) else { return };
    let is_left = words
        .get(idx + 1)
        .is_some_and(|n| matches!(strip_punct(n).to_ascii_lowercase().as_str(), "left" | "remaining"));
    snap.context_pct = Some(if is_left { 100u8.saturating_sub(pct) } else { pct });
}

pub fn parse_vte_status_fallback(visible_text: &str) -> Option<TokenSnapshot> {
    let stripped = strip_ansi_escapes(visible_text);
    let mut snap = TokenSnapshot { source: TelemetrySource::VteFallback, ..Default::default() };
    for line in bottom_status_lines(&stripped) {
        scan_status_line(line, &mut snap);
    }
    snap.has_data().then_some(snap)
}

fn bottom_status_lines(stripped: &str) -> Vec<&str> {
    let mut bottom: Vec<&str> = stripped
        .lines()
        .rev()
        .map(clean_status_line)
        .filter(|l| !l.is_empty())
        .take(VTE_FALLBACK_LINES)
        .collect();
    bottom.reverse();
    bottom
}

fn scan_status_line(line: &str, snap: &mut TokenSnapshot) {
    if is_vte_breakdown_line(line) {
        return;
    }
    let words: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || matches!(c, '|' | '·' | '•'))
        .filter(|s| !s.is_empty())
        .collect();
    for idx in 0..words.len() {
        apply_word_at(&words, idx, snap);
    }
}


#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod tests;
