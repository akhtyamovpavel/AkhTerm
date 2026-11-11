//! Incremental O(1) agent token/context telemetry, VTE status-bar fallback, and turn timer UI.

use std::ffi::{c_void, CString};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::ptr;
use std::time::{Duration, Instant};

use crate::approval::read_vte_visible_text;
use crate::connect_click;
use crate::ffi::*;
use crate::session::{SessionCategory, TermSession};
pub use crate::telemetry_agy::parse_vte_context_panel;
use crate::telemetry_agy::{
    discover_agy_by_fd, discover_agy_transcript, is_vte_breakdown_line, metadata_stamp,
    trim_trailing_slash,
};
use crate::theme_mode::{ThemeMode, TokensMarkupColors};
use crate::ui_text as text;

pub type ClickHandler = extern "C" fn(*mut GtkWidget, *mut c_void);

pub const VTE_FALLBACK_LINES: usize = 4;
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
    VteFallback,
}

impl TelemetrySource {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Agy => "AGY",
            Self::Claude => "Claude",
            Self::Codex => "Codex",
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
            return Some(format!("ctx {}", format_compact_tokens(self.context_tokens)));
        }
        self.context_pct.map(|pct| format!("ctx {pct}%"))
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
    fn reset_for(&mut self, path: PathBuf, kind: TelemetrySource) {
        self.path = Some(path);
        self.kind = kind;
        self.mtime_len = (0, 0);
        self.reset_after_truncate();
    }

    fn reset_after_truncate(&mut self) {
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

    fn consume_line(&mut self, line: &str) {
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
    pub vte_fallback: TokenSnapshot,
    pub vte_panel_override: Option<(u64, Option<u8>, u64)>,
    effective: TokenSnapshot,
    last_discovery: Option<Instant>,
}

impl SessionTelemetry {
    pub fn snapshot(&self) -> &TokenSnapshot {
        if self.vte_panel_override.is_some() {
            &self.effective
        } else if self.tailer.snapshot.has_data() {
            &self.tailer.snapshot
        } else {
            &self.vte_fallback
        }
    }

    pub fn effective_snapshot(&self) -> TokenSnapshot {
        let mut snap = if self.tailer.snapshot.has_data() {
            self.tailer.snapshot.clone()
        } else {
            self.vte_fallback.clone()
        };
        if let Some((toks, pct, captured_offset)) = self.vte_panel_override {
            if self.tailer.offset <= captured_offset {
                apply_panel_override(&mut snap, toks, pct);
            }
        }
        snap
    }

    pub fn sync_effective(&mut self) {
        self.effective = self.effective_snapshot();
    }

    pub fn observe_vte_text(&mut self, visible: &str) {
        if let Some((toks, pct)) = parse_vte_context_panel(visible) {
            self.record_panel_hit(toks, pct);
        } else {
            self.record_status_fallback(visible);
        }
        self.sync_effective();
    }

    fn record_panel_hit(&mut self, toks: u64, pct: Option<u8>) {
        self.vte_panel_override = Some((toks, pct, self.tailer.offset));
        if !self.tailer.snapshot.has_data() {
            self.vte_fallback = TokenSnapshot {
                source: TelemetrySource::VteFallback,
                context_tokens: toks,
                context_pct: pct,
                ..Default::default()
            };
        }
    }

    fn record_status_fallback(&mut self, visible: &str) {
        if !self.tailer.snapshot.has_data() {
            if let Some(parsed) = parse_vte_status_fallback(visible) {
                self.vte_fallback = parsed;
                self.vte_panel_override = None;
            }
        }
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
    pub agent: String,
    pub is_daemon: bool,
    pub telemetry: SessionTelemetry,
}

impl SessionPollJob {
    pub fn from_session(s: &TermSession) -> Self {
        Self {
            sid: s.id,
            root_pid: s.active_pid(),
            vte: s.active_vte(),
            local_cwd: s.active_local_cwd(),
            has_ssh: !s.proc_info.ssh_host.is_empty(),
            has_tmux: !s.proc_info.has_tmux.is_empty(),
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

pub fn run_poll_job(mut job: SessionPollJob, home: &Path, now: Instant) -> (usize, SessionTelemetry) {
    if job.is_daemon {
        return (job.sid, job.telemetry);
    }
    let local_cwd = (!has_live_ssh(&job)).then_some(job.local_cwd.as_deref()).flatten();
    if let Some(cwd) = local_cwd {
        refresh_local_jsonl(&mut job.telemetry, home, cwd, job.root_pid, &job.agent, now);
    } else {
        reset_local_tailer(&mut job.telemetry);
    }
    if should_poll_vte_fallback(local_cwd, job.has_tmux, &job.agent) {
        refresh_vte_telemetry(&mut job.telemetry, job.vte);
    } else {
        job.telemetry.vte_fallback = TokenSnapshot::default();
        job.telemetry.vte_panel_override = None;
    }
    job.telemetry.sync_effective();
    (job.sid, job.telemetry)
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

fn refresh_local_jsonl(
    telem: &mut SessionTelemetry,
    home: &Path,
    cwd: &str,
    root_pid: i32,
    agent: &str,
    now: Instant,
) {
    let prev_path = telem.tailer.path.clone();
    if telem.should_rediscover(now) {
        telem.last_discovery = Some(now);
        if let Some((path, kind)) = discover_transcript(home, cwd, root_pid, agent) {
            let _ = telem.tailer.poll_file(&path, kind);
            invalidate_panel_override_if_advanced(telem, prev_path.as_deref());
            return;
        }
    }
    if let Some(path) = telem.tailer.path.clone() {
        let kind = telem.tailer.kind;
        let _ = telem.tailer.poll_file(&path, kind);
    }
    invalidate_panel_override_if_advanced(telem, prev_path.as_deref());
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
    if low.contains("agy") || low.contains("antigravity") {
        return discover_agy_transcript(home, norm_cwd, root_pid).map(|p| (p, TelemetrySource::Agy));
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

fn read_codex_header_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file.take(16_384));
    let mut line = String::new();
    for _ in 0..5 {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        if let Some(dir) = extract_header_cwd(&line) {
            return Some(dir);
        }
    }
    None
}

fn extract_header_cwd(line: &str) -> Option<String> {
    let raw = json_str(line, "cwd")
        .or_else(|| json_obj(line, "payload").and_then(|p| json_str(p, "cwd")))?;
    (!raw.is_empty()).then(|| trim_trailing_slash(&raw).to_owned())
}

fn discover_codex_transcript(home: &Path, cwd: &str) -> Option<PathBuf> {
    let root = home.join(".codex/sessions");
    if !root.is_dir() {
        return None;
    }
    let mut files = Vec::new();
    collect_jsonl_recursive(&root, 0, &mut files);
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.truncate(24);
    match_codex_candidates(&files, cwd)
}

fn match_codex_candidates(files: &[(u64, PathBuf)], cwd: &str) -> Option<PathBuf> {
    let cwd_path = Path::new(cwd);
    let mut sub_match = None;
    for (_, path) in files {
        let Some(hdr_cwd) = read_codex_header_cwd(path) else { continue };
        if hdr_cwd == cwd {
            return Some(path.clone());
        }
        if sub_match.is_none() && cwd_path.starts_with(Path::new(&hdr_cwd)) {
            sub_match = Some(path.clone());
        }
    }
    sub_match
}

fn skip_ws(bytes: &[u8], mut idx: usize) -> usize {
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

fn skip_json_string(bytes: &[u8], open_quote: usize) -> Option<usize> {
    if bytes.get(open_quote) != Some(&b'"') {
        return None;
    }
    let mut idx = open_quote + 1;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\\' => idx += 2,
            b'"' => return Some(idx + 1),
            _ => idx += 1,
        }
    }
    None
}

fn find_matching_delim(bytes: &[u8], start: usize, open_b: u8, close_b: u8) -> Option<usize> {
    if bytes.get(start) != Some(&open_b) {
        return None;
    }
    let mut depth = 1usize;
    let mut idx = start + 1;
    while idx < bytes.len() {
        idx = step_delim_scan(bytes, idx, open_b, close_b, &mut depth)?;
        if depth == 0 {
            return Some(idx);
        }
    }
    None
}

fn step_delim_scan(
    bytes: &[u8],
    idx: usize,
    open_b: u8,
    close_b: u8,
    depth: &mut usize,
) -> Option<usize> {
    match bytes[idx] {
        b'"' => skip_json_string(bytes, idx),
        b if b == open_b => {
            *depth += 1;
            Some(idx + 1)
        }
        b if b == close_b => {
            *depth -= 1;
            Some(idx + 1)
        }
        _ => Some(idx + 1),
    }
}

fn is_complete_json_object(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return false;
    }
    find_matching_delim(trimmed.as_bytes(), 0, b'{', b'}') == Some(trimmed.len())
}

fn skip_json_value(bytes: &[u8], start: usize) -> Option<usize> {
    match *bytes.get(start)? {
        b'"' => skip_json_string(bytes, start),
        b'{' => find_matching_delim(bytes, start, b'{', b'}'),
        b'[' => find_matching_delim(bytes, start, b'[', b']'),
        _ => Some(skip_json_scalar(bytes, start)),
    }
}

fn skip_json_scalar(bytes: &[u8], mut idx: usize) -> usize {
    while idx < bytes.len() && !matches!(bytes[idx], b',' | b'}' | b']') {
        idx += 1;
    }
    idx
}

fn unescape_json_str(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            push_escaped_char(&mut chars, &mut out);
        } else {
            out.push(ch);
        }
    }
    out
}

fn push_escaped_char(chars: &mut std::str::Chars<'_>, out: &mut String) {
    match chars.next() {
        Some('n') => out.push('\n'),
        Some('r') => out.push('\r'),
        Some('t') => out.push('\t'),
        Some('u') => {
            let hex: String = chars.by_ref().take(4).collect();
            if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                out.push(c);
            }
        }
        Some(other) => out.push(other),
        None => {}
    }
}

fn parse_direct_kv<'a>(trimmed: &'a str, idx: usize) -> Option<(&'a str, &'a str, usize)> {
    let bytes = trimmed.as_bytes();
    let key_end = skip_json_string(bytes, idx)?;
    let colon = skip_ws(bytes, key_end);
    if bytes.get(colon) != Some(&b':') {
        return None;
    }
    let val_start = skip_ws(bytes, colon + 1);
    let val_end = skip_json_value(bytes, val_start)?;
    Some((&trimmed[idx + 1..key_end - 1], trimmed[val_start..val_end].trim(), val_end))
}

fn json_direct_value<'a>(obj: &'a str, target_key: &str) -> Option<&'a str> {
    let trimmed = obj.trim();
    let bytes = trimmed.as_bytes();
    if bytes.first() != Some(&b'{') {
        return None;
    }
    let mut idx = 1;
    while idx < bytes.len() {
        idx = skip_ws(bytes, idx);
        match bytes.get(idx) {
            None | Some(b'}') => break,
            Some(b',') => idx += 1,
            _ => {
                let (key, val, next_idx) = parse_direct_kv(trimmed, idx)?;
                if key == target_key {
                    return Some(val);
                }
                idx = next_idx;
            }
        }
    }
    None
}

fn json_str(obj: &str, key: &str) -> Option<String> {
    let val = json_direct_value(obj, key)?;
    let inner = val.strip_prefix('"')?.strip_suffix('"')?;
    Some(unescape_json_str(inner))
}

fn json_u64(obj: &str, key: &str) -> Option<u64> {
    let val = json_direct_value(obj, key)?;
    let digits: String = val.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| digits.parse::<u64>().ok()).flatten()
}

fn json_obj<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    json_direct_value(obj, key).filter(|v| v.starts_with('{'))
}

pub fn apply_jsonl_line(
    kind: TelemetrySource,
    line: &str,
    snap: &mut TokenSnapshot,
    last_msg_id: &mut String,
    last_msg_usage: &mut (u64, u64, u64, u64),
) {
    match kind {
        TelemetrySource::Agy => apply_agy_line(line, snap),
        TelemetrySource::Claude => apply_claude_line(line, snap, last_msg_id, last_msg_usage),
        TelemetrySource::Codex => apply_codex_line(line, snap),
        TelemetrySource::None | TelemetrySource::VteFallback => {}
    }
}

fn accumulate_agy_usage(snap: &mut TokenSnapshot, in_tok: u64, cache_tok: u64, out_tok: u64) -> bool {
    if in_tok.saturating_add(cache_tok).saturating_add(out_tok) == 0 {
        return false;
    }
    snap.source = TelemetrySource::Agy;
    snap.input_tokens = snap.input_tokens.saturating_add(in_tok);
    snap.cache_tokens = snap.cache_tokens.saturating_add(cache_tok);
    snap.output_tokens = snap.output_tokens.saturating_add(out_tok);
    true
}

fn apply_agy_line(line: &str, snap: &mut TokenSnapshot) {
    let in_tok = json_u64(line, "input_tokens").unwrap_or(0);
    let cache_tok = json_u64(line, "cache_read_tokens").unwrap_or(0);
    let out_tok = json_u64(line, "output_tokens").unwrap_or(0);
    match json_str(line, "type").as_deref() {
        Some("PLANNER_RESPONSE") => {
            if accumulate_agy_usage(snap, in_tok, cache_tok, out_tok) {
                let prompt = in_tok.saturating_add(cache_tok);
                if prompt > 0 {
                    snap.context_tokens = prompt.saturating_add(out_tok);
                }
            }
        }
        Some("CHECKPOINT") => {
            let _ = accumulate_agy_usage(snap, in_tok, cache_tok, out_tok);
            if out_tok > 0 {
                snap.context_tokens = out_tok;
            }
            snap.context_pct = None;
        }
        _ => {}
    }
}

fn extract_claude_usage(line: &str) -> Option<(String, (u64, u64, u64, u64))> {
    let msg = json_obj(line, "message");
    let usage = msg.and_then(|m| json_obj(m, "usage")).or_else(|| json_obj(line, "usage"))?;
    let tuple = (
        json_u64(usage, "input_tokens").unwrap_or(0),
        json_u64(usage, "cache_creation_input_tokens").unwrap_or(0),
        json_u64(usage, "cache_read_input_tokens").unwrap_or(0),
        json_u64(usage, "output_tokens").unwrap_or(0),
    );
    if tuple == (0, 0, 0, 0) {
        return None;
    }
    let msg_id = msg.and_then(|m| json_str(m, "id")).unwrap_or_default();
    Some((msg_id, tuple))
}

fn apply_claude_line(
    line: &str,
    snap: &mut TokenSnapshot,
    last_msg_id: &mut String,
    last_msg_usage: &mut (u64, u64, u64, u64),
) {
    let Some((msg_id, cur)) = extract_claude_usage(line) else { return };
    snap.source = TelemetrySource::Claude;
    if !msg_id.is_empty() && msg_id == *last_msg_id {
        replace_claude_turn(snap, *last_msg_usage, cur);
        *last_msg_usage = max_tuple4(*last_msg_usage, cur);
    } else {
        add_claude_turn(snap, cur);
        *last_msg_id = msg_id;
        *last_msg_usage = cur;
    }
}

fn max_tuple4(a: (u64, u64, u64, u64), b: (u64, u64, u64, u64)) -> (u64, u64, u64, u64) {
    (a.0.max(b.0), a.1.max(b.1), a.2.max(b.2), a.3.max(b.3))
}

fn add_claude_turn(snap: &mut TokenSnapshot, (in_t, cw_t, cr_t, out_t): (u64, u64, u64, u64)) {
    snap.input_tokens = snap.input_tokens.saturating_add(in_t);
    snap.cache_write_tokens = snap.cache_write_tokens.saturating_add(cw_t);
    snap.cache_tokens = snap.cache_tokens.saturating_add(cr_t);
    snap.output_tokens = snap.output_tokens.saturating_add(out_t);
    let ctx = in_t.saturating_add(cw_t).saturating_add(cr_t);
    if ctx > 0 {
        snap.context_tokens = ctx;
    }
}

fn replace_claude_turn(
    snap: &mut TokenSnapshot,
    prev: (u64, u64, u64, u64),
    next: (u64, u64, u64, u64),
) {
    let merged = max_tuple4(prev, next);
    snap.input_tokens = snap.input_tokens.saturating_sub(prev.0).saturating_add(merged.0);
    snap.cache_write_tokens = snap.cache_write_tokens.saturating_sub(prev.1).saturating_add(merged.1);
    snap.cache_tokens = snap.cache_tokens.saturating_sub(prev.2).saturating_add(merged.2);
    snap.output_tokens = snap.output_tokens.saturating_sub(prev.3).saturating_add(merged.3);
    let ctx = merged.0.saturating_add(merged.1).saturating_add(merged.2);
    if ctx > 0 {
        snap.context_tokens = ctx;
    }
}

struct CodexUsage {
    uncached_in: u64,
    cached_in: u64,
    output: u64,
    context: u64,
}

fn parse_codex_usage_obj(obj: &str) -> Option<CodexUsage> {
    let in_raw = json_u64(obj, "input_tokens").unwrap_or(0);
    let cached = json_u64(obj, "cached_input_tokens").unwrap_or(0);
    let output = json_u64(obj, "output_tokens").unwrap_or(0);
    let total = json_u64(obj, "total_tokens");
    if in_raw.saturating_add(cached).saturating_add(output).saturating_add(total.unwrap_or(0)) == 0 {
        return None;
    }
    let disjoint = is_codex_cached_disjoint(in_raw, cached, output, total);
    let (uncached_in, context) = if disjoint {
        (in_raw, in_raw.saturating_add(cached))
    } else {
        (in_raw.saturating_sub(cached), in_raw)
    };
    Some(CodexUsage { uncached_in, cached_in: cached, output, context })
}

fn is_codex_cached_disjoint(in_raw: u64, cached: u64, output: u64, total: Option<u64>) -> bool {
    match total {
        Some(tot) => cached > 0 && tot >= in_raw.saturating_add(cached).saturating_add(output),
        None => in_raw < cached,
    }
}

fn find_codex_token_count(line: &str) -> Option<&str> {
    match json_str(line, "type").as_deref() {
        Some("token_count") => Some(line),
        Some("event_msg") => {
            json_obj(line, "payload").filter(|p| json_str(p, "type").as_deref() == Some("token_count"))
        }
        _ => None,
    }
}

fn apply_codex_line(line: &str, snap: &mut TokenSnapshot) {
    let Some(tc_obj) = find_codex_token_count(line) else { return };
    let info = json_obj(tc_obj, "info").unwrap_or(tc_obj);
    if let Some(tot_obj) = json_obj(info, "total_token_usage") {
        apply_codex_total_info(info, tot_obj, snap);
        return;
    }
    if let Some(step) = parse_codex_usage_obj(info) {
        apply_codex_step(step, snap);
    }
}

fn apply_codex_total_info(info: &str, tot_obj: &str, snap: &mut TokenSnapshot) {
    let Some(tot) = parse_codex_usage_obj(tot_obj) else { return };
    snap.source = TelemetrySource::Codex;
    snap.input_tokens = tot.uncached_in;
    snap.cache_tokens = tot.cached_in;
    snap.output_tokens = tot.output;
    let last_ctx = json_obj(info, "last_token_usage")
        .and_then(parse_codex_usage_obj)
        .map_or(tot.context, |u| u.context);
    if last_ctx > 0 {
        snap.context_tokens = last_ctx;
    }
    if let Some(win) = json_u64(info, "model_context_window").filter(|w| *w > 0) {
        snap.context_pct = Some(((snap.context_tokens.saturating_mul(100)) / win).min(100) as u8);
    }
}

fn apply_codex_step(step: CodexUsage, snap: &mut TokenSnapshot) {
    snap.source = TelemetrySource::Codex;
    snap.input_tokens = snap.input_tokens.saturating_add(step.uncached_in);
    snap.cache_tokens = snap.cache_tokens.saturating_add(step.cached_in);
    snap.output_tokens = snap.output_tokens.saturating_add(step.output);
    if step.context > 0 {
        snap.context_tokens = step.context;
    }
}

pub(crate) fn clean_status_line(line: &str) -> &str {
    line.trim_matches(|c: char| c.is_whitespace() || "│┃┆┊║╭╮╰╯┌┐└┘├┤┬┴┼─━═".contains(c))
}

pub(crate) fn strip_punct(raw: &str) -> &str {
    raw.trim_matches(|c: char| ",;:()[]".contains(c))
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

fn apply_token_pair(raw: &str, snap: &mut TokenSnapshot) {
    if let Some((used_s, limit_s)) = raw.split_once('/') {
        let Some(used) = parse_human_tokens(used_s) else { return };
        snap.context_tokens = used;
        if snap.context_pct.is_none() {
            if let Some(lim) = parse_human_tokens(limit_s).filter(|l| *l > 0) {
                snap.context_pct = Some(((used.saturating_mul(100)) / lim).min(100) as u8);
            }
        }
    } else if let Some(toks) = parse_human_tokens(raw) {
        snap.context_tokens = toks;
    }
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
    let low = w.trim_matches(|c: char| ",;()[]".contains(c)).to_ascii_lowercase();
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
    let mut bottom: Vec<&str> = visible_text
        .lines()
        .rev()
        .map(clean_status_line)
        .filter(|l| !l.is_empty())
        .take(VTE_FALLBACK_LINES)
        .collect();
    bottom.reverse();
    let mut snap = TokenSnapshot { source: TelemetrySource::VteFallback, ..Default::default() };
    for line in bottom {
        if is_vte_breakdown_line(line) {
            continue;
        }
        let words: Vec<&str> = line
            .split(|c: char| c.is_whitespace() || matches!(c, '|' | '·' | '•'))
            .filter(|s| !s.is_empty())
            .collect();
        for idx in 0..words.len() {
            apply_word_at(&words, idx, &mut snap);
        }
    }
    snap.has_data().then_some(snap)
}

pub fn format_compact_tokens(tokens: u64) -> String {
    if tokens >= 999_500 {
        return format_scaled_unit(tokens.max(1_000_000), 1_000_000, "M");
    }
    if tokens >= 10_000 {
        return format!("{}k", tokens.saturating_add(500) / 1_000);
    }
    if tokens >= 1_000 {
        return format_scaled_unit(tokens, 1_000, "k");
    }
    tokens.to_string()
}

fn format_scaled_unit(tokens: u64, unit: u64, suffix: &str) -> String {
    let whole = tokens / unit;
    let tenth = (tokens % unit) / (unit / 10);
    if tenth == 0 {
        format!("{whole}{suffix}")
    } else {
        format!("{whole}.{tenth}{suffix}")
    }
}

pub fn format_turn_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3_600, (secs % 3_600) / 60)
    }
}

pub fn format_working_badge(secs: u64) -> String {
    format!("⏱ {}", format_turn_duration(secs))
}

pub fn format_working_sidebar_status(secs: u64) -> String {
    format!("В работе · {}", format_turn_duration(secs))
}

pub struct TabTokenRow {
    pub title: String,
    pub agent: String,
    pub is_active: bool,
    pub working_secs: Option<u64>,
    pub last_turn_secs: Option<u64>,
    pub snap: TokenSnapshot,
}

impl TabTokenRow {
    pub fn from_session(s: &TermSession, active_id: usize, now: Instant) -> Self {
        Self {
            title: s.title.clone(),
            agent: s.effective_agent(),
            is_active: s.id == active_id,
            working_secs: s.live_turn_secs(now),
            last_turn_secs: s.last_turn_secs,
            snap: s.telemetry.snapshot().clone(),
        }
    }
}

pub fn build_sessions_tokens_view(
    sessions: &[TermSession],
    active_id: usize,
    mode: ThemeMode,
    now: Instant,
) -> TokensViewData {
    let rows: Vec<TabTokenRow> = sessions
        .iter()
        .map(|s| TabTokenRow::from_session(s, active_id, now))
        .collect();
    build_tokens_view_for_mode(&rows, mode)
}

pub struct TokensViewData {
    pub button_label: Option<String>,
    pub alert: ContextAlert,
    pub markup: Option<String>,
}

fn escape_markup(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn format_ctx_span(snap: &TokenSnapshot, colors: TokensMarkupColors) -> String {
    let base = match (snap.context_tokens > 0, snap.context_pct) {
        (true, Some(pct)) => format!("ctx {} ({pct}%)", format_compact_tokens(snap.context_tokens)),
        (true, None) => format!("ctx {}", format_compact_tokens(snap.context_tokens)),
        (false, Some(pct)) => format!("ctx {pct}%"),
        (false, None) => "ctx —".to_owned(),
    };
    match snap.alert_level() {
        ContextAlert::High => format!("<span foreground='{}'><b>{base}</b></span>", colors.high),
        ContextAlert::Warn => format!("<span foreground='{}'><b>{base}</b></span>", colors.warn),
        ContextAlert::Normal => format!("<b>{base}</b>"),
    }
}

fn format_timer_part(row: &TabTokenRow, colors: TokensMarkupColors) -> String {
    if let Some(secs) = row.working_secs {
        return format!(
            "<span foreground='{}'>⏱ {} (в работе)</span>",
            colors.timer,
            format_turn_duration(secs),
        );
    }
    if let Some(secs) = row.last_turn_secs {
        return format!("⏱ {} (посл. ход)", format_turn_duration(secs));
    }
    "⏱ —".to_owned()
}

fn format_io_breakdown(snap: &TokenSnapshot) -> String {
    let cost_s = snap.cost_usd.map_or(String::new(), |c| format!(" · ${c:.2}"));
    if snap.source == TelemetrySource::VteFallback && snap.total_tokens() == 0 {
        return format!("статус TUI{cost_s}");
    }
    format!(
        "input {} · cache {} · output {}{cost_s}",
        format_compact_tokens(snap.effective_input()),
        format_compact_tokens(snap.cache_tokens),
        format_compact_tokens(snap.output_tokens),
    )
}

fn format_tab_token_markup(row: &TabTokenRow, colors: TokensMarkupColors) -> String {
    let marker = if row.is_active { "▸ " } else { "  " };
    let src = if !row.agent.is_empty() { row.agent.as_str() } else { row.snap.source.label() };
    let tag = if src.is_empty() {
        String::new()
    } else {
        format!(" <span foreground='{}'>[{}]</span>", colors.muted, escape_markup(src))
    };
    format!(
        "{marker}<b>{}</b>{tag}\n    {}  ·  <tt>{}</tt>  ·  {}",
        escape_markup(&row.title),
        format_ctx_span(&row.snap, colors),
        format_io_breakdown(&row.snap),
        format_timer_part(row, colors),
    )
}

#[derive(Default)]
struct WindowTotals {
    tot_in: u64,
    tot_cache: u64,
    tot_out: u64,
    tot_ctx: u64,
    tot_cost: f64,
    has_cost: bool,
    active_badge: Option<String>,
    active_alert: ContextAlert,
}

impl WindowTotals {
    fn record(&mut self, row: &TabTokenRow) {
        self.tot_in = self.tot_in.saturating_add(row.snap.effective_input());
        self.tot_cache = self.tot_cache.saturating_add(row.snap.cache_tokens);
        self.tot_out = self.tot_out.saturating_add(row.snap.output_tokens);
        self.tot_ctx = self.tot_ctx.saturating_add(row.snap.context_tokens);
        if let Some(cost) = row.snap.cost_usd {
            self.tot_cost += cost;
            self.has_cost = true;
        }
        if row.is_active {
            self.active_badge = row.snap.compact_context_badge();
            self.active_alert = row.snap.alert_level();
        }
    }

    fn button_label(&self) -> Option<String> {
        if let Some(badge) = &self.active_badge {
            return Some(format!("Токены · {} ▾", badge.trim_start_matches("ctx ")));
        }
        (self.tot_ctx > 0).then(|| format!("Токены · Σ {} ▾", format_compact_tokens(self.tot_ctx)))
    }

    fn summary_markup(&self, row_count: usize, colors: TokensMarkupColors) -> String {
        let cost_s = if self.has_cost { format!("  ·  ${:.2}", self.tot_cost) } else { String::new() };
        format!(
            "<span foreground='{}'>Итого по окну ({row_count}): <b>input {} · cache {} · output {}</b>  ·  Σ ctx {}{cost_s}</span>",
            colors.summary,
            format_compact_tokens(self.tot_in),
            format_compact_tokens(self.tot_cache),
            format_compact_tokens(self.tot_out),
            format_compact_tokens(self.tot_ctx),
        )
    }
}

pub fn build_tokens_view_for_mode(rows: &[TabTokenRow], mode: ThemeMode) -> TokensViewData {
    let colors = mode.tokens_markup_colors();
    let mut totals = WindowTotals::default();
    let mut lines = Vec::new();
    for row in rows {
        totals.record(row);
        if row.snap.has_data() || row.working_secs.is_some() || row.last_turn_secs.is_some() {
            lines.push(format_tab_token_markup(row, colors));
        }
    }
    let button_label = totals.button_label();
    if lines.is_empty() {
        return TokensViewData { button_label, alert: totals.active_alert, markup: None };
    }
    let summary = totals.summary_markup(lines.len(), colors);
    lines.push(String::new());
    lines.push(summary);
    TokensViewData { button_label, alert: totals.active_alert, markup: Some(lines.join("\n")) }
}

#[cfg(test)]
pub fn build_tokens_view(rows: &[TabTokenRow]) -> TokensViewData {
    build_tokens_view_for_mode(rows, crate::theme::current_theme_mode())
}

#[derive(Clone, Copy)]
pub struct TokensMenu {
    button: *mut GtkWidget,
    popover: *mut GtkWidget,
    body_label: *mut GtkWidget,
}

impl TokensMenu {
    pub fn new(topbar: *mut GtkWidget, on_open: ClickHandler) -> Self {
        unsafe {
            let button = gtk_button_new_with_label(text::TOKENS.as_ptr());
            gtk_widget_set_tooltip_text(button, text::TOKENS_TIP.as_ptr());
            gtk_box_pack_end(topbar, button, 0, 0, 0);
            let popover = gtk_popover_new(button);
            let body_label = populate_tokens_popover(popover);
            connect_click(button, on_open, popover);
            Self { button, popover, body_label }
        }
    }

    pub fn popover(&self) -> *mut GtkWidget {
        self.popover
    }

    pub fn apply(&self, view: &TokensViewData) {
        if self.button.is_null() || self.body_label.is_null() {
            return;
        }
        set_class(self.button, "btn-mi-b", view.alert == ContextAlert::Warn);
        set_class(self.button, "btn-mi-c", view.alert == ContextAlert::High);
        apply_button_label(self.button, view.button_label.as_deref());
        apply_body_markup(self.body_label, view.markup.as_deref());
    }
}

fn populate_tokens_popover(popover: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(vbox, "popover-box");
        let hdr = gtk_label_new(text::TOKENS_HEADER.as_ptr());
        gtk_label_set_xalign(hdr, 0.0);
        add_class(hdr, "section-header");
        gtk_box_pack_start(vbox, hdr, 0, 0, 0);

        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, 450);
        gtk_scrolled_window_set_max_content_height(scroll, 380);
        let body_label = gtk_label_new(text::TOKENS_EMPTY.as_ptr());
        gtk_label_set_xalign(body_label, 0.0);
        gtk_container_add(scroll, body_label);
        gtk_box_pack_start(vbox, scroll, 1, 1, 0);
        gtk_container_add(popover, vbox);
        gtk_widget_show_all(vbox);
        body_label
    }
}

fn apply_button_label(button: *mut GtkWidget, label: Option<&str>) {
    if let Some(c) = label.and_then(|s| CString::new(s).ok()) {
        unsafe { gtk_button_set_label(button, c.as_ptr()) };
    } else {
        unsafe { gtk_button_set_label(button, text::TOKENS.as_ptr()) };
    }
}

fn apply_body_markup(label: *mut GtkWidget, markup: Option<&str>) {
    if let Some(c) = markup.and_then(|s| CString::new(s).ok()) {
        unsafe { gtk_label_set_markup(label, c.as_ptr()) };
    } else {
        unsafe { gtk_label_set_text(label, text::TOKENS_EMPTY.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry_agy::{text_mentions_workspace, transcript_from_fd_target};
    use std::ffi::{c_char, CStr};
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::thread;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("akhterm_tel_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn incremental_agy_tailing_seeks_only_appended_bytes() {
        let dir = temp_dir("agy_tail");
        let log = dir.join("transcript.jsonl");
        let l1 = "{\"step_index\":0,\"type\":\"GENERIC\",\"content\":\"{\\\"type\\\":\\\"PLANNER_RESPONSE\\\",\\\"input_tokens\\\":999999}\"}\n";
        let l2 = "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":4500,\"cache_read_tokens\":20500,\"output_tokens\":600}\n";
        fs::write(&log, format!("{l1}{l2}")).unwrap();

        let mut tailer = TranscriptTailer::default();
        assert!(tailer.poll_file(&log, TelemetrySource::Agy).unwrap());
        assert_eq!(tailer.offset, (l1.len() + l2.len()) as u64);
        assert_eq!(tailer.snapshot.context_tokens, 25_600);
        assert!(!tailer.poll_file(&log, TelemetrySource::Agy).unwrap());

        overwrite_prefix_and_append(&log, l1.len() + l2.len() - 1);
        assert!(tailer.poll_file(&log, TelemetrySource::Agy).unwrap());
        assert_eq!(tailer.snapshot.input_tokens, 14_500);
        assert_eq!(tailer.snapshot.cache_tokens, 94_500);
        assert_eq!(tailer.snapshot.output_tokens, 1_500);
        assert_eq!(tailer.snapshot.context_tokens, 84_900);
        let _ = fs::remove_dir_all(&dir);
    }

    fn overwrite_prefix_and_append(log: &Path, scrub_len: usize) {
        let mut f = fs::OpenOptions::new().write(true).open(log).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&vec![b' '; scrub_len]).unwrap();
        f.seek(SeekFrom::End(0)).unwrap();
        let l3 = "{\"step_index\":2,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":10000,\"cache_read_tokens\":74000,\"output_tokens\":900}\n";
        f.write_all(l3.as_bytes()).unwrap();
    }

    #[test]
    fn tailer_resets_safely_when_transcript_is_truncated() {
        let dir = temp_dir("agy_rot");
        let log = dir.join("transcript.jsonl");
        let long = "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":30000,\"cache_read_tokens\":90000,\"output_tokens\":1200}\n";
        fs::write(&log, format!("{long}{long}")).unwrap();
        let mut tailer = TranscriptTailer::default();
        assert!(tailer.poll_file(&log, TelemetrySource::Agy).unwrap());
        assert_eq!(tailer.snapshot.input_tokens, 60_000);

        let short = "{\"step_index\":0,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":1200,\"cache_read_tokens\":3800,\"output_tokens\":150}\n";
        fs::write(&log, short).unwrap();
        assert!(tailer.poll_file(&log, TelemetrySource::Agy).unwrap());
        assert_eq!(tailer.offset, short.len() as u64);
        assert_eq!(tailer.snapshot.input_tokens, 1_200);
        assert_eq!(tailer.snapshot.context_tokens, 5_150);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_checkpoint_resets_stale_pre_compaction_context_tokens() {
        let dir = temp_dir("agy_ckpt");
        let log = dir.join("transcript.jsonl");
        let pre = "{\"step_index\":10,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":38000,\"cache_read_tokens\":218000,\"output_tokens\":2000}\n";
        let ckpt = "{\"step_index\":11,\"type\":\"CHECKPOINT\",\"input_tokens\":258000,\"cache_read_tokens\":0,\"output_tokens\":4200}\n";
        fs::write(&log, pre).unwrap();
        let mut tailer = TranscriptTailer::default();
        tailer.poll_file(&log, TelemetrySource::Agy).unwrap();
        tailer.snapshot.context_pct = Some(26);
        assert_eq!(tailer.snapshot.context_tokens, 258_000);

        fs::write(&log, format!("{pre}{ckpt}")).unwrap();
        tailer.poll_file(&log, TelemetrySource::Agy).unwrap();
        assert_eq!(tailer.snapshot.context_tokens, 4_200);
        assert_eq!(tailer.snapshot.context_pct, None);
        assert_eq!(tailer.snapshot.output_tokens, 6_200);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovers_and_parses_claude_project_jsonl() {
        let base = temp_dir("claude_disc");
        let home = base.join("home");
        let repo = home.join("work/my.repo");
        fs::create_dir_all(repo.join("src")).unwrap();
        let encoded = encode_claude_cwd(&repo.to_string_lossy());
        assert!(encoded.ends_with("-work-my-repo"));
        let claude_dir = home.join(".claude/projects").join(&encoded);
        fs::create_dir_all(&claude_dir).unwrap();
        write_claude_fixture(&claude_dir.join("session-a.jsonl"));

        let (found, kind) = discover_transcript(&home, &repo.join("src").to_string_lossy(), 0, "Claude").unwrap();
        let mut tailer = TranscriptTailer::default();
        tailer.poll_file(&found, kind).unwrap();
        assert_eq!(tailer.snapshot.effective_input(), 5_500);
        assert_eq!(tailer.snapshot.cache_tokens, 65_000);
        assert_eq!(tailer.snapshot.output_tokens, 650);
        assert_eq!(tailer.snapshot.context_tokens, 35_500);
        let _ = fs::remove_dir_all(&base);
    }

    fn write_claude_fixture(path: &Path) {
        let c1 = "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":200,\"cache_creation_input_tokens\":4800,\"cache_read_input_tokens\":30000,\"output_tokens\":120}}}\n";
        let c1_dup = "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":200,\"cache_creation_input_tokens\":4800,\"cache_read_input_tokens\":30000,\"output_tokens\":400}}}\n";
        let c2 = "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_2\",\"usage\":{\"input_tokens\":500,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":35000,\"output_tokens\":250}}}\n";
        fs::write(path, format!("{c1}{c1_dup}{c2}")).unwrap();
    }

    #[test]
    fn discovers_and_parses_codex_session_jsonl() {
        let base = temp_dir("codex_disc");
        let home = base.join("home");
        let repo = home.join("work/app");
        let sibling = home.join("work/app-wt");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let codex_day = home.join(".codex/sessions/2026/11/11");
        fs::create_dir_all(&codex_day).unwrap();
        write_codex_fixture(&codex_day, &repo, &sibling);

        let (found, kind) = discover_transcript(&home, &repo.to_string_lossy(), 0, "Codex").unwrap();
        assert_eq!(found, codex_day.join("rollout-main.jsonl"));
        let mut tailer = TranscriptTailer::default();
        tailer.poll_file(&found, kind).unwrap();
        assert_eq!(tailer.snapshot.input_tokens, 24_000);
        assert_eq!(tailer.snapshot.cache_tokens, 72_000);
        assert_eq!(tailer.snapshot.output_tokens, 3_400);
        assert_eq!(tailer.snapshot.context_tokens, 58_000);
        assert_eq!(tailer.snapshot.context_pct, Some(29));
        let _ = fs::remove_dir_all(&base);
    }

    fn write_codex_fixture(day: &Path, repo: &Path, sibling: &Path) {
        fs::write(
            day.join("rollout-wt.jsonl"),
            format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n", sibling.display()),
        )
        .unwrap();
        let hdr = format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n", repo.display());
        let ev = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":96000,\"cached_input_tokens\":72000,\"output_tokens\":3400,\"total_tokens\":99400},\"last_token_usage\":{\"input_tokens\":58000,\"cached_input_tokens\":50000,\"output_tokens\":1100,\"total_tokens\":59100},\"model_context_window\":200000}}}\n";
        fs::write(day.join("rollout-main.jsonl"), format!("{hdr}{ev}")).unwrap();
    }

    #[test]
    fn matches_agy_fd_and_workspace_boundaries_cleanly() {
        let base = temp_dir("agy_disc");
        let home = base.join("home");
        let logs_dir = home.join(".gemini/antigravity-cli/brain/abc-123/.system_generated/logs");
        fs::create_dir_all(&logs_dir).unwrap();
        let transcript = logs_dir.join("transcript.jsonl");
        fs::write(&transcript, "{\"type\":\"SYSTEM_MESSAGE\",\"content\":\"Workspace: /tmp/proj-alpha\\n\"}\n").unwrap();

        let brain_root = home.join(".gemini/antigravity-cli/brain");
        assert_eq!(transcript_from_fd_target(&transcript, &brain_root).as_deref(), Some(transcript.as_path()));
        assert_eq!(text_mentions_workspace("dir: /tmp/proj-alpha-wt\n", "/tmp/proj-alpha"), None);
        assert_eq!(discover_transcript(&home, "/tmp/proj-alpha", 0, "AGY").map(|p| p.0), Some(transcript));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn parses_vte_status_bar_fallback_patterns() {
        let screen = "999k tokens · 99% context · $9.99\nl1\nl2\nl3\nl4\n╭──────────────────────╮\n│ sonnet · 142k tokens · 68% context · $0.42 │\n╰──────────────────────╯\n\n";
        let snap = parse_vte_status_fallback(screen).unwrap();
        assert_eq!(snap.context_tokens, 142_000);
        assert_eq!(snap.context_pct, Some(68));
        assert_eq!(snap.cost_usd, Some(0.42));
        assert_eq!(snap.alert_level(), ContextAlert::Warn);

        let snap2 = parse_vte_status_fallback("agent ready\nused 12,400 tokens | 25% context left\n").unwrap();
        assert_eq!(snap2.context_tokens, 12_400);
        assert_eq!(snap2.context_pct, Some(75));

        let snap3 = parse_vte_status_fallback("status: ctx:84k cost:$1.05\n").unwrap();
        assert_eq!(snap3.context_tokens, 84_000);
        assert_eq!(snap3.cost_usd, Some(1.05));
        assert!(parse_vte_status_fallback("user@host:~/repo$ ls -la\n-rw-r--r-- 1 u u 12400 a.txt\n").is_none());
    }

    #[test]
    fn formats_timers_compact_tokens_and_popover_markup() {
        assert_eq!(format_turn_duration(42), "42s");
        assert_eq!(format_turn_duration(72), "1m 12s");
        assert_eq!(format_turn_duration(134), "2m 14s");
        assert_eq!(format_turn_duration(3_725), "1h 02m");
        assert_eq!(format_working_badge(42), "⏱ 42s");
        assert_eq!(format_working_sidebar_status(72), "В работе · 1m 12s");
        assert_eq!(format_compact_tokens(84_200), "84k");
        assert_eq!(format_compact_tokens(43_510), "44k");
        assert_eq!(format_compact_tokens(130_700), "131k");
        assert_eq!(format_compact_tokens(999_499), "999k");
        assert_eq!(format_compact_tokens(999_500), "1M");
        assert_eq!(format_compact_tokens(999_600), "1M");
        assert_eq!(format_compact_tokens(1_000_000), "1M");
        assert_eq!(format_compact_tokens(4_200), "4.2k");

        let view = build_tokens_view(&[sample_high_row()]);
        assert_eq!(view.button_label.as_deref(), Some("Токены · 184k ▾"));
        assert_eq!(view.alert, ContextAlert::High);
        let markup = view.markup.unwrap();
        assert!(markup.contains("R&amp;D &lt;main&gt;") && markup.contains("Итого по окну (1):"));
    }

    fn sample_high_row() -> TabTokenRow {
        TabTokenRow {
            title: "R&D <main>".to_owned(),
            agent: "Claude".to_owned(),
            is_active: true,
            working_secs: Some(134),
            last_turn_secs: None,
            snap: TokenSnapshot {
                source: TelemetrySource::Claude,
                input_tokens: 14_000,
                cache_tokens: 170_000,
                cache_write_tokens: 0,
                output_tokens: 3_200,
                context_tokens: 184_000,
                context_pct: Some(92),
                cost_usd: Some(0.42),
            },
        }
    }

    #[test]
    fn headless_broadway_tokens_menu_and_vte_fallback() {
        if std::env::var_os("AKHTERM_TELEMETRY_BWAY_CHILD").is_none() {
            let exe = std::env::current_exe().unwrap();
            let st = Command::new(exe)
                .args(["--exact", "telemetry::tests::headless_broadway_tokens_menu_and_vte_fallback"])
                .env("AKHTERM_TELEMETRY_BWAY_CHILD", "1")
                .status()
                .unwrap();
            assert!(st.success());
            return;
        }
        run_broadway_tokens_check();
    }

    fn run_broadway_tokens_check() {
        let dir = temp_dir("bway17");
        let Ok(mut bway) = spawn_test_broadway(&dir) else {
            let _ = fs::remove_dir_all(&dir);
            return;
        };
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        std::env::set_var("GDK_BACKEND", "broadway");
        std::env::set_var("BROADWAY_DISPLAY", ":17");
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } != 0 {
            exercise_tokens_widgets();
        }
        let _ = bway.kill();
        let _ = bway.wait();
        let _ = fs::remove_dir_all(&dir);
    }

    fn spawn_test_broadway(dir: &Path) -> io::Result<std::process::Child> {
        let child = Command::new("broadwayd")
            .arg("-u")
            .arg(dir.join("http.sock"))
            .arg(":17")
            .env("XDG_RUNTIME_DIR", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        for _ in 0..80 {
            if dir.join("broadway18.socket").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(child)
    }

    extern "C" fn noop_click(_btn: *mut GtkWidget, _data: *mut c_void) {}

    fn pump_gtk() {
        for _ in 0..60 {
            if unsafe { gtk_events_pending() } == 0 {
                break;
            }
            unsafe { gtk_main_iteration_do(0) };
        }
    }

    fn exercise_tokens_widgets() {
        unsafe {
            let (win, vte, menu) = create_test_tokens_window();
            pump_gtk();
            let line = "remote tmux · 142k tokens · 68% context · $0.42\r\n";
            vte_terminal_feed(vte, line.as_ptr() as *const c_char, line.len() as isize);
            thread::sleep(Duration::from_millis(35));
            pump_gtk();
            verify_vte_poll_and_menu(vte, menu);
            verify_vte_context_panel_widget(vte);
            gtk_widget_destroy(win);
        }
    }

    unsafe fn verify_vte_context_panel_widget(vte: *mut GtkWidget) {
        let panel = format!("\x1b[2J\x1b[H{}", sample_agy_context_screen().replace('\n', "\r\n"));
        vte_terminal_feed(vte, panel.as_ptr() as *const c_char, panel.len() as isize);
        thread::sleep(Duration::from_millis(35));
        pump_gtk();
        assert_vte_panel_job_override(vte);
    }

    fn assert_vte_panel_job_override(vte: *mut GtkWidget) {
        let mut seeded = SessionTelemetry::default();
        seeded.tailer.snapshot = TokenSnapshot {
            source: TelemetrySource::Agy, input_tokens: 9_000, context_tokens: 43_510, ..Default::default()
        };
        seeded.tailer.offset = 512;
        let job = SessionPollJob {
            sid: 2, root_pid: 0, vte, local_cwd: Some("/nonexistent".to_owned()),
            has_ssh: false, has_tmux: false, agent: "AGY".to_owned(), is_daemon: false, telemetry: seeded,
        };
        let (_, telem) = run_poll_job(job, Path::new("/nonexistent"), Instant::now());
        assert_eq!((telem.snapshot().context_tokens, telem.snapshot().context_pct), (130_700, Some(13)));
    }

    fn create_test_tokens_window() -> (*mut GtkWidget, *mut GtkWidget, TokensMenu) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            gtk_window_set_default_size(win, 800, 480);
            let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
            let topbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
            let vte = vte_terminal_new();
            let menu = TokensMenu::new(topbar, noop_click);
            gtk_box_pack_start(vbox, topbar, 0, 1, 0);
            gtk_box_pack_start(vbox, vte, 1, 1, 0);
            gtk_container_add(win, vbox);
            gtk_widget_show_all(win);
            (win, vte, menu)
        }
    }

    fn verify_vte_poll_and_menu(vte: *mut GtkWidget, menu: TokensMenu) {
        // Plain local shell (`local_cwd: Some`, no ssh, no tmux, no agent) must ignore `$0.42` / `142k tokens`.
        let plain_local = SessionPollJob {
            sid: 1,
            root_pid: 0,
            vte,
            local_cwd: Some("/tmp".to_owned()),
            has_ssh: false,
            has_tmux: false,
            agent: String::new(),
            is_daemon: false,
            telemetry: SessionTelemetry::default(),
        };
        let (_, plain_telem) = run_poll_job(plain_local, Path::new("/nonexistent"), Instant::now());
        assert!(!plain_telem.snapshot().has_data());

        // Remote or SSH-in-local pane (`has_ssh: true`) resets stale local tailer and uses VTE fallback.
        let mut seeded = SessionTelemetry::default();
        seeded.tailer.snapshot.context_tokens = 99_000;
        let job = SessionPollJob {
            sid: 1,
            root_pid: 0,
            vte,
            local_cwd: Some("/tmp".to_owned()),
            has_ssh: true,
            has_tmux: false,
            agent: "Claude".to_owned(),
            is_daemon: false,
            telemetry: seeded,
        };
        let (_, telem) = run_poll_job(job, Path::new("/nonexistent"), Instant::now());
        assert!(!telem.tailer.snapshot.has_data());
        assert_eq!(telem.snapshot().context_tokens, 142_000);
        apply_and_assert_menu(menu, &telem);
    }

    fn apply_and_assert_menu(menu: TokensMenu, telem: &SessionTelemetry) {
        let row = TabTokenRow {
            title: "srv · Claude".to_owned(),
            agent: "Claude".to_owned(),
            is_active: true,
            working_secs: Some(42),
            last_turn_secs: None,
            snap: telem.snapshot().clone(),
        };
        let light_view = build_tokens_view_for_mode(std::slice::from_ref(&row), ThemeMode::Light);
        let light_markup = light_view.markup.as_deref().unwrap_or("");
        assert!(light_markup.contains("#b45309") && light_markup.contains("#1d4ed8") && light_markup.contains("#5b21b6"));
        menu.apply(&build_tokens_view_for_mode(&[row], ThemeMode::Dark));
        let lbl = unsafe { CStr::from_ptr(gtk_label_get_text(menu.body_label)) }
            .to_string_lossy()
            .into_owned();
        assert!(lbl.contains("srv · Claude"));
    }

    #[test]
    fn scopes_ssh_override_and_vte_fallback_cleanly() {
        let base = temp_dir("ssh_scope");
        let home = base.join("home");
        let repo = home.join("work/proj");
        fs::create_dir_all(&repo).unwrap();
        let claude_dir = home.join(".claude/projects").join(encode_claude_cwd(&repo.to_string_lossy()));
        fs::create_dir_all(&claude_dir).unwrap();
        write_claude_fixture(&claude_dir.join("local.jsonl"));

        assert!(!should_poll_vte_fallback(Some("/tmp/repo"), false, ""));
        assert!(should_poll_vte_fallback(None, false, ""));
        assert!(should_poll_vte_fallback(Some("/tmp/repo"), true, ""));
        assert!(should_poll_vte_fallback(Some("/tmp/repo"), false, "Kimi"));

        // Local Claude tab attaches to local JSONL, then resets when `ssh` starts in the same tab.
        let local_job = SessionPollJob {
            sid: 7,
            root_pid: 0,
            vte: ptr::null_mut(),
            local_cwd: Some(repo.to_string_lossy().into_owned()),
            has_ssh: false,
            has_tmux: false,
            agent: "Claude".to_owned(),
            is_daemon: false,
            telemetry: SessionTelemetry::default(),
        };
        let (_, local_tel) = run_poll_job(local_job, &home, Instant::now());
        assert_eq!(local_tel.snapshot().context_tokens, 35_500);

        let ssh_job = SessionPollJob {
            sid: 7,
            root_pid: 0,
            vte: ptr::null_mut(),
            local_cwd: Some(repo.to_string_lossy().into_owned()),
            has_ssh: true,
            has_tmux: false,
            agent: "Claude".to_owned(),
            is_daemon: false,
            telemetry: local_tel,
        };
        let (_, ssh_tel) = run_poll_job(ssh_job, &home, Instant::now());
        assert!(!ssh_tel.snapshot().has_data());
        assert!(ssh_tel.tailer.path.is_none());
        let _ = fs::remove_dir_all(&base);
    }

    fn write_brain_log(brain_root: &Path, id: &str, body: &str, age_secs: u64) -> PathBuf {
        let logs = brain_root.join(id).join(".system_generated/logs");
        fs::create_dir_all(&logs).unwrap();
        let path = logs.join("transcript.jsonl");
        fs::write(&path, body).unwrap();
        let stamp = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_760_000_000 + age_secs);
        fs::OpenOptions::new().write(true).open(&path).unwrap().set_modified(stamp).unwrap();
        path
    }

    fn register_subagent(brain_root: &Path, parent_id: &str, sub_id: &str) {
        let dir = brain_root.join(parent_id).join(".system_generated/subagents");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{sub_id}.json")), "{}").unwrap();
    }

    fn seed_productivity_layout(home: &Path) -> PathBuf {
        let prod = home.join("Productivity");
        fs::create_dir_all(prod.join("board")).unwrap();
        fs::create_dir_all(prod.join("akhterm-rs/.git")).unwrap();
        fs::create_dir_all(prod.join("akhterm-wt")).unwrap();
        fs::write(prod.join("akhterm-wt/.git"), "gitdir: /tmp/fake-wt\n").unwrap();
        prod
    }

    fn build_large_productivity_transcript() -> String {
        let pad = "x".repeat(70_000);
        let head = format!("{{\"step_index\":0,\"type\":\"GENERIC\",\"content\":\"{pad}\"}}\n");
        let ws = "{\"step_index\":40,\"type\":\"USER_INPUT\",\"content\":\"~/Productivity/board/РЕЕСТР-УЧЕБНЫХ-ССЫЛОК.md\"}\n";
        let turn = "{\"step_index\":41,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":10000,\"cache_read_tokens\":120000,\"output_tokens\":700}\n";
        format!("{head}{ws}{turn}")
    }

    fn seed_competing_brains(brain: &Path, prod: &Path) {
        for idx in 0..20 {
            let body = format!("{{\"type\":\"USER_INPUT\",\"content\":\"/tmp/unrelated-{idx}\"}}\n");
            write_brain_log(brain, &format!("other-{idx}"), &body, 20 + idx);
        }
        let child = format!(
            "{{\"content\":\"{}/src/main.rs ~/Productivity/akhterm-rs\"}}\n{{\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":3000,\"cache_read_tokens\":40000,\"output_tokens\":510}}\n",
            prod.join("akhterm-rs").display()
        );
        write_brain_log(brain, "akhterm-child", &child, 60);
        write_brain_log(brain, "akhterm-wt", "{\"content\":\"~/Productivity/akhterm-wt/Cargo.toml\"}\n", 70);
        write_brain_log(brain, "swarm-sub", "{\"content\":\"~/Productivity/board/РЕЕСТР-УЧЕБНЫХ-ССЫЛОК.md\"}\n", 90);
        register_subagent(brain, "akhterm-child", "swarm-sub");
    }

    #[test]
    fn selects_parent_productivity_over_nested_git_repos_and_subagents() {
        let base = temp_dir("prod_nested");
        let home = base.join("home");
        let prod = seed_productivity_layout(&home);
        let brain = home.join(".gemini/antigravity-cli/brain");
        let prod_log = write_brain_log(&brain, "prod-main", &build_large_productivity_transcript(), 10);
        seed_competing_brains(&brain, &prod);

        let (found, kind) = discover_transcript(&home, &prod.to_string_lossy(), 0, "AGY").unwrap();
        assert_eq!(found, prod_log);
        assert_polled_131k(&found, kind);

        fs::remove_file(&prod_log).unwrap();
        assert!(discover_transcript(&home, &prod.to_string_lossy(), 0, "AGY").is_none());
        let _ = fs::remove_dir_all(&base);
    }

    fn assert_polled_131k(found: &Path, kind: TelemetrySource) {
        let mut tailer = TranscriptTailer::default();
        tailer.poll_file(found, kind).unwrap();
        assert_eq!(tailer.snapshot.context_tokens, 130_700);
        assert_eq!(tailer.snapshot.compact_context_badge().as_deref(), Some("ctx 131k"));
    }

    fn sample_agy_context_screen() -> String {
        [
            "Context Usage",
            "Gemini 4 Carbon (High) · 130.7k/1.0M tokens",
            "(12.5%)",
            "System prompt: 12.4k tokens (1.2%)",
            "System tools: 18.2k tokens (1.7%)",
            "Skills: 2.1k tokens (0.2%)",
            "  SKILL.md: 24 tokens",
            "User messages: 1.4k tokens (0.1%)",
            "Agent responses: 42.1k tokens (4.0%)",
            "Tool calls: 54.5k tokens (5.2%)",
            "System messages: 0 tokens (0.0%)",
            "Subagents: 0 tokens (0.0%)",
            "Free space: 700.1k tokens (66.7%)",
            "Checkpoint buffer: 169.2k tokens (16.1%)",
            "↑/↓ Scroll · Esc Close",
        ]
        .join("\n")
    }

    fn make_local_agy_job(ws: &Path, telem: SessionTelemetry) -> SessionPollJob {
        SessionPollJob {
            sid: 1,
            root_pid: 0,
            vte: ptr::null_mut(),
            local_cwd: Some(ws.to_string_lossy().into_owned()),
            has_ssh: false,
            has_tmux: false,
            agent: "AGY".to_owned(),
            is_daemon: false,
            telemetry: telem,
        }
    }

    #[test]
    fn syncs_multiline_context_panel_and_persists_until_jsonl_advances() {
        let base = temp_dir("ctx_panel");
        let home = base.join("home");
        let ws = home.join("Productivity");
        fs::create_dir_all(&ws).unwrap();
        let init = "{\"type\":\"USER_INPUT\",\"content\":\"~/Productivity/ДОСКА.md\"}\n{\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":8000,\"cache_read_tokens\":118000,\"output_tokens\":500}\n";
        let log = write_brain_log(&home.join(".gemini/antigravity-cli/brain"), "s1", init, 10);

        let (_, mut telem) = run_poll_job(make_local_agy_job(&ws, SessionTelemetry::default()), &home, Instant::now());
        assert_eq!(telem.snapshot().context_tokens, 126_500);
        assert_panel_capture(&mut telem);
        assert_panel_close_and_advance(&home, &ws, &log, telem);
        let _ = fs::remove_dir_all(&base);
    }

    fn assert_panel_capture(telem: &mut SessionTelemetry) {
        telem.observe_vte_text(&sample_agy_context_screen());
        assert_eq!(telem.snapshot().context_tokens, 130_700);
        assert_eq!(telem.snapshot().context_pct, Some(13));
        assert_eq!(telem.snapshot().compact_context_badge().as_deref(), Some("ctx 131k"));
        assert_eq!(telem.snapshot().input_tokens, 8_000);
        assert_eq!(telem.snapshot().source, TelemetrySource::Agy);
    }

    fn assert_panel_close_and_advance(home: &Path, ws: &Path, log: &Path, mut telem: SessionTelemetry) {
        telem.observe_vte_text("agy > ready for next prompt\n");
        let (_, closed) = run_poll_job(make_local_agy_job(ws, telem), home, Instant::now());
        assert_eq!(closed.snapshot().context_tokens, 130_700);
        assert_eq!(closed.snapshot().compact_context_badge().as_deref(), Some("ctx 131k"));

        let mut f = fs::OpenOptions::new().append(true).open(log).unwrap();
        writeln!(f, "{{\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":9000,\"cache_read_tokens\":130000,\"output_tokens\":800}}").unwrap();
        let (_, advanced) = run_poll_job(make_local_agy_job(ws, closed), home, Instant::now());
        assert!(advanced.vte_panel_override.is_none());
        assert_eq!(advanced.snapshot().context_tokens, 139_800);
    }

    #[test]
    fn guards_status_fallback_and_panel_against_breakdown_lines() {
        let screen = sample_agy_context_screen();
        assert_eq!(parse_vte_context_panel(&screen), Some((130_700, Some(13))));
        assert!(parse_vte_status_fallback(&screen).is_none());

        let tail_only = "  SKILL.md: 24 tokens\nFree space: 700.1k tokens\nCheckpoint buffer: 169.2k tokens\n↑/↓ Scroll\n";
        assert!(parse_vte_context_panel(tail_only).is_none());
        assert!(parse_vte_status_fallback(tail_only).is_none());

        let no_pct = "Model · 85k/200k токенов\nSystem prompt: 8.2k tokens (0.8%)\n";
        assert_eq!(parse_vte_context_panel(no_pct), Some((85_000, Some(43))));
    }

    #[test]
    fn maintainability_index_stays_above_targets() {
        let (_, _, tel_mi) = crate::mi::compute_file_mi(include_str!("telemetry.rs"));
        let (_, _, agy_mi) = crate::mi::compute_file_mi(include_str!("telemetry_agy.rs"));
        let (_, _, main_mi) = crate::mi::compute_file_mi(include_str!("main.rs"));
        assert!(tel_mi >= 58.0, "telemetry.rs MI {tel_mi:.2} must be >= 58.0");
        assert!(agy_mi >= 62.0, "telemetry_agy.rs MI {agy_mi:.2} must be >= 62.0");
        assert!(main_mi >= 54.87, "main.rs MI {main_mi:.2} must be >= 54.87");
    }
}
