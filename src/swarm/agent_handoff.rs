//! Context rot health classification, cache drop detection, and 1-click Agent Handoff Pack.

use std::ffi::{c_char, c_int, c_void, CString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::app_state::{self, AppState, APP};
use crate::ffi::*;
use crate::handoff::{current_unix_secs, discover_handoff_files, extract_vte_text};
use crate::i18n::{current_locale, UiLocale};
use crate::palette::{PaletteAction, PaletteItem, PaletteKind};
use crate::paths::{shorten_home, state_dir, write_state_file};
use crate::session_ops;
use crate::tabs::Tab;
use crate::telemetry::TokenSnapshot;
use crate::ui_text as text;

pub const CONTEXT_WARN_PCT: u8 = 70;
pub const CONTEXT_CRIT_PCT: u8 = 85;
pub const CACHE_DROP_DELTA_PCT: u8 = 25;
pub const CACHE_DROP_PEAK_PCT: u8 = 75;
pub const CACHE_DROP_FLOOR_PCT: u8 = 50;
pub const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;

pub const MAX_DIFF_STAT_LINES: usize = 30;
pub const MAX_DIFF_STAT_BYTES: usize = 2_400;
pub const MAX_TAIL_LINES: usize = 45;
pub const MAX_TAIL_BYTES: usize = 3_600;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContextSeverity {
    #[default]
    Healthy,
    Warning,
    Critical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextHealth {
    pub severity: ContextSeverity,
    pub fill_pct: Option<u8>,
    pub cache_drop: bool,
    pub cache_prev_pct: Option<u8>,
    pub cache_curr_pct: Option<u8>,
}

impl ContextHealth {
    pub fn from_snapshot(snap: &TokenSnapshot) -> Self {
        let fill_pct = Self::compute_fill_pct(snap);
        let history = snap.requests.recent_cache_pcts();
        Self::classify(fill_pct, &history)
    }

    pub fn compute_fill_pct(snap: &TokenSnapshot) -> Option<u8> {
        if let Some(pct) = snap.context_pct {
            return Some(pct.min(100));
        }
        if snap.context_tokens == 0 {
            return None;
        }
        let pct = (snap.context_tokens.saturating_mul(100)) / DEFAULT_CONTEXT_WINDOW;
        Some(pct.min(100) as u8)
    }

    pub fn classify(fill_pct: Option<u8>, cache_pcts: &[u8]) -> Self {
        let severity = classify_severity(fill_pct);
        let (cache_drop, cache_prev_pct, cache_curr_pct) = detect_cache_drop(cache_pcts);
        Self { severity, fill_pct, cache_drop, cache_prev_pct, cache_curr_pct }
    }

    pub fn is_warning(self) -> bool {
        self.severity == ContextSeverity::Warning
    }

    pub fn is_critical(self) -> bool {
        self.severity == ContextSeverity::Critical
    }

    pub fn has_cache_drop(self) -> bool {
        self.cache_drop
    }

    pub fn needs_attention(self) -> bool {
        self.severity != ContextSeverity::Healthy || self.cache_drop
    }

    pub fn apply_css_classes(self, widget: *mut GtkWidget) {
        if widget.is_null() {
            return;
        }
        set_class(widget, "ctx-warn", self.is_warning());
        set_class(widget, "ctx-crit", self.is_critical());
        set_class(widget, "ctx-cache-drop", self.has_cache_drop());
    }
}

fn classify_severity(fill_pct: Option<u8>) -> ContextSeverity {
    match fill_pct {
        Some(pct) if pct >= CONTEXT_CRIT_PCT => ContextSeverity::Critical,
        Some(pct) if pct >= CONTEXT_WARN_PCT => ContextSeverity::Warning,
        _ => ContextSeverity::Healthy,
    }
}

fn detect_cache_drop(cache_pcts: &[u8]) -> (bool, Option<u8>, Option<u8>) {
    if cache_pcts.len() < 2 {
        return (false, None, cache_pcts.last().copied());
    }
    let curr = cache_pcts[cache_pcts.len() - 1];
    let prev = cache_pcts[cache_pcts.len() - 2];
    let prior_peak = cache_pcts[..cache_pcts.len() - 1].iter().copied().max().unwrap_or(0);
    let sharp_drop = prev.saturating_sub(curr) >= CACHE_DROP_DELTA_PCT;
    let floor_drop = prior_peak >= CACHE_DROP_PEAK_PCT && curr < CACHE_DROP_FLOOR_PCT;
    let baseline = if sharp_drop { prev } else { prior_peak };
    (sharp_drop || floor_drop, Some(baseline), Some(curr))
}

fn loc_text(entry: crate::i18n::UiText, locale: UiLocale) -> &'static str {
    entry.for_locale(locale).to_str().unwrap_or("")
}

impl ContextHealth {
    pub fn banner_text(self, locale: UiLocale) -> Option<String> {
        if !self.needs_attention() {
            return None;
        }
        let mut parts = Vec::with_capacity(2);
        if let Some(ctx_part) = self.severity_message(locale) {
            parts.push(ctx_part);
        }
        if let Some(cache_part) = self.cache_drop_message(locale) {
            parts.push(cache_part);
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    pub fn tooltip_note(self, locale: UiLocale) -> Option<String> {
        let mut parts = Vec::with_capacity(2);
        if let Some(pct) = self.fill_pct {
            if self.is_critical() {
                parts.push(format!("{} {pct}%", loc_text(text::CTX_HEALTH_CRIT_PREFIX, locale)));
            } else if self.is_warning() {
                parts.push(format!("{} {pct}%", loc_text(text::CTX_HEALTH_WARN_PREFIX, locale)));
            }
        }
        if self.cache_drop {
            parts.push(self.cache_drop_compact(locale));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    fn severity_message(self, locale: UiLocale) -> Option<String> {
        let pct = self.fill_pct?;
        match self.severity {
            ContextSeverity::Critical => Some(format!(
                "{} {pct}% · {}",
                loc_text(text::CTX_HEALTH_CRIT_PREFIX, locale),
                loc_text(text::CTX_HEALTH_CRIT_HINT, locale),
            )),
            ContextSeverity::Warning => Some(format!(
                "{} {pct}% · {}",
                loc_text(text::CTX_HEALTH_WARN_PREFIX, locale),
                loc_text(text::CTX_HEALTH_WARN_HINT, locale),
            )),
            ContextSeverity::Healthy => None,
        }
    }

    fn cache_drop_message(self, locale: UiLocale) -> Option<String> {
        if !self.cache_drop {
            return None;
        }
        let compact = self.cache_drop_compact(locale);
        Some(format!("{compact} · {}", loc_text(text::CTX_CACHE_DROP_HINT, locale)))
    }

    fn cache_drop_compact(self, locale: UiLocale) -> String {
        let label = loc_text(text::CTX_CACHE_DROP_LABEL, locale);
        match (self.cache_prev_pct, self.cache_curr_pct) {
            (Some(prev), Some(curr)) => {
                if locale == UiLocale::Ru {
                    format!("{label} (с {prev}% до {curr}%)")
                } else {
                    format!("{label} ({prev}% down to {curr}%)")
                }
            }
            _ => label.to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffAgent {
    Claude,
    Codex,
    Agy,
}

pub const HANDOFF_AGENTS: [HandoffAgent; 3] = [
    HandoffAgent::Claude,
    HandoffAgent::Codex,
    HandoffAgent::Agy,
];

impl HandoffAgent {
    pub fn agent_action_index(self) -> usize {
        match self {
            Self::Agy => 0,
            Self::Codex => 1,
            Self::Claude => 2,
        }
    }

    pub fn from_code(code: usize) -> Option<Self> {
        HANDOFF_AGENTS.get(code).copied()
    }

    pub fn code(self) -> usize {
        match self {
            Self::Claude => 0,
            Self::Codex => 1,
            Self::Agy => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Agy => "AGY",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    pub fn command(self) -> &'static str {
        text::AGENT_ACTIONS[self.agent_action_index()].1
    }
}

#[derive(Clone, Debug)]
pub struct HandoffPackInput {
    pub session_title: String,
    pub source_agent: String,
    pub target_agent: HandoffAgent,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub git_diff_stat: Option<String>,
    pub terminal_tail: String,
    pub active_brief: Option<PathBuf>,
    pub health: ContextHealth,
    pub locale: UiLocale,
}

pub fn safe_code_fence(text: &str) -> String {
    let max_ticks = text
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    "`".repeat(max_ticks.max(2) + 1)
}

fn truncate_utf8(raw: &str, max_bytes: usize) -> &str {
    if raw.len() <= max_bytes {
        return raw;
    }
    let mut end = max_bytes;
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    &raw[..end]
}

pub fn cap_text_head(raw: &str, max_lines: usize, max_bytes: usize) -> String {
    let lines: Vec<&str> = raw.lines().map(|l| l.trim_end_matches('\r')).collect();
    if lines.is_empty() {
        return String::new();
    }
    let clipped: Vec<&str> = lines.iter().copied().take(max_lines).collect();
    let mut out = clipped.join("\n");
    if out.len() > max_bytes {
        out = truncate_utf8(&out, max_bytes).to_owned();
    }
    if lines.len() > max_lines || raw.len() > max_bytes {
        out.push_str(&format!("\n... (+{} lines truncated)", lines.len().saturating_sub(clipped.len())));
    }
    out
}

pub fn cap_text_tail(raw: &str, max_lines: usize, max_bytes: usize) -> String {
    let non_empty: Vec<&str> = raw
        .lines()
        .map(|l| l.trim_end_matches([' ', '\t', '\r']))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let skip = non_empty.len().saturating_sub(max_lines);
    let mut out = non_empty[skip..].join("\n");
    if out.len() > max_bytes {
        let start = out.len() - max_bytes;
        let mut idx = start;
        while idx < out.len() && !out.is_char_boundary(idx) {
            idx += 1;
        }
        out = format!("...\n{}", &out[idx..]);
    }
    out
}

fn run_git_capture(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn git_branch_name(cwd: &Path) -> Option<String> {
    let branch = run_git_capture(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        run_git_capture(cwd, &["rev-parse", "--short", "HEAD"])
    } else {
        Some(branch)
    }
}

fn git_diff_summary(cwd: &Path) -> Option<String> {
    let raw = run_git_capture(cwd, &["diff", "--stat", "HEAD"])
        .or_else(|| run_git_capture(cwd, &["diff", "--stat"]))
        .or_else(|| run_git_capture(cwd, &["status", "--short"]))?;
    Some(cap_text_head(&raw, MAX_DIFF_STAT_LINES, MAX_DIFF_STAT_BYTES))
}

pub fn collect_git_context(cwd: &Path) -> (Option<String>, Option<String>) {
    if !cwd.is_dir() {
        return (None, None);
    }
    let Some(branch) = git_branch_name(cwd) else {
        return (None, None);
    };
    (Some(branch), git_diff_summary(cwd))
}

fn is_generated_handoff_pack(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with("handoff_pack_"))
}

fn find_terminal_md_path(terminal_raw: &str, cwd: &str) -> Option<PathBuf> {
    terminal_raw
        .lines()
        .rev()
        .flat_map(|line| line.split(['`', '"', '\'', '(', ')', '[', ']']).rev())
        .flat_map(|chunk| std::iter::once(chunk.trim()).chain(chunk.split_whitespace().rev()))
        .filter_map(|word| crate::proc_radar::resolve_link_target(word, Some(cwd)))
        .map(PathBuf::from)
        .find(|p| p.is_file() && crate::markdown::accepts(p) && !is_generated_handoff_pack(p))
}

pub fn find_active_brief(
    cwd: &Path,
    terminal_raw: &str,
    briefs_dir: Option<&Path>,
) -> Option<PathBuf> {
    let cwd_str = cwd.to_string_lossy();
    if let Some(found) = find_terminal_md_path(terminal_raw, &cwd_str) {
        return Some(found);
    }
    discover_handoff_files(briefs_dir, Some(cwd))
        .into_iter()
        .find(|p| !is_generated_handoff_pack(p))
}

fn push_fenced_section(out: &mut String, heading: &str, body: &str) {
    let fence = safe_code_fence(body);
    out.push_str(&format!("\n## {heading}\n\n{fence}text\n{body}\n{fence}\n"));
}

fn pack_meta_labels(ru: bool) -> [&'static str; 6] {
    if ru {
        ["Каталог", "Ветка Git", "Предыдущая сессия", "Целевой агент", "Состояние контекста", "Активный бриф"]
    } else {
        ["Workspace", "Git branch", "Previous session", "Target agent", "Context health", "Active brief"]
    }
}

fn push_code_bullet(out: &mut String, label: &str, value: &str) {
    out.push_str(&format!("- {label}: `{value}`\n"));
}

fn push_metadata_lines(out: &mut String, input: &HandoffPackInput) {
    let labels = pack_meta_labels(input.locale == UiLocale::Ru);
    let cwd = if input.cwd.is_empty() { "~".to_owned() } else { shorten_home(&input.cwd) };
    let session = if input.session_title.trim().is_empty() { "Terminal" } else { input.session_title.trim() };
    let tag = if input.source_agent.is_empty() { String::new() } else { format!(" ({})", input.source_agent) };
    push_code_bullet(out, labels[0], &cwd);
    if let Some(branch) = &input.git_branch {
        push_code_bullet(out, labels[1], branch);
    }
    out.push_str(&format!("- {}: `{session}`{tag}\n", labels[2]));
    push_code_bullet(out, labels[3], input.target_agent.label());
    if let Some(note) = input.health.tooltip_note(input.locale) {
        out.push_str(&format!("- {}: {note}\n", labels[4]));
    }
    if let Some(brief) = &input.active_brief {
        push_code_bullet(out, labels[5], &shorten_home(&brief.to_string_lossy()));
    }
}

fn pack_framing(ru: bool) -> (&'static str, &'static str, &'static str, &'static str) {
    if ru {
        (
            "# Handoff Pack для продолжения задачи\n\nПродолжи работу из предыдущей сессии в этом каталоге без повторения уже сделанных шагов.\n\n## Контекст\n\n",
            "Изменения Git (diff --stat)",
            "Хвост вывода и ошибок терминала",
            "\n## Инструкция\n\n1. Проверь активный бриф (если указан) и текущий `git diff`.\n2. Продолжи с последней точки и доведи задачу до проверки.\n",
        )
    } else {
        (
            "# Agent Handoff Pack\n\nContinue the task from the previous session in this workspace without repeating completed work.\n\n## Context\n\n",
            "Git Diff Stat",
            "Recent Terminal Output and Errors",
            "\n## Next Steps\n\n1. Inspect the active brief (if listed) and current `git diff`.\n2. Continue from the latest state and verify the result.\n",
        )
    }
}

pub fn build_handoff_pack(input: &HandoffPackInput) -> String {
    let (intro, diff_hdr, tail_hdr, steps) = pack_framing(input.locale == UiLocale::Ru);
    let mut out = String::with_capacity(1024);
    out.push_str(intro);
    push_metadata_lines(&mut out, input);
    if let Some(diff) = input.git_diff_stat.as_deref().filter(|d| !d.trim().is_empty()) {
        push_fenced_section(&mut out, diff_hdr, diff);
    }
    let tail = cap_text_tail(&input.terminal_tail, MAX_TAIL_LINES, MAX_TAIL_BYTES);
    if !tail.is_empty() {
        push_fenced_section(&mut out, tail_hdr, &tail);
    }
    out.push_str(steps);
    out
}

pub fn handoff_packs_dir() -> PathBuf {
    state_dir().join("handoffs")
}

fn unique_pack_path(dir: &Path, slug: &str, ts: u64) -> PathBuf {
    let primary = dir.join(format!("handoff_pack_{slug}_{ts}.md"));
    if !primary.exists() {
        return primary;
    }
    (2..1000u32)
        .map(|seq| dir.join(format!("handoff_pack_{slug}_{ts}_{seq}.md")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(primary)
}

pub fn stage_handoff_pack_in(
    dir: &Path,
    target: HandoffAgent,
    pack: &str,
    ts: u64,
) -> io::Result<PathBuf> {
    let path = unique_pack_path(dir, target.slug(), ts);
    write_state_file(&path, pack)?;
    Ok(path)
}

pub fn copy_pack_to_clipboard(pack: &str) {
    let clean = pack.replace('\0', "");
    let Ok(c_text) = CString::new(clean) else {
        return;
    };
    let len = c_text.as_bytes().len().min(c_int::MAX as usize) as c_int;
    unsafe {
        let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
        if !clip.is_null() {
            gtk_clipboard_set_text(clip, c_text.as_ptr() as *const c_char, len);
        }
    }
}

struct ActiveHandoffSource {
    profile: String,
    cwd: String,
    title: String,
    agent: String,
    vte: *mut GtkWidget,
    health: ContextHealth,
    briefs_dir: Option<PathBuf>,
}

fn dismiss_tokens_popover() {
    let pop = app_state::with_app(|app| app.tokens_menu.popover());
    if let Some(pop) = pop.filter(|p| !p.is_null()) {
        unsafe { gtk_popover_popdown(pop) };
    }
}

fn resolve_handoff_profile(app: &AppState, profile: &str) -> String {
    if app.config.profile(profile).monitor {
        app.config.profile("shell").key
    } else {
        profile.to_owned()
    }
}

fn read_active_handoff_source(locale: UiLocale) -> Result<ActiveHandoffSource, String> {
    APP.with(|cell| {
        let state = cell.borrow();
        let app = state.as_ref().ok_or_else(String::new)?;
        let err = || loc_text(text::CTX_HANDOFF_LOCAL_ONLY, locale).to_owned();
        let s = app.sessions.iter().find(|s| s.id == app.active_id).ok_or_else(err)?;
        let Tab::Local { ref profile, .. } = s.tab else {
            return Err(err());
        };
        let cwd = s.active_local_cwd().filter(|c| !c.is_empty()).unwrap_or_else(|| s.cwd.clone());
        Ok(ActiveHandoffSource {
            profile: resolve_handoff_profile(app, profile),
            cwd,
            title: s.title.clone(),
            agent: s.effective_agent(),
            vte: s.active_vte(),
            health: ContextHealth::from_snapshot(s.telemetry.snapshot()),
            briefs_dir: app.config.briefs_dir.clone(),
        })
    })
}

fn format_handoff_status(
    target: HandoffAgent,
    saved_path: Option<&Path>,
    locale: UiLocale,
) -> String {
    let agent = target.label();
    match (locale == UiLocale::Ru, saved_path) {
        (true, Some(path)) => format!(
            "Handoff {agent}: пакет в буфере и {}",
            shorten_home(&path.to_string_lossy()),
        ),
        (true, None) => format!("Handoff {agent}: пакет скопирован в буфер (Ctrl+V)"),
        (false, Some(path)) => format!(
            "Handoff {agent}: pack copied to clipboard and saved to {}",
            shorten_home(&path.to_string_lossy()),
        ),
        (false, None) => format!("Handoff {agent}: pack copied to clipboard (Ctrl+V)"),
    }
}

pub fn launch_handoff_for_active(target: HandoffAgent) -> Option<usize> {
    launch_handoff_into(target, &handoff_packs_dir())
}

fn assemble_active_pack(src: &ActiveHandoffSource, target: HandoffAgent, locale: UiLocale) -> String {
    let cwd_path = Path::new(&src.cwd);
    let terminal_raw = if src.vte.is_null() { String::new() } else { extract_vte_text(src.vte) };
    let (git_branch, git_diff_stat) = collect_git_context(cwd_path);
    let active_brief = find_active_brief(cwd_path, &terminal_raw, src.briefs_dir.as_deref());
    build_handoff_pack(&HandoffPackInput {
        session_title: src.title.clone(),
        source_agent: src.agent.clone(),
        target_agent: target,
        cwd: src.cwd.clone(),
        git_branch,
        git_diff_stat,
        terminal_tail: terminal_raw,
        active_brief,
        health: src.health,
        locale,
    })
}

fn open_handoff_tab(src: ActiveHandoffSource, target: HandoffAgent, pack: &str, packs_dir: &Path, locale: UiLocale) -> usize {
    let saved = stage_handoff_pack_in(packs_dir, target, pack, current_unix_secs()).ok();
    copy_pack_to_clipboard(pack);
    let new_sid = session_ops::open_local_tab(src.profile, &src.cwd);
    session_ops::send_to_session(new_sid, target.command());
    app_state::set_footer_message(&format_handoff_status(target, saved.as_deref(), locale));
    new_sid
}

fn launch_handoff_into(target: HandoffAgent, packs_dir: &Path) -> Option<usize> {
    dismiss_tokens_popover();
    let locale = current_locale();
    let src = match read_active_handoff_source(locale) {
        Ok(src) => src,
        Err(msg) => {
            if !msg.is_empty() {
                app_state::set_footer_message(&msg);
            }
            return None;
        }
    };
    let pack = assemble_active_pack(&src, target, locale);
    Some(open_handoff_tab(src, target, &pack, packs_dir, locale))
}

pub fn trigger_handoff_by_code(code: usize) {
    if let Some(agent) = HandoffAgent::from_code(code) {
        let _ = launch_handoff_for_active(agent);
    }
}

extern "C" fn on_handoff_agent_clicked(_btn: *mut GtkWidget, code: *mut c_void) {
    trigger_handoff_by_code(code as usize);
}

fn create_health_banner() -> *mut GtkWidget {
    unsafe {
        let banner = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(banner, 0.0);
        gtk_label_set_line_wrap(banner, 1);
        gtk_label_set_line_wrap_mode(banner, PANGO_WRAP_WORD_CHAR);
        gtk_label_set_max_width_chars(banner.cast(), 64);
        add_class(banner, "ctx-health-banner");
        gtk_widget_set_no_show_all(banner, 1);
        gtk_widget_set_visible(banner, 0);
        banner
    }
}

fn create_handoff_button_row() -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        add_class(row, "ctx-handoff-row");
        for agent in HANDOFF_AGENTS {
            let label = CString::new(agent.label()).unwrap_or_else(|_| CString::new("Agent").unwrap());
            let btn = gtk_button_new_with_label(label.as_ptr());
            add_class(btn, "ctx-handoff-btn");
            gtk_widget_set_tooltip_text(btn, text::CTX_HANDOFF_TIP.cstr().as_ptr());
            app_state::connect_click(btn, on_handoff_agent_clicked, agent.code() as *mut c_void);
            gtk_box_pack_start(row, btn, 1, 1, 0);
        }
        row
    }
}

#[derive(Clone, Copy)]
pub struct HandoffControls {
    pub(crate) banner: *mut GtkWidget,
    header: *mut GtkWidget,
}

impl HandoffControls {
    pub fn apply(&self, button: *mut GtkWidget, health: ContextHealth) {
        health.apply_css_classes(button);
        let locale = current_locale();
        apply_popover_health_banner(self.banner, health, locale);
        refresh_handoff_header(self.header, locale);
    }
}

pub fn populate_handoff_controls(vbox: *mut GtkWidget) -> HandoffControls {
    unsafe {
        let banner = create_health_banner();
        gtk_box_pack_start(vbox, banner, 0, 1, 0);
        let header = gtk_label_new(text::CTX_HANDOFF_HEADER.cstr().as_ptr());
        gtk_label_set_xalign(header, 0.0);
        add_class(header, "section-header");
        gtk_box_pack_end(vbox, create_handoff_button_row(), 0, 1, 0);
        gtk_box_pack_end(vbox, header, 0, 1, 0);
        HandoffControls { banner, header }
    }
}

pub fn apply_popover_health_banner(banner: *mut GtkWidget, health: ContextHealth, locale: UiLocale) {
    if banner.is_null() {
        return;
    }
    health.apply_css_classes(banner);
    if let Some(msg) = health.banner_text(locale) {
        if let Ok(c_msg) = CString::new(msg) {
            unsafe {
                gtk_label_set_text(banner, c_msg.as_ptr());
                gtk_widget_set_visible(banner, 1);
            }
            return;
        }
    }
    unsafe { gtk_widget_set_visible(banner, 0) };
}

pub fn refresh_handoff_header(header: *mut GtkWidget, locale: UiLocale) {
    if header.is_null() {
        return;
    }
    unsafe { gtk_label_set_text(header, text::CTX_HANDOFF_HEADER.for_locale(locale).as_ptr()) };
}

pub fn build_palette_handoff_items(locale: UiLocale) -> Vec<PaletteItem> {
    let prefix = loc_text(text::CTX_HANDOFF_PALETTE_PREFIX, locale);
    let detail = loc_text(text::CTX_HANDOFF_PALETTE_DETAIL, locale);
    HANDOFF_AGENTS
        .iter()
        .map(|&agent| PaletteItem {
            kind: PaletteKind::Handoff,
            title: format!("{prefix}: {}", agent.label()),
            detail: detail.to_owned(),
            keywords: format!(
                "handoff hand off передать агенту контекст context rot pack {} {}",
                agent.slug(),
                agent.label(),
            ),
            action: PaletteAction::HandOffAgent(agent.code()),
        })
        .collect()
}

#[cfg(test)]
#[path = "agent_handoff_tests.rs"]
mod tests;
