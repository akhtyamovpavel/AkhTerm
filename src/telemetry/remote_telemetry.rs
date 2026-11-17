//! Batched, TTL-throttled background SSH/tmux agent context and JSONL transcript telemetry.

use std::collections::HashMap;
use std::ops::Deref;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[path = "ssh_connection.rs"]
mod ssh_connection;
use crate::proc_radar::shell_quote;
use crate::session::{SessionCategory, TermSession};
use crate::tabs::is_ssh_host;
#[cfg(test)]
use crate::tabs::Tab;
use crate::telemetry::{
    is_complete_json_object, json_obj, json_obj_prefix, json_u64, TelemetrySource, TranscriptTailer,
};

pub const REMOTE_PROBE_TTL: Duration = Duration::from_secs(8);
pub const REMOTE_FULL_MAX_BYTES: u64 = 262_144;
pub const REMOTE_HEAD_BYTES: usize = 32_768;
pub const REMOTE_TAIL_BYTES: usize = 131_072;

const FRAME_PREFIX: &[u8] = b"AKHTERM_RTELEM1|";
const FRAME_END: &[u8] = b"\nAKHTERM_RTELEM_END\n";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteSessionTarget {
    pub sid: usize,
    pub host: String,
    pub ports: Option<(u16, u16)>,
    pub ssh_pid: i32,
    pub tmux_session: Option<String>,
    pub tmux_window: Option<String>,
    pub pane_cwd_hint: String,
    pub remote_cwd: Option<String>,
    pub agent_hint: String,
}

#[derive(Clone, Debug, Default)]
pub struct RemoteTailer {
    pub host: String,
    pub ports: Option<(u16, u16)>,
    pub ssh_pid: i32,
    pub tmux_session: Option<String>,
    pub tmux_window: Option<String>,
    pub agent_hint: String,
    pub remote_path: String,
    pub remote_offset: u64,
    pub mtime: u64,
    pub tailer: TranscriptTailer,
    codex_window: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameMode {
    None,
    Unchanged,
    Full,
    HeadTail,
    Delta,
    TailDelta,
}

#[derive(Clone, Debug)]
pub struct RemoteProbeFrame {
    pub sid: usize,
    pub mode: FrameMode,
    pub kind: TelemetrySource,
    pub size: u64,
    pub mtime: u64,
    pub path: String,
    pub head: Vec<u8>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct SessionProbeQuery {
    pub sid: usize,
    pub ports: Option<(u16, u16)>,
    pub ssh_pid: i32,
    pub tmux_session: Option<String>,
    pub tmux_window: Option<String>,
    pub pane_cwd_hint: String,
    pub agent_hint: String,
    pub prev_path: String,
    pub prev_kind: TelemetrySource,
    pub prev_offset: u64,
}

struct SchedulerState {
    targets: Vec<RemoteSessionTarget>,
    tailers: HashMap<usize, RemoteTailer>,
    host_next_probe_at: HashMap<String, Instant>,
    generations: HashMap<usize, u64>,
    ttl: Duration,
}

struct HostBatchJob {
    host: String,
    queries: Vec<(u64, SessionProbeQuery)>,
}

#[derive(Clone)]
pub struct RemoteTelemetryScheduler {
    inner: Arc<(Mutex<SchedulerState>, Condvar)>,
}

impl RemoteSessionTarget {
    pub fn from_pane(s: &TermSession, pane: &crate::pane_registry::PaneRecord) -> Option<Self> {
        if s.category == SessionCategory::Daemon || pane.child_pid <= 0 { return None; }
        let configured = s.panes.remote_for_vte(pane.vte, s.scroll_wrap, s.vte).or_else(|| s.remote());
        let host = configured.map_or(pane.proc_info.ssh_host.as_str(), |r| r.host.as_str());
        if !is_ssh_host(host) { return None; }
        let live = pane.remote_tmux.as_ref();
        let tmux_session = live.map(|t| t.0.clone()).or_else(|| configured.and_then(|r| r.tmux_session.clone()));
        let ssh_pid = pane.proc_info.ssh_pid;
        Some(Self {
            sid: pane.probe_id, host: host.to_owned(), ports: None, ssh_pid, tmux_session,
            tmux_window: live.and_then(|_| crate::tmux_local::pane_tmux_window(pane)),
            pane_cwd_hint: live.map(|t| t.3.clone()).unwrap_or_default(),
            remote_cwd: live.map(|t| t.3.clone()),
            agent_hint: live.and_then(|t| classify_agent_hint(&t.2)).unwrap_or("").to_owned(),
        })
    }

    #[cfg(test)]
    pub fn from_session(s: &TermSession) -> Option<Self> {
        use crate::pane_registry::{PaneId, PaneRecord};
        let mut pane = PaneRecord::new(PaneId(0), s.vte, std::ptr::null_mut(), s.child_pid.max(1), s.cwd.clone());
        pane.probe_id = s.id;
        pane.proc_info.clone_from(&s.proc_info);
        pane.remote_tmux.clone_from(&s.remote_tmux);
        Self::from_pane(s, &pane)
    }
}

fn classify_agent_hint(raw: &str) -> Option<&'static str> {
    let low = raw.to_ascii_lowercase();
    if low.contains("agy") || low.contains("antigravity") {
        Some("agy")
    } else if low.contains("opencode") {
        Some("opencode")
    } else if low.contains("codex") {
        Some("codex")
    } else if low.contains("claude") {
        Some("claude")
    } else {
        None
    }
}

impl Deref for RemoteTailer {
    type Target = TranscriptTailer;

    fn deref(&self) -> &Self::Target {
        &self.tailer
    }
}

impl RemoteTailer {
    pub fn for_target(target: &RemoteSessionTarget) -> Self {
        Self {
            host: target.host.clone(),
            agent_hint: target.agent_hint.clone(),
            ports: target.ports,
            ssh_pid: target.ssh_pid,
            tmux_session: target.tmux_session.clone(),
            tmux_window: target.tmux_window.clone(),
            ..Default::default()
        }
    }

    pub fn matches_target(&self, target: &RemoteSessionTarget) -> bool {
        self.ssh_pid == target.ssh_pid && self.ports == target.ports && self.host == target.host
            && self.agent_hint == target.agent_hint
            && self.tmux_session == target.tmux_session
            && self.tmux_window == target.tmux_window
    }

    pub fn apply_frame(&mut self, frame: &RemoteProbeFrame) -> bool {
        match frame.mode {
            FrameMode::None => self.clear_tailer(),
            FrameMode::Unchanged => {
                self.mtime = frame.mtime;
                false
            }
            FrameMode::Full => self.apply_full(frame),
            FrameMode::HeadTail => self.apply_head_tail(frame),
            FrameMode::Delta => self.apply_delta(frame),
            FrameMode::TailDelta => self.apply_tail_delta(frame),
        }
    }

    fn clear_tailer(&mut self) -> bool {
        let had = self.tailer.path.is_some() || !self.remote_path.is_empty();
        self.remote_path.clear();
        self.remote_offset = 0;
        self.mtime = 0;
        self.codex_window = None;
        self.tailer = TranscriptTailer::default();
        had
    }

    fn reset_file(&mut self, path: &str, kind: TelemetrySource, mtime: u64) {
        self.remote_path = path.to_owned();
        self.remote_offset = 0;
        self.mtime = mtime;
        self.codex_window = None;
        self.tailer.reset_for(PathBuf::from(path), kind);
    }

    fn apply_full(&mut self, frame: &RemoteProbeFrame) -> bool {
        self.reset_file(&frame.path, frame.kind, frame.mtime);
        let consumed = self.feed_complete_lines(&frame.body);
        self.sync_offset(consumed as u64);
        true
    }

    fn apply_head_tail(&mut self, frame: &RemoteProbeFrame) -> bool {
        self.reset_file(&frame.path, frame.kind, frame.mtime);
        self.scan_header_metadata(&frame.head);
        let unconsumed = self.feed_tail_lines(&frame.body);
        self.sync_offset(frame.size.saturating_sub(unconsumed as u64));
        true
    }

    fn apply_delta(&mut self, frame: &RemoteProbeFrame) -> bool {
        if self.remote_path != frame.path || self.tailer.kind != frame.kind {
            return self.apply_full(frame);
        }
        self.mtime = frame.mtime;
        let consumed = self.feed_complete_lines(&frame.body);
        self.sync_offset(self.remote_offset.saturating_add(consumed as u64));
        consumed > 0
    }

    fn apply_tail_delta(&mut self, frame: &RemoteProbeFrame) -> bool {
        if self.remote_path != frame.path || self.tailer.kind != frame.kind {
            self.reset_file(&frame.path, frame.kind, frame.mtime);
        }
        self.mtime = frame.mtime;
        let unconsumed = self.feed_tail_lines(&frame.body);
        self.sync_offset(frame.size.saturating_sub(unconsumed as u64));
        true
    }

    fn sync_offset(&mut self, next_offset: u64) {
        self.remote_offset = next_offset;
        self.tailer.offset = next_offset;
        self.apply_codex_window_fallback();
    }

    fn scan_header_metadata(&mut self, head: &[u8]) {
        for raw in head.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(raw);
            if let Some(win) = extract_codex_window(&line) {
                self.codex_window = Some(win);
            }
        }
    }

    fn feed_complete_lines(&mut self, bytes: &[u8]) -> usize {
        let mut pos = 0usize;
        while pos < bytes.len() {
            let Some(rel_nl) = bytes[pos..].iter().position(|&b| b == b'\n') else {
                pos += self.consume_trailing_json(&bytes[pos..]);
                break;
            };
            let end = pos + rel_nl + 1;
            self.consume_str(&String::from_utf8_lossy(&bytes[pos..end]));
            pos = end;
        }
        pos
    }

    fn consume_trailing_json(&mut self, tail: &[u8]) -> usize {
        let line = String::from_utf8_lossy(tail);
        if is_complete_json_object(&line) {
            self.consume_str(&line);
            tail.len()
        } else {
            0
        }
    }

    fn feed_tail_lines(&mut self, bytes: &[u8]) -> usize {
        let Some(first_nl) = bytes.iter().position(|&b| b == b'\n') else {
            let used = self.consume_trailing_json(bytes);
            return bytes.len().saturating_sub(used);
        };
        let rest = &bytes[first_nl + 1..];
        let consumed = self.feed_complete_lines(rest);
        rest.len().saturating_sub(consumed)
    }

    fn consume_str(&mut self, line: &str) {
        if let Some(win) = extract_codex_window(line) {
            self.codex_window = Some(win);
        }
        self.tailer.consume_line(line);
    }

    fn apply_codex_window_fallback(&mut self) {
        if self.tailer.kind != TelemetrySource::Codex {
            return;
        }
        let ctx = self.tailer.snapshot.context_tokens;
        if let Some(win) = self.codex_window.filter(|&w| w > 0 && ctx > 0) {
            self.tailer.snapshot.context_pct = Some(((ctx.saturating_mul(100)) / win).min(100) as u8);
        }
    }
}

fn extract_codex_window(line: &str) -> Option<u64> {
    json_u64(line, "model_context_window")
        .or_else(|| json_obj(line, "info").and_then(|o| json_u64(o, "model_context_window")))
        .or_else(|| extract_payload_window(line))
        .filter(|&w| w > 0)
}

fn extract_payload_window(line: &str) -> Option<u64> {
    let payload = json_obj_prefix(line, "payload")?;
    json_u64(payload, "model_context_window")
        .or_else(|| json_obj(payload, "info").and_then(|o| json_u64(o, "model_context_window")))
}

impl FrameMode {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "NONE" => Some(Self::None),
            "UNCHANGED" => Some(Self::Unchanged),
            "FULL" => Some(Self::Full),
            "HEAD_TAIL" => Some(Self::HeadTail),
            "DELTA" => Some(Self::Delta),
            "TAIL_DELTA" => Some(Self::TailDelta),
            _ => None,
        }
    }
}

fn parse_source_kind(raw: &str) -> TelemetrySource {
    match raw {
        "agy" => TelemetrySource::Agy,
        "claude" => TelemetrySource::Claude,
        "opencode" => TelemetrySource::OpenCode,
        "codex" => TelemetrySource::Codex,
        _ => TelemetrySource::None,
    }
}

fn source_kind_slug(kind: TelemetrySource) -> &'static str {
    match kind {
        TelemetrySource::Agy => "agy",
        TelemetrySource::Claude => "claude",
        TelemetrySource::OpenCode => "opencode",
        TelemetrySource::Codex => "codex",
        TelemetrySource::None | TelemetrySource::VteFallback => "",
    }
}

pub fn posix_quote(raw: &str) -> String {
    let clean: String = raw.chars().filter(|&c| !matches!(c, '\0' | '\n' | '\r')).collect();
    shell_quote(&clean)
}

pub fn build_ssh_probe_args(host: &str, posix_script: &str) -> Vec<String> {
    vec![
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ConnectTimeout=2".to_owned(),
        "-o".to_owned(),
        "ClearAllForwardings=yes".to_owned(),
        "--".to_owned(),
        host.to_owned(),
        "sh".to_owned(),
        "-c".to_owned(),
        shell_quote(posix_script),
    ]
}

pub fn run_ssh_probe_with_program(program: &str, host: &str, posix_script: &str) -> Option<Vec<u8>> {
    if !is_ssh_host(host) {
        return None;
    }
    let args = build_ssh_probe_args(host, posix_script);
    let output = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

pub fn run_default_ssh_probe(host: &str, posix_script: &str) -> Option<Vec<u8>> {
    run_ssh_probe_with_program("ssh", host, posix_script)
}

#[path = "remote_probe.rs"]
mod remote_probe;
pub use remote_probe::build_posix_probe_script;

pub fn parse_probe_stdout(stdout: &[u8]) -> Vec<RemoteProbeFrame> {
    let mut frames = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel_start) = find_subslice(&stdout[cursor..], FRAME_PREFIX) {
        let header_start = cursor + rel_start;
        let Some((frame, next_cursor)) = parse_single_frame(stdout, header_start) else {
            cursor = header_start + FRAME_PREFIX.len();
            continue;
        };
        frames.push(frame);
        cursor = next_cursor;
    }
    frames
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_single_frame(stdout: &[u8], header_start: usize) -> Option<(RemoteProbeFrame, usize)> {
    let after_prefix = header_start + FRAME_PREFIX.len();
    let rel_nl = stdout[after_prefix..].iter().position(|&b| b == b'\n')?;
    let header_str = std::str::from_utf8(&stdout[after_prefix..after_prefix + rel_nl]).ok()?;
    let (sid, mode, kind, size, mtime, head_len, body_len, path) = parse_frame_header(header_str)?;
    let payload_start = after_prefix + rel_nl + 1;
    let payload_end = payload_start.checked_add(head_len)?.checked_add(body_len)?;
    let trailer_end = payload_end.checked_add(FRAME_END.len())?;
    if trailer_end > stdout.len() || &stdout[payload_end..trailer_end] != FRAME_END {
        return None;
    }
    let head = stdout[payload_start..payload_start + head_len].to_vec();
    let body = stdout[payload_start + head_len..payload_end].to_vec();
    Some((RemoteProbeFrame { sid, mode, kind, size, mtime, path, head, body }, trailer_end))
}

fn parse_frame_header(
    raw: &str,
) -> Option<(usize, FrameMode, TelemetrySource, u64, u64, usize, usize, String)> {
    let mut parts = raw.splitn(8, '|');
    Some((
        parts.next()?.parse().ok()?,
        FrameMode::parse(parts.next()?)?,
        parse_source_kind(parts.next()?),
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.to_owned(),
    ))
}

impl SchedulerState {
    fn new(ttl: Duration) -> Self {
        Self {
            targets: Vec::new(),
            tailers: HashMap::new(),
            host_next_probe_at: HashMap::new(),
            generations: HashMap::new(),
            ttl,
        }
    }

    fn sync_targets(&mut self, incoming: &[RemoteSessionTarget]) -> bool {
        let mut wake_now = false;
        self.tailers.retain(|sid, _| incoming.iter().any(|t| t.sid == *sid));
        for target in incoming {
            if self.tailers.get(&target.sid).is_none_or(|rt| !rt.matches_target(target)) {
                self.bump_generation(target.sid);
                self.tailers.insert(target.sid, RemoteTailer::for_target(target));
                self.host_next_probe_at.remove(&target.host);
                wake_now = true;
            }
        }
        self.targets = incoming.to_vec();
        wake_now
    }

    fn bump_generation(&mut self, sid: usize) {
        let next_gen = self.generations.get(&sid).copied().unwrap_or(0) + 1;
        self.generations.insert(sid, next_gen);
    }

    fn reset_session(&mut self, sid: usize) {
        self.bump_generation(sid);
        if let Some(prev) = self.tailers.remove(&sid) {
            self.host_next_probe_at.remove(&prev.host);
        }
        self.targets.retain(|t| t.sid != sid);
    }

    fn host_is_due(&self, host: &str, now: Instant) -> bool {
        self.host_next_probe_at.get(host).is_none_or(|&at| now >= at)
    }

    fn claim_due_batches(&mut self, now: Instant) -> Vec<HostBatchJob> {
        let mut hosts = Vec::new();
        for target in &self.targets {
            if self.host_is_due(&target.host, now) && !hosts.contains(&target.host) {
                hosts.push(target.host.clone());
            }
        }
        let mut jobs = Vec::with_capacity(hosts.len());
        for host in hosts {
            self.host_next_probe_at.insert(host.clone(), now + self.ttl);
            let queries = self.build_queries_for_host(&host);
            if !queries.is_empty() {
                jobs.push(HostBatchJob { host, queries });
            }
        }
        jobs
    }

    fn build_queries_for_host(&self, host: &str) -> Vec<(u64, SessionProbeQuery)> {
        self.targets
            .iter()
            .filter(|t| t.host == host)
            .map(|t| (self.generations.get(&t.sid).copied().unwrap_or(0), self.query_for(t)))
            .collect()
    }

    fn query_for(&self, target: &RemoteSessionTarget) -> SessionProbeQuery {
        let rt = self.tailers.get(&target.sid);
        SessionProbeQuery {
            sid: target.sid,
            ports: target.ports,
            ssh_pid: target.ssh_pid,
            tmux_session: target.tmux_session.clone(),
            tmux_window: target.tmux_window.clone(),
            pane_cwd_hint: target.pane_cwd_hint.clone(),
            agent_hint: target.agent_hint.clone(),
            prev_path: rt.map_or_else(String::new, |r| r.remote_path.clone()),
            prev_kind: rt.map_or(TelemetrySource::None, |r| r.tailer.kind),
            prev_offset: rt.map_or(0, |r| r.remote_offset),
        }
    }

    fn apply_batch_frames(
        &mut self,
        queries: &[(u64, SessionProbeQuery)],
        frames: &[RemoteProbeFrame],
    ) -> usize {
        let mut changed = 0usize;
        for frame in frames {
            if !self.is_current_generation(queries, frame.sid) {
                continue;
            }
            if let Some(rt) = self.tailers.get_mut(&frame.sid) {
                changed += usize::from(rt.apply_frame(frame));
            }
        }
        changed
    }

    fn is_current_generation(&self, queries: &[(u64, SessionProbeQuery)], sid: usize) -> bool {
        let Some(&(expected, _)) = queries.iter().find(|(_, q)| q.sid == sid) else {
            return false;
        };
        self.generations.get(&sid).copied().unwrap_or(0) == expected
    }
}

impl Default for RemoteTelemetryScheduler {
    fn default() -> Self {
        Self::new(REMOTE_PROBE_TTL)
    }
}

impl RemoteTelemetryScheduler {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new((Mutex::new(SchedulerState::new(ttl)), Condvar::new())),
        }
    }

    pub fn sync_targets(&self, targets: &[RemoteSessionTarget]) {
        let (lock, cvar) = &*self.inner;
        let wake = lock.lock().ok().is_some_and(|mut g| g.sync_targets(targets));
        if wake {
            cvar.notify_one();
        }
    }

    pub fn reset_session(&self, sid: usize) {
        let (lock, cvar) = &*self.inner;
        if let Ok(mut g) = lock.lock() {
            g.reset_session(sid);
            cvar.notify_one();
        }
    }

    pub fn snapshot(&self) -> HashMap<usize, RemoteTailer> {
        self.inner.0.lock().ok().map(|g| g.tailers.clone()).unwrap_or_default()
    }

    pub fn refresh_due(&self, now: Instant) -> usize {
        self.refresh_due_with(now, run_default_ssh_probe)
    }

    pub fn refresh_due_with<F>(&self, now: Instant, mut runner: F) -> usize
    where
        F: FnMut(&str, &str) -> Option<Vec<u8>>,
    {
        let batches = self
            .inner
            .0
            .lock()
            .ok()
            .map(|mut g| g.claim_due_batches(now))
            .unwrap_or_default();
        let mut updated = 0usize;
        for batch in batches {
            updated += self.run_single_batch(batch, &mut runner);
        }
        updated
    }

    fn run_single_batch<F>(&self, batch: HostBatchJob, runner: &mut F) -> usize
    where
        F: FnMut(&str, &str) -> Option<Vec<u8>>,
    {
        let plain: Vec<SessionProbeQuery> = batch.queries.iter().map(|(_, q)| q.clone()).collect();
        let script = build_posix_probe_script(&plain);
        let Some(stdout) = runner(&batch.host, &script) else { return 0 };
        let frames = parse_probe_stdout(&stdout);
        self.inner
            .0
            .lock()
            .ok()
            .map_or(0, |mut g| g.apply_batch_frames(&batch.queries, &frames))
    }

    pub fn wait_for_turn(&self, max_sleep: Duration) {
        let (lock, cvar) = &*self.inner;
        if let Ok(guard) = lock.lock() {
            let _ = cvar.wait_timeout(guard, max_sleep);
        }
    }
}

#[cfg(test)]
#[path = "ssh_focus_tests.rs"]
mod ssh_focus_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::ffi::{c_char, c_int, c_void, CStr, CString};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;
    use std::ptr;

    use crate::ffi::{
        g_free, g_object_get, gtk_box_new, gtk_button_get_label, gtk_container_add,
        gtk_label_get_text, gtk_style_context_add_class, gtk_widget_destroy,
        gtk_widget_get_style_context, gtk_widget_show_all, gtk_window_new, GdkEvent,
        GtkStyleContext, GtkWidget, GTK_ORIENTATION_VERTICAL, GTK_WINDOW_TOPLEVEL,
    };
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::proc_radar::ProcInfo;
    use crate::session::WorkState;
    use crate::sidebar::{Sidebar, SidebarRow};
    use crate::split_pane::TabPanes;
    use crate::tab_strip::{TabHandlers, TabStrip, TabView};
    use crate::telemetry::{
        build_sessions_tokens_view, parse_vte_status_fallback, run_poll_job_with_remote,
        ContextAlert, SessionPollJob, SessionTelemetry, TokensMenu,
    };
    use crate::theme_mode::ThemeMode;

    extern "C" {
        fn gtk_style_context_has_class(
            context: *mut GtkStyleContext,
            class_name: *const c_char,
        ) -> c_int;
        fn gtk_widget_get_name(widget: *mut GtkWidget) -> *const c_char;
    }

    pub(crate) struct MockRemoteEnv {
        pub(crate) root: PathBuf,
        pub(crate) home: PathBuf,
        pub(crate) proc_dir: PathBuf,
        pub(crate) tmux_panes_file: PathBuf,
        pub(crate) calls_log: PathBuf,
        pub(crate) ssh_bin: PathBuf,
    }

    #[test]
    fn remote_telemetry_module_meets_grade_b_mi_target() {
        let (_, _, score) = crate::mi::compute_file_mi(include_str!("remote_telemetry.rs"));
        assert!(score >= 59.0, "remote_telemetry.rs MI {score:.2} < 59.0");
    }

    impl MockRemoteEnv {
        pub(crate) fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("akhterm_rtel_{tag}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let (home, proc_dir, bin_dir) = create_mock_dirs(&root);
            let tmux_panes_file = root.join("tmux_panes.tsv");
            let calls_log = root.join("ssh_calls.log");
            fs::write(&tmux_panes_file, "").unwrap();
            write_mock_tmux(&bin_dir.join("tmux"), &tmux_panes_file);
            let ssh_bin = bin_dir.join("mock-ssh");
            write_mock_ssh(&ssh_bin, &home, &proc_dir, &bin_dir, &calls_log);
            Self { root, home, proc_dir, tmux_panes_file, calls_log, ssh_bin }
        }

        pub(crate) fn call_count(&self) -> usize {
            fs::read_to_string(&self.calls_log).map_or(0, |s| s.lines().count())
        }

        pub(crate) fn add_proc(&self, pid: u32, cmdline_nul: &str, cwd: &Path, children: &[u32]) -> PathBuf {
            let pdir = self.proc_dir.join(pid.to_string());
            let task_dir = pdir.join(format!("task/{pid}"));
            fs::create_dir_all(pdir.join("fd")).unwrap();
            fs::create_dir_all(&task_dir).unwrap();
            fs::write(pdir.join("cmdline"), cmdline_nul.as_bytes()).unwrap();
            let kids: Vec<String> = children.iter().map(u32::to_string).collect();
            fs::write(task_dir.join("children"), kids.join(" ")).unwrap();
            let _ = symlink(cwd, pdir.join("cwd"));
            pdir
        }
    }

    impl Drop for MockRemoteEnv {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn create_mock_dirs(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let home = root.join("home");
        let proc_dir = root.join("proc");
        let bin_dir = root.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&proc_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();
        (home, proc_dir, bin_dir)
    }

    fn write_executable(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    fn write_mock_tmux(path: &Path, panes_file: &Path) {
        let script = format!(
            "#!/bin/sh\ncase \"$*\" in *'\t'*) exit 95 ;; esac\ncat '{}'\n",
            panes_file.display()
        );
        write_executable(path, &script);
    }

    fn mock_ssh_arg_guard() -> &'static str {
        concat!(
            "#!/bin/sh\n",
            "[ \"$1\" = \"-o\" ] && [ \"$2\" = \"BatchMode=yes\" ] || exit 91\n",
            "[ \"$3\" = \"-o\" ] && [ \"$4\" = \"ConnectTimeout=2\" ] || exit 92\n",
            "[ \"$5\" = \"-o\" ] && [ \"$6\" = \"ClearAllForwardings=yes\" ] || exit 93\n",
            "[ \"$7\" = \"--\" ] || exit 94\n",
            "host=\"$8\"; shift 8\n",
        )
    }

    fn write_mock_ssh(path: &Path, home: &Path, proc_dir: &Path, bin_dir: &Path, calls_log: &Path) {
        let script = format!(
            concat!(
                "{guard}",
                "printf '%s\\n' \"$host\" >> '{calls}'\n",
                "[ \"${{AKHTERM_MOCK_SSH_FAIL:-0}}\" = \"1\" ] && exit 255\n",
                "export HOME='{home}' AKHTERM_TEST_PROC='{proc}' PATH='{bin}':\"$PATH\"\n",
                "exec /bin/sh -c \"$*\"\n",
            ),
            guard = mock_ssh_arg_guard(),
            calls = calls_log.display(),
            home = home.display(),
            proc = proc_dir.display(),
            bin = bin_dir.display(),
        );
        write_executable(path, &script);
    }

    pub(crate) fn dummy_session(id: usize, category: SessionCategory, tab: Tab, ssh_host: &str) -> TermSession {
        build_session_with_views(
            id,
            "tab",
            category,
            tab,
            ssh_host,
            SidebarRow::empty_for_test(category),
            TabView::empty_for_test(category),
        )
    }

    fn build_session_with_views(
        id: usize,
        title: &str,
        category: SessionCategory,
        tab: Tab,
        ssh_host: &str,
        sidebar_row: SidebarRow,
        top_tab: TabView,
    ) -> TermSession {
        let now = Instant::now();
        TermSession {
            id, title: title.to_owned(), base_title: title.to_owned(), pinned_title: None,
            osc_title: String::new(), profile_cwd: String::new(), cwd: "/tmp".to_owned(),
            category, vte: ptr::null_mut(), scroll_wrap: ptr::null_mut(),
            panes: TabPanes::empty_for_test(), sidebar_row, top_tab, child_pid: 4242,
            state: WorkState::Idle, unread: false, last_output: now, work_started: now,
            work_started_at: None, last_turn_secs: None, last_notified: None,
            proc_info: ProcInfo { ssh_host: ssh_host.to_owned(), ..Default::default() },
            tab, remote_tmux: None, tmux_title_from_vte: false,
            telemetry: SessionTelemetry::default(),
            pane_registry: crate::pane_registry::PaneRegistry::default(),
        }
    }

    fn seed_claude_fixture(env: &MockRemoteEnv) -> PathBuf {
        let sub = env.home.join("work/my.repo/src");
        fs::create_dir_all(&sub).unwrap();
        let slug = crate::telemetry::encode_claude_cwd(&sub.parent().unwrap().to_string_lossy());
        let dir = env.home.join(".claude/projects").join(slug);
        fs::create_dir_all(&dir).unwrap();
        let c1 = "{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":200,\"cache_creation_input_tokens\":4800,\"cache_read_input_tokens\":30000,\"output_tokens\":120}}}\n";
        let c1b = "{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":200,\"cache_creation_input_tokens\":4800,\"cache_read_input_tokens\":30000,\"output_tokens\":400}}}\n";
        let c2 = "{\"type\":\"assistant\",\"message\":{\"id\":\"m2\",\"usage\":{\"input_tokens\":500,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":35000,\"output_tokens\":250}}}\n";
        fs::write(dir.join("sess.jsonl"), format!("{c1}{c1b}{c2}")).unwrap();
        env.add_proc(2100, "zsh\0", &sub, &[2101]);
        env.add_proc(2101, "node\0/usr/local/bin/claude\0", &sub, &[]);
        sub
    }

    fn seed_codex_fixture(env: &MockRemoteEnv) -> PathBuf {
        let repo = env.home.join("work/app");
        let wt = env.home.join("work/app-wt");
        let day = env.home.join(".codex/sessions/2026/11/11");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&wt).unwrap();
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("rollout-wt.jsonl"), format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n", wt.display())).unwrap();
        let hdr = format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n", repo.display());
        let ev = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":96000,\"cached_input_tokens\":72000,\"output_tokens\":3400,\"total_tokens\":99400},\"last_token_usage\":{\"input_tokens\":58000,\"cached_input_tokens\":50000,\"output_tokens\":1100,\"total_tokens\":59100},\"model_context_window\":200000}}}\n";
        fs::write(day.join("rollout-main.jsonl"), format!("{hdr}{ev}")).unwrap();
        env.add_proc(2200, "zsh\0", &repo, &[2201]);
        env.add_proc(2201, "/usr/local/bin/codex\0", &repo, &[]);
        repo
    }

    fn seed_agy_fd_fixture(env: &MockRemoteEnv) {
        let ws = env.home.join("work/agy-proj");
        let logs = env.home.join(".gemini/antigravity-cli/brain/c-99/.system_generated/logs");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&logs).unwrap();
        let tr = logs.join("transcript.jsonl");
        let l1 = "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":4500,\"cache_read_tokens\":20500,\"output_tokens\":600}\n";
        let l2 = "{\"step_index\":2,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":10000,\"cache_read_tokens\":74000,\"output_tokens\":900}\n";
        fs::write(&tr, format!("{l1}{l2}")).unwrap();
        let pdir = env.add_proc(2301, "/opt/antigravity/agy\0chat\0", &ws, &[]);
        let _ = symlink(&tr, pdir.join("fd/5"));
    }

    fn assert_batched_three_agents(snap: &HashMap<usize, RemoteTailer>) {
        assert_eq!(snap[&1].snapshot.source, TelemetrySource::Claude);
        assert_eq!(snap[&1].snapshot.effective_input(), 5_500);
        assert_eq!(snap[&1].snapshot.cache_tokens, 65_000);
        assert_eq!(snap[&1].snapshot.context_tokens, 35_500);
        assert_eq!(snap[&2].snapshot.source, TelemetrySource::Codex);
        assert_eq!(snap[&2].snapshot.context_tokens, 58_000);
        assert_eq!(snap[&2].snapshot.context_pct, Some(29));
        assert_eq!(snap[&3].snapshot.source, TelemetrySource::Agy);
        assert_eq!(snap[&3].snapshot.context_tokens, 84_900);
    }

    fn build_three_batch_targets(env: &MockRemoteEnv) -> Vec<RemoteSessionTarget> {
        let (claude_cwd, codex_cwd) = (seed_claude_fixture(env), seed_codex_fixture(env));
        seed_agy_fd_fixture(env);
        let panes = format!("claude-s|0|1|9999|zsh|/tmp\nclaude-s|1|1|2100|zsh|{}\ncodex-s|1|1|2200|zsh|{}\n", claude_cwd.display(), codex_cwd.display());
        fs::write(&env.tmux_panes_file, panes).unwrap();
        let s1 = dummy_session(1, SessionCategory::Remote, Tab::remote("gpu-box", Some("claude-s")), "gpu-box");
        let s2 = dummy_session(2, SessionCategory::Remote, Tab::remote("gpu-box", Some("codex-s")), "gpu-box");
        let s3 = dummy_session(3, SessionCategory::Project, Tab::local("shell"), "gpu-box");
        let mut targets: Vec<_> = [&s1, &s2, &s3].into_iter().filter_map(RemoteSessionTarget::from_session).collect();
        fs::write(env.proc_dir.join("2301/environ"), b"SSH_CONNECTION=127.0.0.1 50123 127.0.0.1 22\0").unwrap();
        targets[2].ports = Some((50123, 22));
        targets
    }

    #[test]
    fn mock_ssh_batches_claude_codex_and_agy_with_8s_host_ttl() {
        let env = MockRemoteEnv::new("batch3");
        let sched = RemoteTelemetryScheduler::new(REMOTE_PROBE_TTL);
        sched.sync_targets(&build_three_batch_targets(&env));
        let prog = env.ssh_bin.to_string_lossy().into_owned();
        let t0 = Instant::now();
        assert_eq!(sched.refresh_due_with(t0, |h, sc| run_ssh_probe_with_program(&prog, h, sc)), 3);
        assert_eq!(sched.refresh_due_with(t0 + Duration::from_secs(4), |h, sc| run_ssh_probe_with_program(&prog, h, sc)), 0);
        assert_eq!(env.call_count(), 1);
        assert_batched_three_agents(&sched.snapshot());
        assert_eq!(sched.refresh_due_with(t0 + Duration::from_secs(8), |h, sc| run_ssh_probe_with_program(&prog, h, sc)), 0);
        assert_eq!(env.call_count(), 2);
    }

    fn seed_agy_subagent_decoy(brain: &Path, ws: &Path) {
        let sub_reg = brain.join("w1/.system_generated/subagents");
        let sub_logs = brain.join("sub-newer/.system_generated/logs");
        fs::create_dir_all(&sub_reg).unwrap();
        fs::create_dir_all(&sub_logs).unwrap();
        fs::write(sub_reg.join("sub-newer.json"), "{}").unwrap();
        let sub_tr = sub_logs.join("transcript.jsonl");
        let body = format!(
            "{{\"type\":\"SYSTEM_MESSAGE\",\"content\":\"Workspace: {}\\n\"}}\n{{\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":2000,\"cache_read_tokens\":3000,\"output_tokens\":100}}\n",
            ws.display()
        );
        fs::write(&sub_tr, body).unwrap();
        let future = std::time::SystemTime::now() + Duration::from_secs(60);
        fs::OpenOptions::new().write(true).open(&sub_tr).unwrap().set_modified(future).unwrap();
    }

    fn seed_agy_workspace_fixture(env: &MockRemoteEnv, tricky_sess: &str) {
        let ws = env.home.join("proj-alpha");
        let brain = env.home.join(".gemini/antigravity-cli/brain");
        let logs = brain.join("w1/.system_generated/logs");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&logs).unwrap();
        let hdr = format!("{{\"type\":\"SYSTEM_MESSAGE\",\"content\":\"Workspace: {}\\n\"}}\n", ws.display());
        let turn = "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":12000,\"cache_read_tokens\":112000,\"output_tokens\":800}\n";
        fs::write(logs.join("transcript.jsonl"), format!("{hdr}{turn}")).unwrap();
        seed_agy_subagent_decoy(&brain, &ws);
        env.add_proc(3100, "agy\0", &ws, &[]);
        fs::write(&env.tmux_panes_file, format!("{tricky_sess}|1|1|3100|agy|{}\n", ws.display())).unwrap();
    }

    #[test]
    fn shell_injection_session_names_stay_inert_and_resolve_agy_workspace() {
        let env = MockRemoteEnv::new("escape");
        let pwn = env.root.join("pwned_marker");
        let tricky_sess = format!("dev'$(touch {})", pwn.display());
        seed_agy_workspace_fixture(&env, &tricky_sess);
        let mut s = dummy_session(7, SessionCategory::Project, Tab::local("shell"), "gpu-box");
        s.remote_tmux = Some((tricky_sess, "0".into(), "agy".into(), format!("'$(touch {})", pwn.display())));
        let sched = RemoteTelemetryScheduler::default();
        sched.sync_targets(&[RemoteSessionTarget::from_session(&s).unwrap()]);
        let prog = env.ssh_bin.to_string_lossy().into_owned();
        assert_eq!(sched.refresh_due_with(Instant::now(), |h, sc| run_ssh_probe_with_program(&prog, h, sc)), 1);
        assert!(!pwn.exists());
        assert_eq!(sched.snapshot()[&7].snapshot.context_tokens, 124_800);
    }

    fn write_large_codex_rollout(path: &Path, cwd: &Path) -> u64 {
        let mut f = fs::File::create(path).unwrap();
        writeln!(f, "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\",\"model_context_window\":200000}}}}", cwd.display()).unwrap();
        let filler = format!("{{\"type\":\"note\",\"pad\":\"{}\"}}\n", "x".repeat(900));
        for _ in 0..340 {
            f.write_all(filler.as_bytes()).unwrap();
        }
        let tail_ev = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":210000,\"cached_input_tokens\":160000,\"output_tokens\":9000,\"total_tokens\":219000},\"last_token_usage\":{\"input_tokens\":154000,\"cached_input_tokens\":130000,\"output_tokens\":1200,\"total_tokens\":155200}}}}\n";
        f.write_all(tail_ev.as_bytes()).unwrap();
        f.metadata().unwrap().len()
    }

    fn run_measured_probe(sched: &RemoteTelemetryScheduler, prog: &str, t: Instant) -> (usize, usize) {
        let mut byte_len = 0usize;
        let updated = sched.refresh_due_with(t, |h, sc| {
            let out = run_ssh_probe_with_program(prog, h, sc)?;
            byte_len = out.len();
            Some(out)
        });
        (updated, byte_len)
    }

    fn verify_delta_and_truncation(sched: &RemoteTelemetryScheduler, prog: &str, rollout: &Path, repo: &Path, t0: Instant) {
        let next_turn = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":260000,\"cached_input_tokens\":200000,\"output_tokens\":10500,\"total_tokens\":270500},\"last_token_usage\":{\"input_tokens\":186000,\"cached_input_tokens\":160000,\"output_tokens\":1500,\"total_tokens\":187500}}}}\n";
        fs::OpenOptions::new().append(true).open(rollout).unwrap().write_all(next_turn.as_bytes()).unwrap();
        let (u2, sz2) = run_measured_probe(sched, prog, t0 + Duration::from_secs(18));
        assert_eq!(u2, 1);
        assert!(sz2 < 500, "delta poll transferred {sz2} bytes");
        assert_eq!(sched.snapshot()[&9].snapshot.context_pct, Some(93));
        assert_eq!(sched.snapshot()[&9].snapshot.alert_level(), ContextAlert::High);

        let rotated = format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\",\"model_context_window\":200000}}}}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"last_token_usage\":{{\"input_tokens\":12000,\"cached_input_tokens\":0,\"output_tokens\":200,\"total_tokens\":12200}},\"total_token_usage\":{{\"input_tokens\":12000,\"cached_input_tokens\":0,\"output_tokens\":200,\"total_tokens\":12200}}}}}}}}\n", repo.display());
        fs::write(rollout, &rotated).unwrap();
        assert_eq!(run_measured_probe(sched, prog, t0 + Duration::from_secs(27)).0, 1);
        assert_eq!(sched.snapshot()[&9].remote_offset, rotated.len() as u64);
        assert_eq!(sched.snapshot()[&9].snapshot.context_tokens, 12_000);
    }

    fn setup_large_codex_fixture(env: &MockRemoteEnv) -> (PathBuf, PathBuf, u64) {
        let repo = env.home.join("work/big");
        let day = env.home.join(".codex/sessions/2026/11/11");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&day).unwrap();
        let rollout = day.join("rollout-big.jsonl");
        let initial_sz = write_large_codex_rollout(&rollout, &repo);
        assert!(initial_sz > REMOTE_FULL_MAX_BYTES);
        env.add_proc(4100, "codex\0", &repo, &[]);
        (repo, rollout, initial_sz)
    }

    #[test]
    fn large_transcript_uses_bounded_head_tail_zero_byte_noop_delta_and_truncate() {
        let env = MockRemoteEnv::new("large");
        let (repo, rollout, initial_sz) = setup_large_codex_fixture(&env);
        let s = dummy_session(9, SessionCategory::Remote, Tab::remote("gpu-box", None), "gpu-box");
        let sched = RemoteTelemetryScheduler::new(REMOTE_PROBE_TTL);
        let mut target = RemoteSessionTarget::from_session(&s).unwrap();
        target.ports = Some((50123, 22));
        fs::write(env.proc_dir.join("4100/environ"), b"SSH_CONNECTION=127.0.0.1 50123 127.0.0.1 22\0").unwrap();
        sched.sync_targets(&[target]);
        let prog = env.ssh_bin.to_string_lossy().into_owned();
        let t0 = Instant::now();

        let (u0, sz0) = run_measured_probe(&sched, &prog, t0);
        assert!(u0 == 1 && sz0 < 165_000, "initial transfer {sz0} bytes");
        assert_eq!(sched.snapshot()[&9].remote_offset, initial_sz);
        assert_eq!(sched.snapshot()[&9].snapshot.context_pct, Some(77));
        let (u1, sz1) = run_measured_probe(&sched, &prog, t0 + Duration::from_secs(9));
        assert!(u1 == 0 && sz1 < 240);
        verify_delta_and_truncation(&sched, &prog, &rollout, &repo, t0);
    }

    fn set_file_mtime_offset(path: &Path, plus_secs: u64) {
        let stamp = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_762_000_000 + plus_secs);
        fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(stamp).unwrap();
    }

    pub(crate) fn seed_codex_0167_tmux_fixture(env: &MockRemoteEnv) -> PathBuf {
        let repo = env.home.join("Development/Akhcheck");
        let day = env.home.join(".codex/sessions/2026/11/03");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&day).unwrap();
        let prompt_pad = format!("subagent instructions {}", "p".repeat(68_000));
        let main_rollout = day.join("rollout-main-154k.jsonl");
        let main_hdr = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\",\"source\":\"cli\",\"model_context_window\":258400,\"base_instructions\":\"{}\"}}}}\n",
            repo.display(),
            prompt_pad
        );
        let main_tok = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":420000,\"cached_input_tokens\":380000,\"output_tokens\":14000,\"total_tokens\":434000},\"last_token_usage\":{\"input_tokens\":154000,\"cached_input_tokens\":140000,\"output_tokens\":1200,\"total_tokens\":155200},\"model_context_window\":258400}}}\n";
        fs::write(&main_rollout, format!("{main_hdr}{main_tok}")).unwrap();
        set_file_mtime_offset(&main_rollout, 10);

        let guardian = day.join("rollout-guardian-41k.jsonl");
        let g_hdr = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\",\"source\":{{\"subagent\":{{\"other\":\"guardian\"}}}}}}}}\n",
            repo.display()
        );
        let g_tok = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":41000,\"cached_input_tokens\":0,\"output_tokens\":500,\"total_tokens\":41500},\"last_token_usage\":{\"input_tokens\":41000,\"cached_input_tokens\":0,\"output_tokens\":500,\"total_tokens\":41500},\"model_context_window\":258400}}}\n";
        fs::write(&guardian, format!("{g_hdr}{g_tok}")).unwrap();
        set_file_mtime_offset(&guardian, 40);

        let empty_new = day.join("rollout-empty-new.jsonl");
        fs::write(&empty_new, &main_hdr).unwrap();
        set_file_mtime_offset(&empty_new, 70);
        main_rollout
    }

    #[test]
    fn codex_0167_tmux_resolves_large_header_and_skips_guardian_subagents() {
        let env = MockRemoteEnv::new("codex0167");
        let main_rollout = seed_codex_0167_tmux_fixture(&env);
        let repo = env.home.join("Development/Akhcheck");
        env.add_proc(3065, "-zsh\0", &repo, &[2696]);
        let codex_pdir = env.add_proc(2696, "codex\0resume\0", &repo, &[]);
        let _ = symlink(env.home.join(".codex/logs_2.sqlite"), codex_pdir.join("fd/9"));
        fs::write(&env.tmux_panes_file, format!("akhcheck|1|1|3065|codex|{}\n", repo.display())).unwrap();

        let mut s = dummy_session(15, SessionCategory::Remote, Tab::remote("home", Some("akhcheck")), "home");
        s.remote_tmux = Some(("akhcheck".into(), "0".into(), "codex".into(), repo.to_string_lossy().into_owned()));
        let sched = RemoteTelemetryScheduler::default();
        sched.sync_targets(&[RemoteSessionTarget::from_session(&s).unwrap()]);
        let prog = env.ssh_bin.to_string_lossy().into_owned();
        assert_eq!(sched.refresh_due_with(Instant::now(), |h, sc| run_ssh_probe_with_program(&prog, h, sc)), 1);
        let rt = &sched.snapshot()[&15];
        assert_eq!(Path::new(&rt.remote_path), main_rollout);
        assert_eq!(rt.snapshot.context_tokens, 154_000);
        assert_eq!(rt.snapshot.context_pct, Some(59));
    }

    fn make_remote_claude_tailer(target: &RemoteSessionTarget, ctx_read: u64) -> RemoteTailer {
        let mut rt = RemoteTailer::for_target(target);
        let body = format!("{{\"type\":\"assistant\",\"message\":{{\"id\":\"m9\",\"usage\":{{\"input_tokens\":8000,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":{ctx_read},\"output_tokens\":900}}}}}}\n").into_bytes();
        rt.apply_frame(&RemoteProbeFrame {
            sid: target.sid, mode: FrameMode::Full, kind: TelemetrySource::Claude,
            size: body.len() as u64, mtime: 1, path: "/remote/.claude/projects/-srv/s.jsonl".into(),
            head: Vec::new(), body,
        });
        rt
    }

    #[test]
    fn batchmode_failure_falls_back_to_vte_ratio_and_yields_to_remote_jsonl() {
        let vte_line = "\x1b[38;5;244msonnet · 84.5k / 200k · $0.42\x1b[0m\n";
        let mut s = dummy_session(11, SessionCategory::Remote, Tab::remote("locked-box", Some("main")), "locked-box");
        s.telemetry.vte_fallback = parse_vte_status_fallback(vte_line).unwrap();
        let target = RemoteSessionTarget::from_session(&s).unwrap();
        let sched = RemoteTelemetryScheduler::default();
        sched.sync_targets(std::slice::from_ref(&target));
        assert_eq!(sched.refresh_due_with(Instant::now(), |_, _| None), 0);

        let (_, telem1) = run_poll_job_with_remote(SessionPollJob::from_session(&s), sched.snapshot().remove(&11), Path::new("/tmp"), Instant::now());
        assert_eq!(telem1.snapshot().source, TelemetrySource::VteFallback);
        assert_eq!(telem1.snapshot().context_tokens, 84_500);
        assert_eq!(telem1.snapshot().context_pct, Some(42));
        assert_eq!(telem1.snapshot().cost_usd, Some(0.42));

        s.telemetry = telem1;
        let rt_live = make_remote_claude_tailer(&target, 140_000);
        let (_, telem2) = run_poll_job_with_remote(SessionPollJob::from_session(&s), Some(rt_live), Path::new("/tmp"), Instant::now());
        assert_eq!(telem2.snapshot().source, TelemetrySource::Claude);
        assert_eq!(telem2.snapshot().context_tokens, 148_000);
        assert_eq!(telem2.snapshot().alert_level(), ContextAlert::Warn);
    }

    extern "C" fn noop_click(_w: *mut GtkWidget, _d: *mut c_void) {}
    extern "C" fn noop_event(_w: *mut GtkWidget, _e: *const GdkEvent, _d: *mut c_void) -> c_int { 0 }

    fn has_css_class(widget: *mut GtkWidget, cls: &str) -> bool {
        let c = CString::new(cls).unwrap();
        unsafe { gtk_style_context_has_class(gtk_widget_get_style_context(widget), c.as_ptr()) != 0 }
    }

    fn widget_tooltip_str(widget: *mut GtkWidget) -> String {
        let mut raw: *mut c_char = ptr::null_mut();
        unsafe {
            g_object_get(widget as *mut c_void, c"tooltip-text".as_ptr(), &mut raw, ptr::null::<c_char>());
            if raw.is_null() {
                return String::new();
            }
            let text = CStr::from_ptr(raw).to_string_lossy().into_owned();
            g_free(raw as *mut c_void);
            text
        }
    }

    fn gtk_label_str(label: *mut GtkWidget) -> String {
        unsafe { CStr::from_ptr(gtk_label_get_text(label)).to_string_lossy().into_owned() }
    }

    fn gtk_button_str(btn: *mut GtkWidget) -> String {
        unsafe { CStr::from_ptr(gtk_button_get_label(btn)).to_string_lossy().into_owned() }
    }

    fn gtk_name_str(widget: *mut GtkWidget) -> String {
        unsafe { CStr::from_ptr(gtk_widget_get_name(widget)).to_string_lossy().into_owned() }
    }

    fn make_gtk_remote_session(
        sidebar: &Sidebar,
        strip: &TabStrip,
        sid: usize,
        title: &str,
        cat: SessionCategory,
        tab: Tab,
        host: &str,
    ) -> TermSession {
        let row = sidebar.append_tab(cat, sid, noop_click, noop_event);
        let handlers = TabHandlers {
            on_select: noop_click, on_close: noop_click,
            on_press: noop_event, on_motion: noop_event, on_release: noop_event,
        };
        let top_tab = strip.append(sid, cat, handlers);
        build_session_with_views(sid, title, cat, tab, host, row, top_tab)
    }

    fn assert_single_remote_row(s: &TermSession, badge: &str, cls: &str, agent_id: &str) {
        assert_eq!(gtk_label_str(s.sidebar_row.metric_widget()), badge);
        assert!(has_css_class(s.sidebar_row.metric_widget(), cls));
        assert_eq!(gtk_name_str(s.sidebar_row.icon_widget()), agent_id);
        assert_eq!(gtk_name_str(s.top_tab.icon_widget()), agent_id);
        assert!(widget_tooltip_str(s.sidebar_row.row_widget()).contains(badge));
        assert!(widget_tooltip_str(s.top_tab.select_button()).contains(badge));
    }

    fn assert_remote_gtk_badges_and_menu(sessions: &[TermSession], menu: TokensMenu) {
        assert_single_remote_row(&sessions[0], "ctx 148k", "sidebar-metric-warn", "agent-claude");
        assert_single_remote_row(&sessions[1], "ctx 186k", "sidebar-metric-high", "agent-codex");
        let view = build_sessions_tokens_view(sessions, sessions[1].id, ThemeMode::Dark, Instant::now());
        menu.apply(&view);
        assert_eq!(gtk_button_str(menu.button_widget()), "Токены · 186k · cache 86% ▾");
        assert!(has_css_class(menu.button_widget(), "btn-mi-c"));
        let markup = view.markup.as_deref().unwrap_or("");
        assert!(markup.contains("ctx 148k") && markup.contains("ctx 186k (93%)") && markup.contains("Итого по окну (2)"));
    }

    fn populate_gtk_remote_sessions(sidebar: &Sidebar, strip: &TabStrip) -> Vec<TermSession> {
        let mut s_rem = make_gtk_remote_session(sidebar, strip, 1, "gpu-box · main", SessionCategory::Remote, Tab::remote("gpu-box", Some("main")), "gpu-box");
        let mut s_dyn = make_gtk_remote_session(sidebar, strip, 2, "ssh gpu-box", SessionCategory::Project, Tab::local("shell"), "gpu-box");
        let rt_claude = make_remote_claude_tailer(&RemoteSessionTarget::from_session(&s_rem).unwrap(), 140_000);
        let mut rt_codex = RemoteTailer::for_target(&RemoteSessionTarget::from_session(&s_dyn).unwrap());
        let cbody = b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":260000,\"cached_input_tokens\":200000,\"output_tokens\":10500,\"total_tokens\":270500},\"last_token_usage\":{\"input_tokens\":186000,\"cached_input_tokens\":160000,\"output_tokens\":1500,\"total_tokens\":187500},\"model_context_window\":200000}}}\n".to_vec();
        rt_codex.apply_frame(&RemoteProbeFrame { sid: 2, mode: FrameMode::Full, kind: TelemetrySource::Codex, size: cbody.len() as u64, mtime: 2, path: "/r/rollout.jsonl".into(), head: Vec::new(), body: cbody });
        s_rem.telemetry = run_poll_job_with_remote(SessionPollJob::from_session(&s_rem), Some(rt_claude), Path::new("/tmp"), Instant::now()).1;
        s_dyn.telemetry = run_poll_job_with_remote(SessionPollJob::from_session(&s_dyn), Some(rt_codex), Path::new("/tmp"), Instant::now()).1;
        s_rem.sidebar_row.update(0, &s_rem, Some((61.4, "B")), "+3 -1", false);
        s_rem.top_tab.update(&s_rem, false);
        s_dyn.sidebar_row.update(1, &s_dyn, Some((61.4, "B")), "+3 -1", true);
        s_dyn.top_tab.update(&s_dyn, true);
        vec![s_rem, s_dyn]
    }

    #[test]
    fn headless_broadway_remote_sidebar_ctx_badges_tooltips_and_tokens_menu() {
        with_headless_gtk(|| {
            let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
            let root = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 0) };
            unsafe {
                gtk_style_context_add_class(gtk_widget_get_style_context(win), c"akhterm-window".as_ptr());
                gtk_container_add(win, root);
            }
            let strip = TabStrip::new(root, noop_click);
            let sidebar = Sidebar::new(root, noop_click);
            let menu = TokensMenu::new(root, noop_click);
            unsafe { gtk_widget_show_all(win) };

            let sessions = populate_gtk_remote_sessions(&sidebar, &strip);
            pump_gtk_events();
            assert_remote_gtk_badges_and_menu(&sessions, menu);

            for s in sessions {
                s.sidebar_row.destroy();
                s.top_tab.destroy();
            }
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
        });
    }
}
