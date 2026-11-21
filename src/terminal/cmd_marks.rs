//! Per-pane OSC 133 command mark history, prompt navigation, and failed-output handoff.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::ptr;

use crate::app_state;
use crate::context_menu::{self, ContextAction};
use crate::ffi::*;
use crate::i18n::UiLocale;
use crate::osc_parser::OscEvent;
use crate::palette::{PaletteAction, PaletteItem, PaletteKind};
use crate::ui_text as text;

pub const MAX_CMD_MARKS: usize = 128;
pub const MAX_OUTPUT_LINES: usize = 200;
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_CMD_HINT_CHARS: usize = 160;
const GDK_KEY_UP: c_uint = 0xff52;
const GDK_KEY_DOWN: c_uint = 0xff54;

extern "C" {
    fn gtk_scrollable_get_vadjustment(scrollable: *mut GtkWidget) -> *mut GtkAdjustment;
    fn gtk_adjustment_get_value(adjustment: *mut GtkAdjustment) -> f64;
    fn gtk_adjustment_set_value(adjustment: *mut GtkAdjustment, value: f64);
    fn gtk_adjustment_get_lower(adjustment: *mut GtkAdjustment) -> f64;
    fn gtk_adjustment_get_upper(adjustment: *mut GtkAdjustment) -> f64;
    fn gtk_adjustment_get_page_size(adjustment: *mut GtkAdjustment) -> f64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Osc133MarkKind {
    PromptStart,
    CommandStart,
    CommandExecuted,
    CommandFinish { exit_code: Option<i32> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CmdMark {
    pub prompt_row: i64,
    pub cmd_start_row: i64,
    pub cmd_start_col: i64,
    pub output_start_row: Option<i64>,
    pub end_row: Option<i64>,
    pub exit_code: Option<i32>,
    pub command_hint: String,
    prompt_lf: u64,
    cmd_lf: u64,
    output_lf: Option<u64>,
}

impl CmdMark {
    fn at_prompt(row: i64, col: i64, lf: u64) -> Self {
        Self {
            prompt_row: row,
            cmd_start_row: row,
            cmd_start_col: col,
            output_start_row: None,
            end_row: None,
            exit_code: None,
            command_hint: String::new(),
            prompt_lf: lf,
            cmd_lf: lf,
            output_lf: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CmdMarkHistory {
    marks: VecDeque<CmdMark>,
}

impl CmdMarkHistory {
    pub fn clear(&mut self) {
        self.marks.clear();
    }

    pub fn has_marks(&self) -> bool {
        !self.marks.is_empty()
    }

    pub fn has_failed(&self) -> bool {
        self.last_failed_command().is_some()
    }

    pub fn last_command_with_output(&self) -> Option<&CmdMark> {
        self.marks
            .iter()
            .rev()
            .find(|m| m.output_start_row.is_some())
            .or_else(|| self.marks.back())
    }

    pub fn last_failed_command(&self) -> Option<&CmdMark> {
        self.marks
            .iter()
            .rev()
            .find(|m| m.exit_code.is_some_and(|code| code != 0))
    }

    pub fn prev_prompt_row(&self, current_top_row: i64) -> Option<i64> {
        self.marks
            .iter()
            .rev()
            .map(|m| m.prompt_row)
            .find(|&row| row < current_top_row)
    }

    pub fn next_prompt_row(&self, current_top_row: i64) -> Option<i64> {
        self.marks
            .iter()
            .map(|m| m.prompt_row)
            .find(|&row| row > current_top_row)
    }

    fn push_capped(&mut self, mark: CmdMark) {
        if self.marks.len() >= MAX_CMD_MARKS {
            self.marks.pop_front();
        }
        self.marks.push_back(mark);
    }

    pub fn on_prompt_start(&mut self, row: i64, col: i64, lf: u64) {
        if self.try_close_or_reuse_prompt(row, col, lf) {
            return;
        }
        self.push_capped(CmdMark::at_prompt(row, col, lf));
    }

    fn try_close_or_reuse_prompt(&mut self, row: i64, col: i64, lf: u64) -> bool {
        let Some(last) = self.marks.back_mut() else {
            return false;
        };
        if let Some(out_row) = last.output_start_row {
            if last.end_row.is_none() {
                last.end_row = Some(row.saturating_sub(1).max(out_row));
            }
            return false;
        }
        if last.end_row.is_none() && last.prompt_row == row {
            last.cmd_start_row = row;
            last.cmd_start_col = col;
            last.prompt_lf = lf;
            last.cmd_lf = lf;
            return true;
        }
        false
    }

    pub fn on_command_start(&mut self, row: i64, col: i64, lf: u64) {
        if let Some(last) = self.marks.back_mut() {
            if last.output_start_row.is_none() && last.end_row.is_none() {
                adjust_burst_row(&mut last.prompt_row, row, last.prompt_lf, lf, 0);
                last.cmd_start_row = row;
                last.cmd_start_col = col;
                last.cmd_lf = lf;
                return;
            }
        }
        self.push_capped(CmdMark::at_prompt(row, col, lf));
    }

    pub fn on_command_executed(&mut self, row: i64, lf: u64, hint: String) {
        if let Some(last) = self.marks.back_mut() {
            if last.output_start_row.is_none() && last.end_row.is_none() {
                adjust_burst_row(&mut last.prompt_row, row, last.prompt_lf, lf, 0);
                adjust_burst_row(&mut last.cmd_start_row, row, last.cmd_lf, lf, last.prompt_row);
                last.output_start_row = Some(row);
                last.output_lf = Some(lf);
                if !hint.is_empty() {
                    last.command_hint = hint;
                }
                return;
            }
        }
        let mut mark = CmdMark::at_prompt(row.saturating_sub(1).max(0), 0, lf);
        mark.output_start_row = Some(row);
        mark.output_lf = Some(lf);
        mark.command_hint = hint;
        self.push_capped(mark);
    }

    pub fn on_command_finish(&mut self, row: i64, lf: u64, exit_code: Option<i32>) {
        if let Some(last) = self.marks.back_mut() {
            if last.end_row.is_none() {
                reanchor_burst_finish(last, row, lf);
                last.end_row = Some(row);
                last.exit_code = exit_code;
                return;
            }
        }
        let mut mark = CmdMark::at_prompt(row, 0, lf);
        mark.output_start_row = Some(row);
        mark.end_row = Some(row);
        mark.exit_code = exit_code;
        self.push_capped(mark);
    }

    pub fn apply_event(&mut self, kind: Osc133MarkKind, row: i64, col: i64, lf: u64, hint: String) {
        match kind {
            Osc133MarkKind::PromptStart => self.on_prompt_start(row, col, lf),
            Osc133MarkKind::CommandStart => self.on_command_start(row, col, lf),
            Osc133MarkKind::CommandExecuted => self.on_command_executed(row, lf, hint),
            Osc133MarkKind::CommandFinish { exit_code } => self.on_command_finish(row, lf, exit_code),
        }
    }
}

fn adjust_burst_row(slot: &mut i64, current_row: i64, prev_lf: u64, cur_lf: u64, floor: i64) {
    if *slot == current_row && cur_lf > prev_lf {
        let delta = (cur_lf - prev_lf).min(i64::MAX as u64) as i64;
        *slot = current_row.saturating_sub(delta).max(floor);
    }
}

fn reanchor_row(slot: &mut i64, row: i64, lf: u64, prev_lf: u64, out_delta: i64, floor: i64) {
    if *slot + out_delta >= row && lf >= prev_lf {
        let delta = (lf - prev_lf).min(i64::MAX as u64) as i64;
        *slot = row.saturating_sub(delta).max(floor);
    }
}

fn reanchor_burst_finish(mark: &mut CmdMark, row: i64, lf: u64) {
    let Some(out_lf) = mark.output_lf else {
        return;
    };
    if mark.output_start_row != Some(row) || lf <= out_lf {
        return;
    }
    let out_delta = (lf - out_lf).min(i64::MAX as u64) as i64;
    reanchor_row(&mut mark.prompt_row, row, lf, mark.prompt_lf, out_delta, 0);
    reanchor_row(&mut mark.cmd_start_row, row, lf, mark.cmd_lf, out_delta, mark.prompt_row);
    mark.output_start_row = Some(row.saturating_sub(out_delta).max(mark.cmd_start_row));
}

pub fn command_nav_delta(keyval: c_uint, state: c_uint) -> Option<i32> {
    let mods = GDK_CONTROL_MASK | GDK_SHIFT_MASK | GDK_MOD1_MASK;
    if (state & mods) != (GDK_CONTROL_MASK | GDK_SHIFT_MASK) {
        return None;
    }
    match keyval {
        GDK_KEY_UP => Some(-1),
        GDK_KEY_DOWN => Some(1),
        _ => None,
    }
}

pub fn vte_cursor_pos(vte: *mut VteTerminal) -> (i64, i64) {
    if vte.is_null() {
        return (0, 0);
    }
    let (mut col, mut row) = (0isize, 0isize);
    unsafe { vte_terminal_get_cursor_position(vte, &mut col, &mut row) };
    (col as i64, row as i64)
}

pub fn vte_top_row(vte: *mut VteTerminal) -> Option<i64> {
    if vte.is_null() {
        return None;
    }
    let vadj = unsafe { gtk_scrollable_get_vadjustment(vte) };
    if vadj.is_null() {
        return None;
    }
    Some(unsafe { gtk_adjustment_get_value(vadj) }.floor() as i64)
}

pub fn scroll_vte_to_row(vte: *mut VteTerminal, row: i64) -> bool {
    if vte.is_null() {
        return false;
    }
    let vadj = unsafe { gtk_scrollable_get_vadjustment(vte) };
    if vadj.is_null() {
        return false;
    }
    unsafe {
        let lower = gtk_adjustment_get_lower(vadj);
        let upper = gtk_adjustment_get_upper(vadj);
        let page = gtk_adjustment_get_page_size(vadj);
        let max_val = (upper - page).max(lower);
        let clamped = (row as f64).clamp(lower, max_val);
        gtk_adjustment_set_value(vadj, clamped);
    }
    true
}

fn take_vte_string(raw: *mut c_char) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned();
    unsafe { g_free(raw.cast()) };
    Some(text)
}

pub fn read_vte_range(
    vte: *mut VteTerminal,
    start_row: i64,
    start_col: i64,
    end_row: i64,
) -> String {
    if vte.is_null() || end_row < start_row {
        return String::new();
    }
    unsafe {
        let cols = vte_terminal_get_column_count(vte).max(256);
        let mut len = 0usize;
        let ptr = vte_terminal_get_text_range_format(
            vte,
            VTE_FORMAT_TEXT,
            start_row.max(0) as isize,
            start_col.max(0) as isize,
            end_row.max(0) as isize,
            cols,
            &mut len,
        );
        take_vte_string(ptr).unwrap_or_default()
    }
}

pub fn extract_command_hint(
    vte: *mut VteTerminal,
    cmd_start_row: i64,
    cmd_start_col: i64,
    exec_row: i64,
) -> String {
    let last_row = exec_row.saturating_sub(1).max(cmd_start_row);
    let raw = read_vte_range(vte, cmd_start_row, cmd_start_col, last_row);
    sanitize_command_hint(&raw)
}

pub fn sanitize_command_hint(raw: &str) -> String {
    let collapsed: String = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = String::new();
    for ch in collapsed.chars().take(MAX_CMD_HINT_CHARS) {
        if !ch.is_control() {
            out.push(ch);
        }
    }
    out
}

pub fn extract_mark_output(vte: *mut VteTerminal, mark: &CmdMark) -> String {
    let (_, cur_row) = vte_cursor_pos(vte);
    let raw_start = mark.output_start_row.unwrap_or(mark.cmd_start_row).max(0);
    let end = mark.end_row.unwrap_or(cur_row).max(raw_start);
    let max_span = MAX_OUTPUT_LINES as i64;
    let start = if end - raw_start >= max_span {
        end - max_span + 1
    } else {
        raw_start
    };
    let raw = read_vte_range(vte, start, 0, end);
    trim_bounded_output(&raw, MAX_OUTPUT_LINES, MAX_OUTPUT_BYTES)
}

fn sanitize_output_text(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .chars()
        .filter(|&c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

fn slice_tail_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn collect_trimmed_lines(clean: &str, max_lines: usize) -> (String, bool) {
    let mut lines: Vec<&str> = clean.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let truncated = max_lines > 0 && lines.len() > max_lines;
    if truncated {
        let drop_count = lines.len() - max_lines;
        lines.drain(..drop_count);
    }
    (lines.join("\n"), truncated)
}

pub fn trim_bounded_output(raw: &str, max_lines: usize, max_bytes: usize) -> String {
    let clean = sanitize_output_text(raw);
    let (joined, truncated) = collect_trimmed_lines(&clean, max_lines);
    if max_bytes > 0 && joined.len() > max_bytes {
        let tail = slice_tail_utf8(&joined, max_bytes);
        return format!("...[truncated]\n{tail}");
    }
    if truncated {
        format!("...[truncated]\n{joined}")
    } else {
        joined
    }
}

pub fn safe_markdown_fence(body: &str) -> String {
    let longest = body.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(3.max(longest + 1))
}

fn inline_code_span(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let ticks = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') { " " } else { "" };
    format!("{ticks}{pad}{text}{pad}{ticks}")
}

pub fn format_failed_command_markdown(mark: &CmdMark, raw_output: &str) -> String {
    let code = mark.exit_code.unwrap_or(1);
    let body = trim_bounded_output(raw_output, MAX_OUTPUT_LINES, MAX_OUTPUT_BYTES);
    let fence = safe_markdown_fence(&body);
    let hint = sanitize_command_hint(&mark.command_hint);
    let header = if hint.is_empty() {
        format!("Command failed (exit {code}):")
    } else {
        format!("Command failed (exit {code}): {}", inline_code_span(&hint))
    };
    if body.is_empty() {
        format!("{header}\n{fence}text\n(no output)\n{fence}\n")
    } else {
        format!("{header}\n{fence}text\n{body}\n{fence}\n")
    }
}

struct MarkIdleJob {
    vte_addr: usize,
    kind: Osc133MarkKind,
    lf: u64,
}

pub fn enqueue_osc133_event(vte_addr: usize, ev: &OscEvent, lf: u64) {
    let kind = match *ev {
        OscEvent::PromptStart => Osc133MarkKind::PromptStart,
        OscEvent::CommandStart => Osc133MarkKind::CommandStart,
        OscEvent::CommandExecuted => Osc133MarkKind::CommandExecuted,
        OscEvent::CommandFinish { exit_code } => Osc133MarkKind::CommandFinish { exit_code },
        OscEvent::Osc52Write(_) | OscEvent::Osc52BlockedQuery => return,
    };
    if vte_addr == 0 {
        return;
    }
    let job = Box::into_raw(Box::new(MarkIdleJob { vte_addr, kind, lf }));
    unsafe { g_timeout_add(30, on_osc133_mark_idle, job.cast()) };
}

extern "C" fn on_osc133_mark_idle(data: *mut c_void) -> c_int {
    if data.is_null() {
        return 0;
    }
    let job = unsafe { *Box::from_raw(data.cast::<MarkIdleJob>()) };
    apply_mark_to_live_vte(job.vte_addr as *mut GtkWidget, job.kind, job.lf);
    0
}

fn apply_mark_to_live_vte(vte: *mut GtkWidget, kind: Osc133MarkKind, lf: u64) {
    let _ = app_state::with_app_mut(|app| {
        let session = app.sessions.iter_mut().find(|s| s.has_vte(vte))?;
        let pane = session.pane_registry.by_vte_mut(vte)?;
        let (col, row) = vte_cursor_pos(vte);
        let hint = if matches!(kind, Osc133MarkKind::CommandExecuted) {
            pane.cmd_marks
                .marks
                .back()
                .map(|m| extract_command_hint(vte, m.cmd_start_row, m.cmd_start_col, row))
                .unwrap_or_default()
        } else {
            String::new()
        };
        pane.cmd_marks.apply_event(kind, row, col, lf, hint);
        Some(())
    });
}

fn active_pane_vte() -> Option<*mut GtkWidget> {
    app_state::with_app(|app| {
        app.sessions
            .iter()
            .find(|s| s.id == app.active_id)
            .map(crate::session::TermSession::active_vte)
    })
    .flatten()
    .filter(|w| !w.is_null())
}

pub fn menu_flags_for_session(session: &crate::session::TermSession, vte: *mut GtkWidget) -> (bool, bool) {
    session
        .pane_registry
        .by_vte(vte)
        .map_or((false, false), |p| (p.cmd_marks.has_marks(), p.cmd_marks.has_failed()))
}

pub fn clear_for_vte(vte: *mut GtkWidget) {
    if vte.is_null() {
        return;
    }
    let _ = app_state::with_app_mut(|app| {
        let session = app.sessions.iter_mut().find(|s| s.has_vte(vte))?;
        session.pane_registry.by_vte_mut(vte)?.cmd_marks.clear();
        Some(())
    });
}

fn resolve_jump_target(
    vte: *mut GtkWidget,
    top_row: i64,
    delta: i32,
) -> Option<Result<i64, crate::i18n::UiText>> {
    app_state::with_app(|app| {
        let session = app.sessions.iter().find(|s| s.has_vte(vte))?;
        let pane = session.pane_registry.by_vte(vte)?;
        if !pane.cmd_marks.has_marks() {
            return Some(Err(text::CMD_NO_MARKS));
        }
        let target = if delta < 0 {
            pane.cmd_marks.prev_prompt_row(top_row).ok_or(text::CMD_NO_PREV)
        } else {
            pane.cmd_marks.next_prompt_row(top_row).ok_or(text::CMD_NO_NEXT)
        };
        Some(target)
    })
    .flatten()
}

pub fn jump_command_for_vte(vte: *mut GtkWidget, delta: i32) -> bool {
    let Some(top_row) = vte_top_row(vte) else {
        return false;
    };
    match resolve_jump_target(vte, top_row, delta) {
        Some(Ok(row)) => scroll_vte_to_row(vte, row),
        Some(Err(msg)) => {
            app_state::set_footer_message(msg.as_str());
            false
        }
        None => false,
    }
}

pub fn handle_nav_shortcut(keyval: c_uint, state: c_uint) -> bool {
    let Some(delta) = command_nav_delta(keyval, state) else {
        return false;
    };
    if let Some(vte) = active_pane_vte() {
        let _ = jump_command_for_vte(vte, delta);
    }
    true
}

pub fn copy_last_output_for_vte(vte: *mut GtkWidget) -> bool {
    if vte.is_null() {
        return false;
    }
    let mark = app_state::with_app(|app| {
        let session = app.sessions.iter().find(|s| s.has_vte(vte))?;
        let pane = session.pane_registry.by_vte(vte)?;
        pane.cmd_marks.last_command_with_output().cloned()
    })
    .flatten();
    let Some(mark) = mark else {
        app_state::set_footer_message(text::CMD_NO_MARKS.as_str());
        return false;
    };
    let output = extract_mark_output(vte, &mark);
    context_menu::copy_text_to_clipboard(&output);
    app_state::set_footer_message(text::CMD_OUTPUT_COPIED.as_str());
    true
}

pub fn pick_agent_target(
    targets: Vec<crate::markdown::Target>,
    source_sid: Option<usize>,
    source_vte: *mut GtkWidget,
) -> Option<crate::markdown::Target> {
    if let Some(sid) = source_sid {
        if let Some(pos) = targets.iter().position(|t| t.sid == sid && t.vte != source_vte) {
            return Some(targets.into_iter().nth(pos)?);
        }
    }
    if let Some(pos) = targets.iter().position(|t| t.vte != source_vte) {
        return Some(targets.into_iter().nth(pos)?);
    }
    targets.into_iter().next()
}

fn find_failed_mark(vte: *mut GtkWidget) -> Option<(usize, CmdMark)> {
    app_state::with_app(|app| {
        let session = app.sessions.iter().find(|s| s.has_vte(vte))?;
        let pane = session.pane_registry.by_vte(vte)?;
        let mark = pane.cmd_marks.last_failed_command().cloned()?;
        Some((session.id, mark))
    })
    .flatten()
}

pub fn send_failed_output_for_vte(vte: *mut GtkWidget) -> bool {
    if vte.is_null() {
        return false;
    }
    let Some((source_sid, mark)) = find_failed_mark(vte) else {
        app_state::set_footer_message(text::CMD_NO_FAILED.as_str());
        return false;
    };
    let output = extract_mark_output(vte, &mark);
    let payload = format_failed_command_markdown(&mark, &output);
    let targets = crate::markdown::targets();
    if let Some(target) = pick_agent_target(targets, Some(source_sid), vte) {
        let label = target.label.clone();
        if crate::markdown::deliver(target, payload.clone()) {
            app_state::set_footer_message(&format!("{} · {label}", text::CMD_SENT_TO_AGENT.as_str()));
            return true;
        }
    }
    context_menu::copy_text_to_clipboard(&payload);
    app_state::set_footer_message(text::CMD_COPIED_NO_AGENT.as_str());
    true
}

pub fn try_execute_context_action(action: ContextAction) -> Result<(), ContextAction> {
    match action {
        ContextAction::PrevCommand(vte) => {
            let _ = jump_command_for_vte(vte, -1);
        }
        ContextAction::NextCommand(vte) => {
            let _ = jump_command_for_vte(vte, 1);
        }
        ContextAction::CopyCommandOutput(vte) => {
            let _ = copy_last_output_for_vte(vte);
        }
        ContextAction::SendFailedToAgent(vte) => {
            let _ = send_failed_output_for_vte(vte);
        }
        other => return Err(other),
    }
    Ok(())
}

fn palette_item(
    kind: PaletteKind,
    title: crate::i18n::UiText,
    detail: crate::i18n::UiText,
    keywords: &str,
    action: PaletteAction,
    locale: UiLocale,
) -> PaletteItem {
    PaletteItem {
        kind,
        title: title.for_locale(locale).to_string_lossy().into_owned(),
        detail: detail.for_locale(locale).to_string_lossy().into_owned(),
        keywords: keywords.to_owned(),
        action,
    }
}

fn make_prev_cmd_item(locale: UiLocale) -> PaletteItem {
    let kw = "osc133 prompt prev previous command jump up предыдущая команда промпт";
    palette_item(PaletteKind::Search, text::CTX_PREV_COMMAND, text::CMD_PREV_DETAIL, kw, PaletteAction::PrevCommand, locale)
}

fn make_next_cmd_item(locale: UiLocale) -> PaletteItem {
    let kw = "osc133 prompt next command jump down следующая команда промпт";
    palette_item(PaletteKind::Search, text::CTX_NEXT_COMMAND, text::CMD_NEXT_DETAIL, kw, PaletteAction::NextCommand, locale)
}

fn make_copy_output_item(locale: UiLocale) -> PaletteItem {
    let kw = "osc133 copy command output clipboard скопировать вывод команды";
    palette_item(PaletteKind::Handoff, text::CTX_COPY_CMD_OUTPUT, text::CMD_COPY_OUTPUT_DETAIL, kw, PaletteAction::CopyCommandOutput, locale)
}

fn make_send_failed_item(locale: UiLocale) -> PaletteItem {
    let kw = "osc133 failed error exit output agent send отправить ошибку агенту";
    palette_item(PaletteKind::Agent, text::CTX_SEND_FAILED_AGENT, text::CMD_SEND_FAILED_DETAIL, kw, PaletteAction::SendFailedToAgent, locale)
}

pub fn push_palette_items(out: &mut Vec<PaletteItem>, locale: UiLocale) {
    out.push(make_prev_cmd_item(locale));
    out.push(make_next_cmd_item(locale));
    out.push(make_copy_output_item(locale));
    out.push(make_send_failed_item(locale));
}

pub fn execute_palette_action(action: PaletteAction) {
    let vte = active_pane_vte().unwrap_or(ptr::null_mut());
    match action {
        PaletteAction::PrevCommand => {
            let _ = jump_command_for_vte(vte, -1);
        }
        PaletteAction::NextCommand => {
            let _ = jump_command_for_vte(vte, 1);
        }
        PaletteAction::CopyCommandOutput => {
            let _ = copy_last_output_for_vte(vte);
        }
        PaletteAction::SendFailedToAgent => {
            let _ = send_failed_output_for_vte(vte);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc_parser::OscStreamParser;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

    fn record_sample_pair(hist: &mut CmdMarkHistory) {
        hist.on_prompt_start(0, 0, 0);
        hist.on_command_start(0, 2, 0);
        hist.on_command_executed(1, 1, "cargo test".to_owned());
        hist.on_command_finish(5, 5, Some(0));
        hist.on_prompt_start(6, 0, 6);
        hist.on_command_start(6, 2, 6);
        hist.on_command_executed(7, 7, "false".to_owned());
        hist.on_command_finish(9, 9, Some(1));
    }

    #[test]
    fn tracks_osc133_lifecycle_and_caps_ring_buffer() {
        let mut hist = CmdMarkHistory::default();
        record_sample_pair(&mut hist);
        let last = hist.last_command_with_output().expect("last command");
        assert_eq!((last.command_hint.as_str(), last.exit_code), ("false", Some(1)));
        assert_eq!(hist.last_failed_command(), Some(last));
        assert_eq!((hist.prev_prompt_row(10), hist.prev_prompt_row(6), hist.prev_prompt_row(0)), (Some(6), Some(0), None));
        assert_eq!((hist.next_prompt_row(0), hist.next_prompt_row(6)), (Some(6), None));
        for i in 0..(MAX_CMD_MARKS + 20) {
            let row = (i as i64) * 3 + 20;
            hist.on_prompt_start(row, 0, row as u64);
        }
        assert_eq!(hist.marks.len(), MAX_CMD_MARKS);
    }

    fn osc_event_to_kind(ev: OscEvent) -> Option<Osc133MarkKind> {
        match ev {
            OscEvent::PromptStart => Some(Osc133MarkKind::PromptStart),
            OscEvent::CommandStart => Some(Osc133MarkKind::CommandStart),
            OscEvent::CommandExecuted => Some(Osc133MarkKind::CommandExecuted),
            OscEvent::CommandFinish { exit_code } => Some(Osc133MarkKind::CommandFinish { exit_code }),
            _ => None,
        }
    }

    #[test]
    fn adjusts_burst_rows_when_osc133_arrives_in_one_chunk() {
        let mut parser = OscStreamParser::new();
        let (mut out, mut events) = (Vec::new(), Vec::new());
        let stream = b"\x1b]133;A\x07$ \x1b]133;B\x07cargo build\n\x1b]133;C\x07err1\nerr2\n\x1b]133;D;101\x07";
        parser.feed_with_lf(stream, &mut out, &mut events);
        assert_eq!(events.len(), 4);
        let mut hist = CmdMarkHistory::default();
        for (ev, lf) in events {
            if let Some(kind) = osc_event_to_kind(ev) {
                hist.apply_event(kind, 13, 0, lf, "cargo build".to_owned());
            }
        }
        let m = hist.last_failed_command().expect("failed burst mark");
        assert_eq!((m.prompt_row, m.cmd_start_row, m.output_start_row, m.end_row, m.exit_code), (10, 10, Some(11), Some(13), Some(101)));
    }

    #[test]
    fn trims_bounded_output_and_keeps_utf8_boundaries() {
        let many: String = (0..260).map(|i| format!("line-{i}\n")).collect();
        let trimmed = trim_bounded_output(&many, MAX_OUTPUT_LINES, MAX_OUTPUT_BYTES);
        assert!(trimmed.starts_with("...[truncated]\nline-60\n"));
        assert!(trimmed.ends_with("line-259"));

        let cyr = "Ошибка сборки: проверка UTF-8 границы\n".repeat(600);
        let bounded = trim_bounded_output(&cyr, 1000, MAX_OUTPUT_BYTES);
        assert!(bounded.starts_with("...[truncated]\n"));
        assert!(bounded.len() <= MAX_OUTPUT_BYTES + 32);
    }

    #[test]
    fn formats_failed_markdown_with_nested_fence_safety() {
        let mut mark = CmdMark::at_prompt(10, 2, 10);
        mark.exit_code = Some(101);
        mark.command_hint = "cargo test `foo`".to_owned();
        let raw = "error:\n```rust\nfn boom() {}\n````\nfailed";
        let md = format_failed_command_markdown(&mark, raw);
        assert!(md.starts_with("Command failed (exit 101): `` cargo test `foo` ``\n`````text\n"));
        assert!(md.ends_with("\n`````\n"));
        assert!(!md.contains("->") && !md.contains('→') && !md.contains("=>"));
    }

    #[test]
    fn decodes_ctrl_shift_arrow_nav_shortcuts() {
        let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
        assert_eq!(command_nav_delta(GDK_KEY_UP, cs), Some(-1));
        assert_eq!(command_nav_delta(GDK_KEY_DOWN, cs), Some(1));
        assert_eq!(command_nav_delta(GDK_KEY_UP, GDK_CONTROL_MASK), None);
        assert_eq!(command_nav_delta(GDK_KEY_UP, cs | GDK_MOD1_MASK), None);
    }

    fn feed_vte_str(vte: *mut VteTerminal, s: &str) {
        let before = vte_cursor_pos(vte);
        unsafe { vte_terminal_feed(vte, s.as_ptr() as *const c_char, s.len() as isize) };
        for _ in 0..25 {
            pump_gtk_events();
            if vte_cursor_pos(vte) != before {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn feed_step_lines(vte: *mut VteTerminal, lf: &mut u64, step: usize) {
        for n in 0..22 {
            let line = if step == 4 && n == 5 {
                "error[E0308]: ```mismatch```\r\n".to_owned()
            } else {
                format!("step-{step}-line-{n}\r\n")
            };
            feed_vte_str(vte, &line);
            *lf += 1;
        }
    }

    fn feed_synthetic_command(
        vte: *mut VteTerminal,
        hist: &mut CmdMarkHistory,
        lf: &mut u64,
        step: usize,
    ) {
        let (col, row) = vte_cursor_pos(vte);
        hist.on_prompt_start(row, col, *lf);
        feed_vte_str(vte, "$ ");
        let (b_col, b_row) = vte_cursor_pos(vte);
        hist.on_command_start(b_row, b_col, *lf);
        let cmd = if step == 4 { "cargo check" } else { "echo ok" };
        feed_vte_str(vte, &format!("{cmd}\r\n"));
        *lf += 1;
        let (_, c_row) = vte_cursor_pos(vte);
        let hint = extract_command_hint(vte, b_row, b_col, c_row);
        hist.on_command_executed(c_row, *lf, hint);
        feed_step_lines(vte, lf, step);
        let (_, d_row) = vte_cursor_pos(vte);
        hist.on_command_finish(d_row, *lf, Some(if step == 4 { 101 } else { 0 }));
    }

    fn wait_vte_scrolled(vte: *mut VteTerminal) -> i64 {
        for _ in 0..10 {
            pump_gtk_events();
            if let Some(top) = vte_top_row(vte).filter(|&r| r > 0) {
                return top;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        vte_top_row(vte).expect("vadj top")
    }

    fn verify_headless_scroll(vte: *mut VteTerminal, hist: &CmdMarkHistory) {
        let top_before = wait_vte_scrolled(vte);
        let prompts: Vec<i64> = hist.marks.iter().map(|m| m.prompt_row).collect();
        let prev1 = hist.prev_prompt_row(top_before).expect("prev1 prompt");
        let prev2 = hist
            .prev_prompt_row(prev1)
            .unwrap_or_else(|| panic!("prev2: top={top_before} prev1={prev1} prompts={prompts:?}"));
        assert!(scroll_vte_to_row(vte, prev2));
        pump_gtk_events();
        assert_eq!(vte_top_row(vte), Some(prev2));
        let next = hist.next_prompt_row(prev2).expect("next prompt");
        assert_eq!(next, prev1);
        assert!(scroll_vte_to_row(vte, next));
        pump_gtk_events();
        assert_eq!(vte_top_row(vte), Some(next));
    }

    fn verify_headless_failed_output(vte: *mut VteTerminal, hist: &CmdMarkHistory) {
        let failed = hist.last_failed_command().expect("failed mark");
        assert_eq!(failed.exit_code, Some(101));
        assert_eq!(failed.command_hint, "cargo check");
        let out = extract_mark_output(vte, failed);
        assert!(out.contains("error[E0308]: ```mismatch```"), "got: {out}");
        let md = format_failed_command_markdown(failed, &out);
        assert!(md.contains("Command failed (exit 101): `cargo check`"));
        assert!(md.contains("````text\n"));
    }

    fn run_headless_vte_marks_check() {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            gtk_window_set_default_size(win, 640, 260);
            let vte = vte_terminal_new();
            vte_terminal_set_scrollback_lines(vte, 2000);
            let scroll = crate::split_pane::wrap_vte_scroll(vte);
            gtk_container_add(win, scroll);
            gtk_widget_show_all(win);
            pump_gtk_events();

            let mut hist = CmdMarkHistory::default();
            let mut lf = 0u64;
            for step in 0..5 {
                feed_synthetic_command(vte, &mut hist, &mut lf, step);
            }
            verify_headless_scroll(vte, &hist);
            verify_headless_failed_output(vte, &hist);

            gtk_widget_destroy(win);
            pump_gtk_events();
        }
    }

    #[test]
    fn headless_vte_tracks_prompts_scrolls_and_extracts_failed_output() {
        with_headless_gtk(run_headless_vte_marks_check);
    }

    fn assert_arrow_free(items: &[PaletteItem]) {
        for item in items {
            for s in [&item.title, &item.detail] {
                assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"));
            }
        }
    }

    #[test]
    fn palette_items_are_bilingual_and_arrow_free() {
        let (mut ru, mut en) = (Vec::new(), Vec::new());
        push_palette_items(&mut ru, UiLocale::Ru);
        push_palette_items(&mut en, UiLocale::En);
        assert_eq!((ru.len(), en.len()), (4, 4));
        assert_eq!(ru[0].title, "Предыдущая команда");
        assert_eq!(en[0].title, "Previous Command");
        assert_eq!(ru[3].title, "Отправить ошибку в агента");
        assert_eq!(en[3].title, "Send Failed Output to Agent");
        assert_arrow_free(&ru);
        assert_arrow_free(&en);
    }

    #[test]
    fn cmd_marks_meets_grade_b_mi_target() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("cmd_marks.rs"));
        assert!(mi >= 60.0, "cmd_marks.rs MI {mi:.2} < 60.0");
    }
}
