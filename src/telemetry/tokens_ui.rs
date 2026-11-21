//! GTK token popover UI, window usage aggregation, and compact telemetry formatters.

use std::collections::HashSet;
use std::ffi::{c_void, CString};
use std::ptr;
use std::time::Instant;

use crate::agent_handoff::{self, ContextHealth};
use crate::connect_click;
use crate::ffi::*;
use crate::session::TermSession;
use crate::theme_mode::{ThemeMode, TokensMarkupColors};
use crate::ui_text as text;

use super::{ContextAlert, TelemetrySource, TokenSnapshot};

pub type ClickHandler = extern "C" fn(*mut GtkWidget, *mut c_void);

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

#[cfg(test)]
pub fn build_sessions_tokens_view(
    sessions: &[TermSession],
    active_id: usize,
    mode: ThemeMode,
    now: Instant,
) -> TokensViewData {
    let rows: Vec<(TabTokenRow, Option<String>)> = sessions
        .iter()
        .map(|s| (TabTokenRow::from_session(s, active_id, now), s.telemetry.transcript_key()))
        .collect();
    build_tokens_view_with_keys(&rows, mode)
}

pub struct TokensViewData {
    pub button_label: Option<String>,
    pub alert: ContextAlert,
    pub health: ContextHealth,
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
    let detail = snap.requests.detail(snap);
    let cost_s = snap.cost_usd.map_or(String::new(), |c| format!(" · ${c:.2}"));
    if snap.source == TelemetrySource::VteFallback && snap.total_tokens() == 0 {
        return format!("статус TUI{cost_s}");
    }
    format!(
        "input {} · cache {} · output {}{cost_s}{detail}",
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
    let title = escape_markup(&row.title);
    let context = format_ctx_span(&row.snap, colors);
    let timer = format_timer_part(row, colors);
    if !row.snap.has_data() {
        return format!("{marker}<b>{title}</b>{tag}\n    {context}  ·  {timer}");
    }
    let usage = format_io_breakdown(&row.snap);
    format!("{marker}<b>{title}</b>{tag}\n    {context}  ·  <tt>{usage}</tt>  ·  {timer}")
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
    active_health: ContextHealth,
    seen_keys: HashSet<String>,
}

impl WindowTotals {
    fn record_active(&mut self, row: &TabTokenRow) {
        if row.is_active {
            self.active_badge = row.snap.usage_badge();
            self.active_alert = row.snap.alert_level();
            self.active_health = ContextHealth::from_snapshot(&row.snap);
        }
    }

    fn record_totals(&mut self, snap: &TokenSnapshot) {
        self.tot_in = self.tot_in.saturating_add(snap.effective_input());
        self.tot_cache = self.tot_cache.saturating_add(snap.cache_tokens);
        self.tot_out = self.tot_out.saturating_add(snap.output_tokens);
        self.tot_ctx = self.tot_ctx.saturating_add(snap.context_tokens);
        if let Some(cost) = snap.cost_usd {
            self.tot_cost += cost;
            self.has_cost = true;
        }
    }

    fn record_unique(&mut self, row: &TabTokenRow, key: Option<&str>) {
        self.record_active(row);
        if let Some(k) = key {
            if !self.seen_keys.insert(k.to_owned()) {
                return;
            }
        }
        self.record_totals(&row.snap);
    }

    fn button_label(&self) -> Option<String> {
        if let Some(badge) = &self.active_badge {
            return Some(format!("Токены · {} ▾", badge.trim_start_matches("ctx ")));
        }
        None
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

pub fn build_tokens_view_with_keys(
    rows: &[(TabTokenRow, Option<String>)],
    mode: ThemeMode,
) -> TokensViewData {
    build_token_rows(rows.iter().map(|(row, key)| (row, key.as_deref())), mode)
}

fn build_token_rows<'a>(
    rows: impl IntoIterator<Item = (&'a TabTokenRow, Option<&'a str>)>,
    mode: ThemeMode,
) -> TokensViewData {
    let colors = mode.tokens_markup_colors();
    let mut totals = WindowTotals::default();
    let mut lines = Vec::new();
    for (row, key) in rows {
        totals.record_unique(row, key);
        if row.snap.has_data() || row.working_secs.is_some() || row.last_turn_secs.is_some() {
            let line = format_tab_token_markup(row, colors);
            if row.is_active {
                lines.insert(0, line);
            } else {
                lines.push(line);
            }
        }
    }
    finalize_tokens_view(totals, lines, colors)
}

fn finalize_tokens_view(
    totals: WindowTotals,
    mut lines: Vec<String>,
    colors: TokensMarkupColors,
) -> TokensViewData {
    let button_label = totals.button_label();
    if lines.is_empty() {
        return TokensViewData { button_label, alert: totals.active_alert, health: totals.active_health, markup: None };
    }
    let summary = totals.summary_markup(lines.len(), colors);
    lines.push(String::new());
    lines.push(summary);
    TokensViewData { button_label, alert: totals.active_alert, health: totals.active_health, markup: Some(lines.join("\n")) }
}

#[allow(dead_code)]
pub fn build_tokens_view_for_mode(rows: &[TabTokenRow], mode: ThemeMode) -> TokensViewData {
    build_token_rows(rows.iter().map(|row| (row, None)), mode)
}

#[cfg(test)]
pub fn build_tokens_view(rows: &[TabTokenRow]) -> TokensViewData {
    build_tokens_view_for_mode(rows, crate::theme::current_theme_mode())
}

#[derive(Clone, Copy)]
pub struct TokensMenu {
    button: *mut GtkWidget,
    popover: *mut GtkWidget,
    pub(super) body_label: *mut GtkWidget,
    handoff: agent_handoff::HandoffControls,
}

impl TokensMenu {
    pub fn new(topbar: *mut GtkWidget, on_open: ClickHandler) -> Self {
        unsafe {
            let button = gtk_button_new_with_label(text::TOKENS.as_ptr());
            gtk_widget_set_tooltip_text(button, text::TOKENS_TIP.as_ptr());
            gtk_box_pack_end(topbar, button, 0, 0, 0);
            let popover = gtk_popover_new(button);
            let (body_label, handoff) = populate_tokens_popover(popover);
            connect_click(button, on_open, popover);
            Self { button, popover, body_label, handoff }
        }
    }

    pub fn popover(&self) -> *mut GtkWidget {
        self.popover
    }

    pub fn button_widget(&self) -> *mut GtkWidget {
        self.button
    }

    #[cfg(test)]
    pub fn health_banner_widget(&self) -> *mut GtkWidget {
        self.handoff.banner
    }

    pub fn apply(&self, view: &TokensViewData) {
        if self.button.is_null() || self.body_label.is_null() {
            return;
        }
        set_class(self.button, "btn-mi-b", view.alert == ContextAlert::Warn);
        set_class(self.button, "btn-mi-c", view.alert == ContextAlert::High);
        self.handoff.apply(self.button, view.health);
        apply_button_label(self.button, view.button_label.as_deref());
        apply_body_markup(self.body_label, view.markup.as_deref());
    }
}

fn populate_tokens_popover(popover: *mut GtkWidget) -> (*mut GtkWidget, agent_handoff::HandoffControls) {
    unsafe {
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(vbox, "popover-box");
        let hdr = gtk_label_new(text::TOKENS_HEADER.as_ptr());
        gtk_label_set_xalign(hdr, 0.0);
        add_class(hdr, "section-header");
        gtk_box_pack_start(vbox, hdr, 0, 0, 0);
        let handoff = agent_handoff::populate_handoff_controls(vbox);
        let body_label = create_tokens_body_label();
        gtk_box_pack_start(vbox, create_tokens_scroll(body_label), 1, 1, 0);
        gtk_container_add(popover, vbox);
        gtk_widget_show_all(vbox);
        (body_label, handoff)
    }
}

fn create_tokens_scroll(body_label: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, 450);
        gtk_scrolled_window_set_max_content_height(scroll, 380);
        gtk_scrolled_window_set_propagate_natural_height(scroll, 1);
        gtk_container_add(scroll, body_label);
        scroll
    }
}

unsafe fn create_tokens_body_label() -> *mut GtkWidget {
    let body_label = gtk_label_new(text::TOKENS_EMPTY.as_ptr());
    gtk_label_set_xalign(body_label, 0.0);
    gtk_widget_set_valign(body_label, GTK_ALIGN_START);
    gtk_label_set_line_wrap(body_label, 1);
    gtk_label_set_line_wrap_mode(body_label, PANGO_WRAP_WORD_CHAR);
    gtk_label_set_max_width_chars(body_label.cast(), 80);
    body_label
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
