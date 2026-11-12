//! Per-pane process, approval snippet, turn timer, and token telemetry radar.

#![allow(dead_code)]

use std::{collections::{HashMap, HashSet}, path::Path, time::{Duration, Instant}};
use crate::approval::{classify_terminal_text_with_snippet, maybe_notify_background_event, read_vte_visible_text, QUIET_SETTLE_DELAY};
use crate::ffi::GtkWidget;
use crate::i18n::{current_locale, UiLocale, UiText};
use crate::pane_registry::{PaneId, PaneRecord, PaneRegistry};
use crate::paths::{expand_home, shorten_home};
use crate::proc_radar::{inspect_proc_tree, ProcInfo};
use crate::remote_telemetry::{RemoteSessionTarget, RemoteTailer};
use crate::session::{detect_agent_name, TermSession, WorkState};
use crate::tabs::RemoteConnection;
use crate::telemetry::{build_tokens_view_with_keys, format_turn_duration, run_poll_job_with_remote, ContextAlert, SessionPollJob, SessionTelemetry, TabTokenRow, TelemetrySource, TokenSnapshot, TokensViewData};
use crate::theme_mode::ThemeMode;

pub const BADGE_APPROVE: UiText = UiText::new(c"Апрув", c"Approve");
pub const BADGE_WORKING: UiText = UiText::new(c"В работе", c"Working");
pub const BADGE_READY: UiText = UiText::new(c"Готово", c"Ready");
pub const BADGE_SHELL: UiText = UiText::new(c"оболочка", c"shell");
pub const BADGE_EXITED: UiText = UiText::new(c"завершена", c"exited");
const AGENT_SINGULAR: UiText = UiText::new(c"агент", c"agent");
const AGENT_FEW: UiText = UiText::new(c"агента", c"agents");
const AGENT_MANY: UiText = UiText::new(c"агентов", c"agents");

pub fn format_agents_badge_for(count: usize, locale: UiLocale) -> Option<String> {
    if count < 2 {
        return None;
    }
    let word = match (count % 100, count % 10) {
        (11..=14, _) => AGENT_MANY,
        (_, 1) => AGENT_SINGULAR,
        (_, 2..=4) => AGENT_FEW,
        _ => AGENT_MANY,
    };
    Some(format!("{count} {}", word.for_locale(locale).to_str().unwrap_or("")))
}

pub fn format_agents_badge(count: usize) -> Option<String> {
    format_agents_badge_for(count, current_locale())
}

pub fn sync_pane_proc(
    pane: &mut PaneRecord,
    mut proc_info: ProcInfo,
    remote_tmux: Option<(String, String, String, String)>,
) {
    if !proc_info.cwd.is_empty() && proc_info.ssh_host.is_empty() {
        pane.cwd = expand_home(&proc_info.cwd);
    }
    if let Some(tmux) = remote_tmux {
        pane.remote_tmux = Some(tmux);
    }
    fill_pane_agent_from_hints(pane, &mut proc_info);
    pane.proc_info = proc_info;
}

fn fill_pane_agent_from_hints(pane: &PaneRecord, proc_info: &mut ProcInfo) {
    if !proc_info.agent.is_empty() {
        return;
    }
    let tmux_agent = pane
        .remote_tmux
        .as_ref()
        .and_then(|(_, win, cmd, _)| detect_agent_name(cmd).or_else(|| detect_agent_name(win)));
    if let Some(agent) = tmux_agent.or_else(|| detect_agent_name(&pane.osc_title)) {
        proc_info.agent = agent.to_owned();
    }
}

pub fn sync_registry_procs(reg: &mut PaneRegistry) {
    for pane in reg.panes_mut() {
        if pane.child_pid > 0 {
            sync_pane_proc(pane, inspect_proc_tree(pane.child_pid), None);
        }
    }
}

pub fn apply_pane_vte_title(pane: &mut PaneRecord, raw_title: &str, host: &str) {
    if let Some(rest) = raw_title.strip_prefix("AKHTERM_TMUX:") {
        apply_pane_tmux_title(pane, rest);
        return;
    }
    let clean = raw_title.trim();
    if clean.is_empty() || clean == host || matches!(clean, "zsh" | "bash" | "sh") {
        pane.osc_title.clear();
        return;
    }
    pane.osc_title = clean.chars().take(28).collect();
}

fn apply_pane_tmux_title(pane: &mut PaneRecord, rest: &str) {
    let parts: Vec<&str> = rest.splitn(4, '|').collect();
    if parts.len() == 4 && !parts[0].is_empty() {
        pane.remote_tmux = Some((parts[0].into(), parts[1].into(), parts[2].into(), parts[3].into()));
    }
}

pub fn settle_pane_with_text(
    pane: &mut PaneRecord,
    visible_text: &str,
    now: Instant,
    is_focused_active: bool,
    is_daemon: bool,
) -> Option<(WorkState, Duration)> {
    if is_daemon || pane.state != WorkState::Working {
        return None;
    }
    if now.saturating_duration_since(pane.last_output) <= QUIET_SETTLE_DELAY {
        return None;
    }
    let started = pane.work_started_at.unwrap_or(pane.work_started);
    let span = pane.last_output.saturating_duration_since(started);
    let (classified, snippet) = classify_terminal_text_with_snippet(visible_text);
    apply_settled_pane_state(pane, classified, snippet, is_focused_active, now);
    ensure_min_settled_secs(pane, span);
    Some((classified, span))
}

fn apply_settled_pane_state(
    pane: &mut PaneRecord,
    classified: WorkState,
    snippet: Option<String>,
    focused: bool,
    now: Instant,
) {
    if classified == WorkState::NeedsApproval {
        pane.set_approval(snippet, !focused, now);
    } else if !focused {
        pane.set_state(WorkState::Ready, now);
        pane.unread = true;
    } else {
        pane.set_state(WorkState::Idle, now);
        pane.unread = false;
    }
}

fn ensure_min_settled_secs(pane: &mut PaneRecord, work_span: Duration) {
    let secs = work_span.as_secs();
    if secs >= 1 {
        pane.last_turn_secs = Some(secs);
    } else if matches!(pane.state, WorkState::Ready | WorkState::NeedsApproval) && pane.last_turn_secs.is_none() {
        pane.last_turn_secs = Some(1);
    }
}

pub struct SettleContext {
    pub is_active_tab: bool,
    pub focused_vte: *mut GtkWidget,
    pub is_daemon: bool,
    pub tab_title: String,
    pub now: Instant,
    pub win: *mut GtkWidget,
    pub window_active: bool,
}

pub fn settle_registry_panes_with<F>(reg: &mut PaneRegistry, ctx: &SettleContext, mut read_text: F) -> usize
where
    F: FnMut(&PaneRecord) -> String,
{
    let mut settled = 0usize;
    for pane in reg.panes_mut() {
        let focused = ctx.is_active_tab && !ctx.focused_vte.is_null() && pane.vte == ctx.focused_vte;
        let text = read_text(pane);
        if let Some((cls, span)) = settle_pane_with_text(pane, &text, ctx.now, focused, ctx.is_daemon) {
            settled += 1;
            maybe_notify_background_event(cls, span, &ctx.tab_title, &mut pane.last_notified, ctx.now, ctx.win, ctx.window_active);
        }
    }
    settled
}

pub fn settle_registry_panes(reg: &mut PaneRegistry, ctx: &SettleContext) -> usize {
    settle_registry_panes_with(reg, ctx, |pane| read_vte_visible_text(pane.vte))
}

#[derive(Clone)]
pub struct PanePollJob {
    pub sid: usize,
    pub pane_id: PaneId,
    pub root_pid: i32,
    pub vte: *mut GtkWidget,
    pub local_cwd: Option<String>,
    pub has_ssh: bool,
    pub has_tmux: bool,
    pub agent: String,
    pub is_daemon: bool,
    pub telemetry: SessionTelemetry,
}

fn pane_local_cwd(pane: &PaneRecord, is_remote: bool) -> Option<String> {
    (!is_remote && pane.proc_info.ssh_host.is_empty() && !pane.cwd.is_empty())
        .then(|| expand_home(&pane.cwd))
}

impl PanePollJob {
    pub fn from_pane(sid: usize, pane: &PaneRecord, is_daemon: bool, is_remote: bool) -> Self {
        Self {
            sid, pane_id: pane.id, root_pid: pane.child_pid, vte: pane.vte,
            local_cwd: pane_local_cwd(pane, is_remote),
            has_ssh: is_remote || !pane.proc_info.ssh_host.is_empty(),
            has_tmux: !pane.proc_info.has_tmux.is_empty() || pane.remote_tmux.is_some(),
            agent: pane.effective_agent(), is_daemon, telemetry: pane.telemetry.clone(),
        }
    }

    fn into_session_job(self) -> SessionPollJob {
        SessionPollJob {
            sid: self.sid, root_pid: self.root_pid, vte: self.vte, local_cwd: self.local_cwd,
            has_ssh: self.has_ssh, has_tmux: self.has_tmux, agent: self.agent,
            is_daemon: self.is_daemon, telemetry: self.telemetry,
        }
    }
}

pub fn collect_pane_poll_jobs(sid: usize, reg: &PaneRegistry, is_daemon: bool, is_remote: bool) -> Vec<PanePollJob> {
    reg.panes().iter().map(|p| PanePollJob::from_pane(sid, p, is_daemon, is_remote)).collect()
}

pub struct PaneTokenSource<'a> {
    pub title: &'a str,
    pub is_active_tab: bool,
    pub focused_vte: *mut GtkWidget,
    pub session_fallback: &'a TermSession,
    pub registry: Option<&'a PaneRegistry>,
}

pub fn run_pane_poll_job(
    job: PanePollJob,
    remote_tailer: Option<RemoteTailer>,
    home: &Path,
    now: Instant,
) -> (PaneId, SessionTelemetry) {
    let pane_id = job.pane_id;
    let (_, telem) = run_poll_job_with_remote(job.into_session_job(), remote_tailer, home, now);
    (pane_id, telem)
}

pub fn apply_pane_telemetry(reg: &mut PaneRegistry, updates: Vec<(PaneId, SessionTelemetry)>) {
    for (pane_id, telem) in updates {
        if let Some(pane) = reg.get_mut(pane_id) {
            pane.telemetry = telem;
        }
    }
}

#[derive(Default)]
pub struct PaneProbeRouter {
    next_probe_id: usize,
    to_probe: HashMap<(usize, PaneId), usize>,
}

impl PaneProbeRouter {
    pub fn probe_id_for(&mut self, sid: usize, pane_id: PaneId) -> usize {
        if let Some(&id) = self.to_probe.get(&(sid, pane_id)) {
            return id;
        }
        self.next_probe_id = self.next_probe_id.saturating_add(1);
        self.to_probe.insert((sid, pane_id), self.next_probe_id);
        self.next_probe_id
    }

    pub fn collect_targets(
        &mut self,
        sid: usize,
        reg: &PaneRegistry,
        remote: Option<&RemoteConnection>,
    ) -> Vec<(PaneId, RemoteSessionTarget)> {
        let host = remote.map(|r| r.host.as_str());
        let tmux = remote.and_then(|r| r.tmux_session.as_deref());
        reg.panes().iter().filter_map(|p| {
            let pid = self.probe_id_for(sid, p.id);
            RemoteSessionTarget::from_pane(pid, p, host, tmux).map(|t| (p.id, t))
        }).collect()
    }
}

pub fn aggregate_registry_tokens(reg: &PaneRegistry) -> TokenSnapshot {
    let mut out = TokenSnapshot::default();
    let mut seen = HashSet::new();
    for pane in reg.panes() {
        let fresh = pane.telemetry.transcript_key().map_or(true, |k| seen.insert(k));
        if fresh {
            merge_snapshot(&mut out, pane.telemetry.snapshot());
        }
    }
    out
}

fn merge_snapshot(acc: &mut TokenSnapshot, snap: &TokenSnapshot) {
    if !snap.has_data() {
        return;
    }
    if acc.source == TelemetrySource::None {
        acc.source = snap.source;
    }
    acc.input_tokens = acc.input_tokens.saturating_add(snap.input_tokens);
    acc.cache_tokens = acc.cache_tokens.saturating_add(snap.cache_tokens);
    acc.cache_write_tokens = acc.cache_write_tokens.saturating_add(snap.cache_write_tokens);
    acc.output_tokens = acc.output_tokens.saturating_add(snap.output_tokens);
    acc.context_tokens = acc.context_tokens.saturating_add(snap.context_tokens);
    merge_snapshot_extras(acc, snap);
}

fn merge_snapshot_extras(acc: &mut TokenSnapshot, snap: &TokenSnapshot) {
    acc.context_pct = match (acc.context_pct, snap.context_pct) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    if let Some(c) = snap.cost_usd {
        acc.cost_usd = Some(acc.cost_usd.unwrap_or(0.0) + c);
    }
}

fn append_source_token_rows(src: &PaneTokenSource<'_>, now: Instant, out: &mut Vec<(TabTokenRow, Option<String>)>) {
    let Some(reg) = src.registry.filter(|r| !r.is_empty()) else {
        let active_id = if src.is_active_tab { src.session_fallback.id } else { 0 };
        let row = TabTokenRow::from_session(src.session_fallback, active_id, now);
        out.push((row, src.session_fallback.telemetry.transcript_key()));
        return;
    };
    let multi = reg.len() > 1;
    for (idx, pane) in reg.panes().iter().enumerate() {
        out.push(pane_to_token_row(src, pane, idx, multi, now));
    }
}

fn pane_to_token_row(src: &PaneTokenSource<'_>, pane: &PaneRecord, idx: usize, multi: bool, now: Instant) -> (TabTokenRow, Option<String>) {
    let title = if multi { format!("{} · #{}", src.title, idx + 1) } else { src.title.to_owned() };
    let is_active = src.is_active_tab && (pane.vte == src.focused_vte || (!multi && src.focused_vte.is_null()));
    let row = TabTokenRow {
        title, agent: pane.effective_agent(), is_active,
        working_secs: pane.live_turn_secs(now), last_turn_secs: pane.last_turn_secs,
        snap: pane.telemetry.snapshot().clone(),
    };
    (row, pane.telemetry.transcript_key())
}

pub fn build_panes_tokens_view(sources: &[PaneTokenSource<'_>], mode: ThemeMode, now: Instant) -> TokensViewData {
    let mut rows = Vec::new();
    for src in sources {
        append_source_token_rows(src, now, &mut rows);
    }
    build_tokens_view_with_keys(&rows, mode)
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabRadarSummary {
    pub state: WorkState,
    pub unread: bool,
    pub active_agents: usize,
    pub agents_badge: Option<String>,
    pub live_turn_secs: Option<u64>,
    pub last_turn_secs: Option<u64>,
    pub tokens: TokenSnapshot,
    pub primary_agent: String,
    pub approval_snippet: Option<String>,
}

pub fn summarize_registry_for(reg: &PaneRegistry, now: Instant, locale: UiLocale) -> TabRadarSummary {
    let active_agents = reg.active_agent_count();
    TabRadarSummary {
        state: reg.aggregated_state(), unread: reg.has_unread(), active_agents,
        agents_badge: format_agents_badge_for(active_agents, locale),
        live_turn_secs: reg.live_turn_secs(now), last_turn_secs: reg.last_turn_secs(),
        tokens: aggregate_registry_tokens(reg), primary_agent: pick_primary_agent(reg),
        approval_snippet: pick_approval_snippet(reg),
    }
}

pub fn summarize_registry(reg: &PaneRegistry, now: Instant) -> TabRadarSummary {
    summarize_registry_for(reg, now, current_locale())
}

fn pick_primary_agent(reg: &PaneRegistry) -> String {
    let order = [WorkState::NeedsApproval, WorkState::Working, WorkState::Ready, WorkState::Idle];
    for target in order {
        if let Some(a) = reg.panes().iter().filter(|p| p.state == target).map(PaneRecord::effective_agent).find(|a| !a.is_empty()) {
            return a;
        }
    }
    String::new()
}

fn pick_approval_snippet(reg: &PaneRegistry) -> Option<String> {
    reg.panes().iter().filter(|p| p.state == WorkState::NeedsApproval).find_map(|p| p.approval_snippet.clone())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneHeaderSummary {
    pub pane_id: PaneId,
    pub agent_or_proc: String,
    pub short_cwd: String,
    pub state: WorkState,
    pub state_badge: String,
    pub ctx_badge: Option<String>,
    pub alert: ContextAlert,
    pub approval_snippet: Option<String>,
    pub header_line: String,
}

pub fn format_pane_header_summary_for(pane: &PaneRecord, now: Instant, locale: UiLocale) -> PaneHeaderSummary {
    let agent_or_proc = pane_actor_label(pane, locale);
    let short_cwd = pane_short_cwd(pane);
    let state_badge = pane_state_badge(pane, now, locale);
    let snap = pane.telemetry.snapshot();
    let ctx_badge = snap.compact_context_badge();
    let header_line = join_header_parts(&agent_or_proc, &short_cwd, &state_badge, ctx_badge.as_deref());
    PaneHeaderSummary {
        pane_id: pane.id, agent_or_proc, short_cwd, state: pane.state, state_badge,
        ctx_badge, alert: snap.alert_level(), approval_snippet: pane.approval_snippet.clone(), header_line,
    }
}

pub fn format_pane_header_summary(pane: &PaneRecord, now: Instant) -> PaneHeaderSummary {
    format_pane_header_summary_for(pane, now, current_locale())
}

fn loc_str(text: UiText, locale: UiLocale) -> &'static str {
    text.for_locale(locale).to_str().unwrap_or("")
}

fn pane_actor_label(pane: &PaneRecord, locale: UiLocale) -> String {
    let agent = pane.effective_agent();
    if !agent.is_empty() {
        return agent;
    }
    if pane.child_pid <= 0 {
        return loc_str(BADGE_EXITED, locale).to_owned();
    }
    if !pane.proc_info.fg_cmd.is_empty() {
        return pane.proc_info.fg_cmd.clone();
    }
    loc_str(BADGE_SHELL, locale).to_owned()
}

fn pane_state_badge(pane: &PaneRecord, now: Instant, locale: UiLocale) -> String {
    match pane.state {
        WorkState::NeedsApproval => loc_str(BADGE_APPROVE, locale).to_owned(),
        WorkState::Working => pane.live_turn_secs(now)
            .map_or_else(|| loc_str(BADGE_WORKING, locale).to_owned(), |s| format!("⏱ {}", format_turn_duration(s))),
        WorkState::Ready => pane.last_turn_secs
            .map_or_else(|| loc_str(BADGE_READY, locale).to_owned(), |s| format!("{} · ⏱ {}", loc_str(BADGE_READY, locale), format_turn_duration(s))),
        WorkState::Idle => pane.last_turn_secs.map_or_else(String::new, |s| format!("⏱ {}", format_turn_duration(s))),
    }
}

fn pane_short_cwd(pane: &PaneRecord) -> String {
    let raw = if !pane.cwd.is_empty() {
        pane.cwd.as_str()
    } else {
        pane.remote_tmux.as_ref().map_or("", |t| t.3.as_str())
    };
    shorten_home(raw)
}

fn join_header_parts(actor: &str, cwd: &str, state: &str, ctx: Option<&str>) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(4);
    for item in [actor, cwd, state] {
        if !item.is_empty() {
            parts.push(item);
        }
    }
    if let Some(c) = ctx.filter(|s| !s.is_empty()) {
        parts.push(c);
    }
    parts.join(" · ")
}

pub fn summarize_registry_headers(reg: &PaneRegistry, now: Instant) -> Vec<PaneHeaderSummary> {
    summarize_registry_headers_for(reg, now, current_locale())
}

#[derive(Clone, Debug)]
pub struct PaneSwarmInfo {
    pub session_id: usize,
    pub pane_id: PaneId,
    pub vte: *mut GtkWidget,
    pub header: PaneHeaderSummary,
    pub live_turn_secs: Option<u64>,
    pub last_turn_secs: Option<u64>,
    pub snapshot: TokenSnapshot,
}

fn to_swarm_info(session_id: usize, pane: &PaneRecord, now: Instant) -> PaneSwarmInfo {
    PaneSwarmInfo {
        session_id, pane_id: pane.id, vte: pane.vte,
        header: format_pane_header_summary(pane, now),
        live_turn_secs: pane.live_turn_secs(now),
        last_turn_secs: pane.last_turn_secs,
        snapshot: pane.telemetry.snapshot().clone(),
    }
}

pub fn summarize_registry_headers_for(reg: &PaneRegistry, now: Instant, locale: UiLocale) -> Vec<PaneHeaderSummary> {
    reg.panes().iter().map(|p| format_pane_header_summary_for(p, now, locale)).collect()
}

pub fn collect_swarm_pane_infos(session_id: usize, reg: &PaneRegistry, now: Instant) -> Vec<PaneSwarmInfo> {
    reg.panes().iter()
        .filter(|p| p.is_active_agent() || p.state != WorkState::Idle)
        .map(|p| to_swarm_info(session_id, p, now))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{approval::{find_next_pane_attention, PaneAttention}, ffi::*, i18n::UiLocale};
    use crate::{proc_radar::tests::{pump_gtk_events, with_headless_gtk}, sidebar::Sidebar};
    use crate::{split_pane::{finish_split, wrap_vte_scroll, TabPanes}, tab_strip::{TabHandlers, TabStrip}};
    use crate::{session::SessionCategory, tabs::Tab, telemetry::{encode_claude_cwd, TokensMenu}};
    use std::{ffi::c_char, fs, path::PathBuf, ptr};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("akhterm_prad_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_widget(tag: usize) -> *mut GtkWidget {
        tag as *mut GtkWidget
    }

    fn make_agent_pane(reg: &mut PaneRegistry, vte: *mut GtkWidget, cwd: &str, agent: &str, t0: Instant, focus: bool) -> PaneId {
        let id = reg.register(vte, ptr::null_mut(), 1001, cwd.to_owned());
        let pane = reg.get_mut(id).unwrap();
        pane.proc_info.agent = agent.to_owned();
        pane.mark_output(t0, focus);
        id
    }

    fn seed_claude_jsonl(home: &Path, repo: &Path) -> PathBuf {
        let dir = home.join(".claude/projects").join(encode_claude_cwd(&repo.to_string_lossy()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("claude-s1.jsonl");
        let line = "{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":4000,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":30000,\"output_tokens\":500}}}\n";
        fs::write(&file, line).unwrap();
        file
    }

    fn seed_codex_jsonl(home: &Path, repo: &Path) -> PathBuf {
        let day = home.join(".codex/sessions/2026/11/12");
        fs::create_dir_all(&day).unwrap();
        let file = day.join("rollout-1.jsonl");
        let hdr = format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\",\"source\":\"cli\"}}}}\n", repo.display());
        let tok = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":80000,\"cached_input_tokens\":50000,\"output_tokens\":1000,\"total_tokens\":81000},\"last_token_usage\":{\"input_tokens\":80000,\"cached_input_tokens\":50000,\"output_tokens\":1000,\"total_tokens\":81000},\"model_context_window\":200000}}}\n";
        fs::write(&file, format!("{hdr}{tok}")).unwrap();
        file
    }

    fn poll_test_panes(reg: &mut PaneRegistry, home: &Path, now: Instant, probe_vte: bool) {
        let updates = collect_pane_poll_jobs(1, reg, false, false)
            .into_iter()
            .map(|mut j| {
                if !probe_vte {
                    j.vte = ptr::null_mut();
                }
                run_pane_poll_job(j, None, home, now)
            })
            .collect();
        apply_pane_telemetry(reg, updates);
    }

    fn settle_claude_and_keep_codex(reg: &mut PaneRegistry, t0: Instant) -> usize {
        reg.panes_mut()[0].last_output = t0 + Duration::from_secs(5);
        reg.panes_mut()[1].last_output = t0 + Duration::from_secs(14);
        let ctx = SettleContext {
            is_active_tab: true, focused_vte: fake_widget(2), is_daemon: false,
            tab_title: "Work".into(), now: t0 + Duration::from_secs(15), win: ptr::null_mut(), window_active: true,
        };
        settle_registry_panes_with(reg, &ctx, |p| {
            if p.vte == fake_widget(1) { "Allow Bash(cargo test)?".into() } else { "compiling...".into() }
        })
    }

    fn assert_pane_states(reg: &PaneRegistry, p1: PaneId, p2: PaneId, now: Instant) {
        let (r1, r2) = (reg.get(p1).unwrap(), reg.get(p2).unwrap());
        assert_eq!(r1.state, WorkState::NeedsApproval);
        assert_eq!(r1.last_turn_secs, Some(5));
        assert_eq!(r1.approval_snippet.as_deref(), Some("Allow Bash(cargo test)?"));
        assert_eq!(r2.state, WorkState::Working);
        assert_eq!(r2.live_turn_secs(now), Some(5));
    }

    fn assert_ru_summary_and_headers(reg: &PaneRegistry, now: Instant) {
        let sum = summarize_registry_for(reg, now, UiLocale::Ru);
        assert_eq!(sum.state, WorkState::NeedsApproval);
        assert_eq!(sum.active_agents, 2);
        assert_eq!(sum.agents_badge.as_deref(), Some("2 агента"));
        assert_eq!(sum.tokens.context_tokens, 34_000 + 80_000);
        let hdrs = summarize_registry_headers_for(reg, now, UiLocale::Ru);
        assert!(hdrs[0].header_line.contains("Claude") && hdrs[0].header_line.contains("Апрув"));
        assert!(hdrs[1].header_line.contains("Codex") && hdrs[1].header_line.contains("⏱ 5s"));
    }

    #[test]
    fn two_split_panes_track_independent_agents_timers_tokens_and_snippet() {
        let base = temp_dir("split_panes");
        let (home, r1, r2) = (base.join("home"), base.join("home/repo1"), base.join("home/repo2"));
        let _ = (fs::create_dir_all(&r1), fs::create_dir_all(&r2));
        let _ = (seed_claude_jsonl(&home, &r1), seed_codex_jsonl(&home, &r2));
        let (t0, mut reg) = (Instant::now(), PaneRegistry::default());
        let p1 = make_agent_pane(&mut reg, fake_widget(1), &r1.to_string_lossy(), "Claude", t0, false);
        let p2 = make_agent_pane(&mut reg, fake_widget(2), &r2.to_string_lossy(), "Codex", t0 + Duration::from_secs(10), true);
        poll_test_panes(&mut reg, &home, t0, false);
        assert_eq!(settle_claude_and_keep_codex(&mut reg, t0), 1);
        assert_pane_states(&reg, p1, p2, t0 + Duration::from_secs(15));
        assert_ru_summary_and_headers(&reg, t0 + Duration::from_secs(15));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn deduplicates_shared_transcript_files_across_panes_and_formats_en_badges() {
        let base = temp_dir("dedup");
        let (home, repo) = (base.join("home"), base.join("home/proj"));
        fs::create_dir_all(&repo).unwrap();
        seed_claude_jsonl(&home, &repo);
        let (t0, mut reg) = (Instant::now(), PaneRegistry::default());
        make_agent_pane(&mut reg, fake_widget(10), &repo.to_string_lossy(), "Claude", t0, false);
        make_agent_pane(&mut reg, fake_widget(11), &repo.to_string_lossy(), "Claude", t0, false);
        poll_test_panes(&mut reg, &home, t0, false);
        let sum = summarize_registry_for(&reg, t0, UiLocale::En);
        assert_eq!(sum.agents_badge.as_deref(), Some("2 agents"));
        assert_eq!((sum.tokens.context_tokens, sum.tokens.input_tokens), (34_000, 4_000));
        assert_eq!((format_agents_badge_for(1, UiLocale::En), format_agents_badge_for(5, UiLocale::En).as_deref()), (None, Some("5 agents")));
        let _ = fs::remove_dir_all(&base);
    }

    extern "C" fn noop_click(_w: *mut GtkWidget, _d: *mut std::ffi::c_void) {}
    extern "C" fn noop_evt(_w: *mut GtkWidget, _e: *const GdkEvent, _d: *mut std::ffi::c_void) -> std::ffi::c_int { 0 }

    fn feed_vte_line(vte: *mut GtkWidget, text: &str) {
        unsafe { vte_terminal_feed(vte, text.as_ptr() as *const c_char, text.len() as isize) };
        std::thread::sleep(Duration::from_millis(35));
        pump_gtk_events();
    }

    fn dummy_handlers() -> TabHandlers {
        TabHandlers { on_select: noop_click, on_close: noop_click, on_press: noop_evt, on_motion: noop_evt, on_release: noop_evt }
    }

    fn make_dummy_session(root: *mut GtkWidget, panes: TabPanes, vte: *mut GtkWidget, scroll_wrap: *mut GtkWidget) -> TermSession {
        let (sidebar, strip, now) = (Sidebar::new(root, noop_click), TabStrip::new(root, noop_click), Instant::now());
        TermSession {
            id: 7, base_title: "Work".into(), profile_cwd: "/tmp".into(), title: "Work".into(),
            osc_title: String::new(), pinned_title: None, cwd: "/tmp".into(), category: SessionCategory::Project,
            vte, scroll_wrap, panes, sidebar_row: sidebar.append_tab(SessionCategory::Project, 7, noop_click, noop_evt),
            top_tab: strip.append(7, SessionCategory::Project, dummy_handlers()), child_pid: 1001, state: WorkState::Idle,
            unread: false, last_output: now, work_started: now, work_started_at: None, last_turn_secs: None,
            last_notified: None, proc_info: ProcInfo::default(), tab: Tab::local("shell"), remote_tmux: None,
            tmux_title_from_vte: false, telemetry: SessionTelemetry::default(),
        }
    }

    fn setup_split_gtk_panes(root: *mut GtkWidget) -> (TabPanes, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
        unsafe {
            let (v1, v2) = (vte_terminal_new(), vte_terminal_new());
            let (s1, s2) = (wrap_vte_scroll(v1), wrap_vte_scroll(v2));
            let mut panes = TabPanes::new(s1, v1);
            gtk_box_pack_start(root, panes.root, 1, 1, 0);
            let op = panes.prepare_split_vte(v1, s1, v1, s2, v2, 1002, "/tmp".into(), GTK_ORIENTATION_HORIZONTAL);
            finish_split(Some(op), s2, v2);
            (panes, v1, s1, v2)
        }
    }

    fn verify_headless_tokens(session: &TermSession, reg: &PaneRegistry, v2: *mut GtkWidget, menu: TokensMenu, now: Instant) {
        let src = PaneTokenSource { title: "Work", is_active_tab: true, focused_vte: v2, session_fallback: session, registry: Some(reg) };
        let view = build_panes_tokens_view(&[src], ThemeMode::Dark, now);
        menu.apply(&view);
        let markup = view.markup.as_deref().unwrap_or("");
        assert!(markup.contains("Work · #1") && markup.contains("Work · #2"));
        assert!(markup.contains("Итого по окну (2): <b>input 4k · cache 30k · output 500</b>  ·  Σ ctx 34k"));
    }

    fn verify_headless_jump_focus(win: *mut GtkWidget, session: &TermSession, reg: &PaneRegistry, v1: *mut GtkWidget, v2: *mut GtkWidget) {
        unsafe { gtk_widget_grab_focus(v2) };
        pump_gtk_events();
        assert_eq!(unsafe { gtk_window_get_focus(win) }, v2);
        let target = find_next_pane_attention(&[(session, Some(reg))], session.id, v2).expect("jump to pane 1");
        assert_eq!((target.session_id, target.vte), (session.id, v1));
        unsafe { gtk_widget_grab_focus(target.vte) };
        pump_gtk_events();
        assert_eq!(unsafe { gtk_window_get_focus(win) }, v1);
    }

    fn prepare_headless_split_reg(win: *mut GtkWidget, v1: *mut GtkWidget, v2: *mut GtkWidget, home: &Path, repo: &Path) -> (PaneRegistry, Instant) {
        let (t0, mut reg) = (Instant::now(), PaneRegistry::default());
        make_agent_pane(&mut reg, v1, &repo.to_string_lossy(), "Claude", t0, false);
        make_agent_pane(&mut reg, v2, &repo.to_string_lossy(), "Codex", t0, false);
        poll_test_panes(&mut reg, home, t0, true);
        reg.panes_mut()[0].last_output = t0 + Duration::from_secs(4);
        reg.panes_mut()[1].last_output = t0 + Duration::from_secs(11);
        let ctx = SettleContext { is_active_tab: true, focused_vte: v2, is_daemon: false, tab_title: "Work".into(), now: t0 + Duration::from_secs(12), win, window_active: true };
        assert_eq!(settle_registry_panes(&mut reg, &ctx), 1);
        assert_eq!(reg.panes()[0].approval_snippet.as_deref(), Some("Allow Bash(cargo test --offline)?"));
        (reg, ctx.now)
    }

    fn run_headless_split_scenario(win: *mut GtkWidget, root: *mut GtkWidget, home: &Path, repo: &Path) {
        let menu = TokensMenu::new(root, noop_click);
        let (panes, v1, s1, v2) = setup_split_gtk_panes(root);
        unsafe { gtk_widget_show_all(win) };
        pump_gtk_events();
        feed_vte_line(v1, "Allow Bash(cargo test --offline)?\r\n");
        feed_vte_line(v2, "Running cargo test...\r\n");
        let (reg, now) = prepare_headless_split_reg(win, v1, v2, home, repo);
        let session = make_dummy_session(root, panes, v1, s1);
        verify_headless_tokens(&session, &reg, v2, menu, now);
        verify_headless_jump_focus(win, &session, &reg, v1, v2);
        let _ = PaneAttention { session_id: 7, pane_id: reg.panes()[0].id, vte: v1, state: WorkState::NeedsApproval, unread: true };
    }

    #[test]
    fn headless_broadway_split_panes_approval_tokens_dedup_and_ctrl_shift_u_focus() {
        let base = temp_dir("bway_split");
        let (home, repo) = (base.join("home"), base.join("home/shared"));
        fs::create_dir_all(&repo).unwrap();
        seed_claude_jsonl(&home, &repo);
        with_headless_gtk(|| unsafe {
            let (win, root) = (gtk_window_new(GTK_WINDOW_TOPLEVEL), gtk_box_new(GTK_ORIENTATION_VERTICAL, 0));
            gtk_window_set_default_size(win, 900, 480);
            gtk_container_add(win, root);
            run_headless_split_scenario(win, root, &home, &repo);
            gtk_widget_destroy(win);
            pump_gtk_events();
        });
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn pane_radar_maintains_grade_b_mi_target() {
        let (_, _, score) = crate::mi::compute_file_mi(include_str!("pane_radar.rs"));
        assert!(score >= 60.0, "pane_radar.rs MI {score:.2} < 60.0");
    }
}
