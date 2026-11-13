//! Pane-aware mouse selection, Cairo highlight overlay, and clipboard slicing for split tmux panes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_int, c_uint, c_void, CStr, CString};
use std::ptr;

use crate::ffi::*;
use crate::theme::current_theme_mode;
use crate::theme_mode::ThemeMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneBounds { pub left_col: usize, pub right_col: usize, pub top_row: usize, pub bottom_row: usize }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellPos { pub row: usize, pub col: usize }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cell { Empty, Char(char), WideTail }
#[derive(Clone, Copy, Debug)]
struct VertRun { col: usize, r_start: usize, r_end: usize }
#[derive(Clone, Debug)]
pub struct TmuxLayout { cols: usize, content_rows: usize, vert_runs: Vec<VertRun> }
#[derive(Clone, Debug)]
pub struct ScreenGrid { cols: usize, rows: usize, lines: Vec<String>, cells: Vec<Vec<Cell>> }
#[derive(Clone, Copy, Debug)]
struct CellMetrics { cw: f64, ch: f64, pad_left: f64, pad_top: f64, cols: usize, rows: usize }
#[derive(Clone, Copy, Debug, PartialEq)]
struct HighlightRect { x: f64, y: f64, w: f64, h: f64 }
#[derive(Clone, Debug)]
struct VteTmuxState {
    bounds: PaneBounds, anchor: CellPos, cursor: CellPos, active: bool, dragging: bool, start_px: (f64, f64),
}

thread_local! {
    static VTE_STATES: RefCell<HashMap<usize, VteTmuxState>> = RefCell::new(HashMap::new());
}

fn in_u32_ranges(u: u32, ranges: &[(u32, u32)]) -> bool {
    for &(lo, hi) in ranges {
        if u >= lo && u <= hi {
            return true;
        }
    }
    false
}

fn is_zero_width_code(u: u32) -> bool {
    const ZERO_RANGES: &[(u32, u32)] = &[
        (0x00, 0x1f), (0x7f, 0x9f), (0x0300, 0x036f), (0x1ab0, 0x1aff), (0x1dc0, 0x1dff),
        (0x200b, 0x200f), (0x202a, 0x202e), (0x2060, 0x206f), (0x20d0, 0x20ff), (0xfe00, 0xfe0f),
        (0xfe20, 0xfe2f), (0xfeff, 0xfeff), (0xe0100, 0xe01ef),
    ];
    in_u32_ranges(u, ZERO_RANGES)
}

fn is_wide_symbol_code(u: u32) -> bool {
    const SYM_RANGES: &[(u32, u32)] = &[
        (0x231a, 0x231b), (0x2329, 0x232a), (0x23e9, 0x23ec), (0x23f0, 0x23f0), (0x23f3, 0x23f3),
        (0x25fd, 0x25fe), (0x2614, 0x2615), (0x2648, 0x2653), (0x267f, 0x267f), (0x2693, 0x2693),
        (0x26a1, 0x26a1), (0x26aa, 0x26ab), (0x26bd, 0x26be), (0x26c4, 0x26c5), (0x26ce, 0x26ce),
        (0x26d4, 0x26d4), (0x26ea, 0x26ea), (0x26f2, 0x26f3), (0x26f5, 0x26f5), (0x26fa, 0x26fa),
        (0x26fd, 0x26fd), (0x2705, 0x2705), (0x270a, 0x270b), (0x2728, 0x2728), (0x274c, 0x274c),
        (0x274e, 0x274e), (0x2753, 0x2755), (0x2757, 0x2757), (0x2795, 0x2797), (0x27b0, 0x27b0),
        (0x27bf, 0x27bf), (0x2b1b, 0x2b1c), (0x2b50, 0x2b50), (0x2b55, 0x2b55),
    ];
    in_u32_ranges(u, SYM_RANGES)
}

fn is_wide_cjk_code(u: u32) -> bool {
    const CJK_RANGES: &[(u32, u32)] = &[
        (0x1100, 0x115f), (0x2e80, 0x303e), (0x3040, 0xa4cf), (0xac00, 0xd7a3), (0xf900, 0xfaff),
        (0xfe10, 0xfe19), (0xfe30, 0xfe6f), (0xff01, 0xff60), (0xffe0, 0xffe6), (0x1f004, 0x1f004),
        (0x1f0cf, 0x1f0cf), (0x1f18e, 0x1f18e), (0x1f191, 0x1f19a), (0x1f200, 0x1fbff), (0x20000, 0x3fffd),
    ];
    in_u32_ranges(u, CJK_RANGES)
}

pub fn char_cell_width(ch: char) -> usize {
    let u = ch as u32;
    if is_zero_width_code(u) {
        0
    } else if is_wide_symbol_code(u) || is_wide_cjk_code(u) {
        2
    } else {
        1
    }
}

fn build_row_cells(line: &str, cols: usize) -> Vec<Cell> {
    let (mut row, mut col) = (vec![Cell::Empty; cols], 0usize);
    for ch in line.chars() {
        let w = char_cell_width(ch);
        if w == 0 { continue; }
        if col + w > cols { break; }
        row[col] = Cell::Char(ch);
        if w == 2 { row[col + 1] = Cell::WideTail; }
        col += w;
    }
    row
}

impl ScreenGrid {
    pub fn from_text(raw: &str, cols: usize, rows: usize) -> Self {
        let mut lines: Vec<String> = raw
            .split('\n')
            .take(rows)
            .map(|l| l.trim_end_matches('\r').to_owned())
            .collect();
        while lines.len() < rows {
            lines.push(String::new());
        }
        let cells = lines.iter().map(|l| build_row_cells(l, cols)).collect();
        Self { cols, rows, lines, cells }
    }

    fn cell_char(&self, row: usize, col: usize) -> Option<char> {
        match self.cells.get(row).and_then(|r| r.get(col)).copied() {
            Some(Cell::Char(ch)) => Some(ch),
            _ => None,
        }
    }

    fn line_at(&self, row: usize) -> &str {
        if let Some(line) = self.lines.get(row) {
            line.as_str()
        } else {
            ""
        }
    }
}

pub fn slice_line_cols(line: &str, from_col: usize, to_col: usize) -> String {
    let (mut out, mut col, mut prev_in) = (String::new(), 0usize, false);
    for ch in line.chars().take_while(|_| from_col <= to_col) {
        let w = char_cell_width(ch);
        if w == 0 {
            if prev_in { out.push(ch); }
            continue;
        }
        if col > to_col { break; }
        prev_in = col >= from_col;
        if prev_in { out.push(ch); } else if col + w > from_col { out.push(' '); }
        col += w;
    }
    out
}

fn is_pure_vert_char(ch: char) -> bool {
    matches!(ch, '│' | '┃' | '║')
}

fn is_vert_border_char(ch: char) -> bool {
    is_pure_vert_char(ch)
        || matches!(
            ch,
            '├' | '┤' | '┬' | '┴' | '┼' | '┣' | '┫' | '┳' | '┻' | '╋' | '╠' | '╣' | '╦' | '╩' | '╬'
        )
}

fn is_pure_horiz_char(ch: char) -> bool {
    matches!(ch, '─' | '━' | '═')
}

fn is_horiz_border_char(ch: char) -> bool {
    is_pure_horiz_char(ch)
        || matches!(
            ch,
            '├' | '┤' | '┬' | '┴' | '┼' | '┣' | '┫' | '┳' | '┻' | '╋' | '╠' | '╣' | '╦' | '╩' | '╬'
        )
}

fn is_horiz_segment(grid: &ScreenGrid, row: usize, left: usize, right: usize) -> bool {
    let width = right.saturating_sub(left) + 1;
    if width < 3 || row >= grid.rows {
        return false;
    }
    let mut pure = 0usize;
    for col in left..=right {
        let Some(ch) = grid.cell_char(row, col).filter(|&c| is_horiz_border_char(c)) else {
            return false;
        };
        pure += usize::from(is_pure_horiz_char(ch));
    }
    pure * 2 >= width
}

fn looks_like_tmux_status_text(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with('[') && trimmed.contains("] ") && trimmed.contains(':') {
        return true;
    }
    trimmed
        .split_whitespace()
        .any(|tok| tok.contains(':') && tok.ends_with(['*', '-', '!', '#']))
}

fn has_vert_terminator_at_last_row(grid: &ScreenGrid) -> bool {
    if grid.rows < 3 || grid.cols < 3 { return false; }
    let last = grid.rows - 1;
    if grid.line_at(last).trim().is_empty() || is_horiz_segment(grid, last, 0, grid.cols - 1) { return false; }
    let min_hits = last.div_ceil(2).max(2);
    (1..(grid.cols - 1)).any(|col| {
        let hits = (0..last).filter(|&r| grid.cell_char(r, col).is_some_and(is_vert_border_char)).count();
        hits >= min_hits && !grid.cell_char(last, col).is_some_and(is_vert_border_char)
    })
}

fn detect_content_rows(grid: &ScreenGrid) -> usize {
    if grid.rows < 3 {
        return grid.rows;
    }
    let last = grid.rows - 1;
    if has_vert_terminator_at_last_row(grid) {
        return last;
    }
    let has_horiz = (1..last).any(|r| is_horiz_segment(grid, r, 0, grid.cols - 1));
    if has_horiz && looks_like_tmux_status_text(grid.line_at(last)) {
        return last;
    }
    grid.rows
}

fn collect_col_runs(grid: &ScreenGrid, col: usize, content_rows: usize) -> Vec<VertRun> {
    let mut runs = Vec::new();
    let mut r = 0usize;
    while r < content_rows {
        if !grid.cell_char(r, col).is_some_and(is_vert_border_char) {
            r += 1;
            continue;
        }
        let start = r;
        while r + 1 < content_rows && grid.cell_char(r + 1, col).is_some_and(is_vert_border_char) {
            r += 1;
        }
        runs.push(VertRun { col, r_start: start, r_end: r });
        r += 1;
    }
    runs
}

fn is_valid_vert_run(
    run: VertRun,
    total_hits: usize,
    content_rows: usize,
    full_horiz: &[usize],
) -> bool {
    let len = run.r_end - run.r_start + 1;
    if len < 2 {
        return false;
    }
    let threshold = content_rows.div_ceil(2).max(3).min(content_rows);
    if total_hits >= threshold {
        return true;
    }
    let top_ok = run.r_start == 0 || full_horiz.contains(&run.r_start);
    let bot_ok = run.r_end + 1 == content_rows || full_horiz.contains(&run.r_end);
    top_ok && bot_ok
}

fn collect_valid_vert_runs(grid: &ScreenGrid, content_rows: usize, full_horiz: &[usize]) -> Vec<VertRun> {
    let mut vert_runs = Vec::new();
    for col in 1..(grid.cols - 1) {
        let runs = collect_col_runs(grid, col, content_rows);
        let hits: usize = runs.iter().map(|x| x.r_end - x.r_start + 1).sum();
        vert_runs.extend(runs.into_iter().filter(|&r| is_valid_vert_run(r, hits, content_rows, full_horiz)));
    }
    vert_runs
}

pub fn detect_tmux_layout(grid: &ScreenGrid) -> Option<TmuxLayout> {
    if grid.cols < 3 || grid.rows < 2 {
        return None;
    }
    let content_rows = detect_content_rows(grid);
    let full_horiz: Vec<usize> = (1..content_rows.saturating_sub(1))
        .filter(|&r| is_horiz_segment(grid, r, 0, grid.cols - 1))
        .collect();
    let vert_runs = collect_valid_vert_runs(grid, content_rows, &full_horiz);
    if vert_runs.is_empty() && full_horiz.is_empty() {
        return None;
    }
    Some(TmuxLayout { cols: grid.cols, content_rows, vert_runs })
}

impl TmuxLayout {
    fn has_vert_at(&self, col: usize, row: usize) -> bool {
        self.vert_runs
            .iter()
            .any(|r| r.col == col && row >= r.r_start && row <= r.r_end)
    }

    fn horizontal_span_bounds(&self, col: usize, row: usize) -> Option<(usize, usize)> {
        if self.has_vert_at(col, row) {
            return None;
        }
        let left_col = (0..col)
            .rev()
            .find(|&c| self.has_vert_at(c, row))
            .map_or(0, |c| c + 1);
        let right_col = ((col + 1)..self.cols)
            .find(|&c| self.has_vert_at(c, row))
            .map_or(self.cols - 1, |c| c - 1);
        (left_col <= right_col).then_some((left_col, right_col))
    }

    fn pane_bounds_at(&self, grid: &ScreenGrid, col: usize, row: usize) -> Option<PaneBounds> {
        if row >= self.content_rows || col >= self.cols { return None; }
        let (left_col, right_col) = self.horizontal_span_bounds(col, row)?;
        if is_horiz_segment(grid, row, left_col, right_col) { return None; }
        let top_row = (1..row).rev().find(|&r| is_horiz_segment(grid, r, left_col, right_col)).map_or(0, |r| r + 1);
        let bottom_row = ((row + 1)..self.content_rows.saturating_sub(1))
            .find(|&r| is_horiz_segment(grid, r, left_col, right_col))
            .map_or(self.content_rows - 1, |r| r - 1);
        (top_row <= bottom_row).then_some(PaneBounds { left_col, right_col, top_row, bottom_row })
    }
}

pub fn find_pane_bounds(grid: &ScreenGrid, col: usize, row: usize) -> Option<PaneBounds> {
    let Some(layout) = detect_tmux_layout(grid) else {
        return None;
    };
    layout.pane_bounds_at(grid, col, row)
}

pub fn normalized_range(anchor: CellPos, cursor: CellPos) -> (CellPos, CellPos) {
    if (anchor.row, anchor.col) <= (cursor.row, cursor.col) {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    }
}

fn clamp_cell_to_bounds(pos: CellPos, bounds: &PaneBounds) -> CellPos {
    CellPos {
        row: pos.row.clamp(bounds.top_row, bounds.bottom_row),
        col: pos.col.clamp(bounds.left_col, bounds.right_col),
    }
}

fn row_col_span(bounds: &PaneBounds, start: CellPos, end: CellPos, row: usize) -> (usize, usize) {
    let c0 = if row == start.row { start.col } else { bounds.left_col };
    let c1 = if row == end.row { end.col } else { bounds.right_col };
    (c0.min(c1), c1)
}

pub fn extract_pane_selection_text(
    grid: &ScreenGrid, bounds: &PaneBounds, anchor: CellPos, cursor: CellPos,
) -> String {
    let (s, e) = normalized_range(clamp_cell_to_bounds(anchor, bounds), clamp_cell_to_bounds(cursor, bounds));
    (s.row..=e.row)
        .map(|row| {
            let (c0, c1) = row_col_span(bounds, s, e, row);
            slice_line_cols(grid.line_at(row), c0, c1).trim_end_matches(' ').to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punct,
}

fn classify_pane_char(ch: Option<char>) -> CharClass {
    match ch {
        None | Some(' ') => CharClass::Space,
        Some(c)
            if c.is_alphanumeric()
                || matches!(c, '_' | '-' | '.' | '/' | ':' | '~' | '?' | '&' | '=' | '%' | '+' | '#' | '@') =>
        {
            CharClass::Word
        }
        _ => CharClass::Punct,
    }
}

fn resolve_cell_anchor_col(grid: &ScreenGrid, row: usize, col: usize, min_col: usize) -> usize {
    if matches!(grid.cells.get(row).and_then(|r| r.get(col)), Some(Cell::WideTail)) && col > min_col {
        col - 1
    } else {
        col
    }
}

fn word_scan_left(grid: &ScreenGrid, bounds: &PaneBounds, r: usize, c: usize, cls: CharClass) -> usize {
    let mut start = c;
    while start > bounds.left_col {
        let prev = resolve_cell_anchor_col(grid, r, start - 1, bounds.left_col);
        if classify_pane_char(grid.cell_char(r, prev)) != cls {
            break;
        }
        start = prev;
    }
    start
}

fn word_scan_right(grid: &ScreenGrid, bounds: &PaneBounds, r: usize, c: usize, cls: CharClass) -> usize {
    let mut end = c;
    while end < bounds.right_col {
        let is_wide = matches!(grid.cells.get(r).and_then(|x| x.get(end + 1)), Some(Cell::WideTail));
        let step = if is_wide { 2 } else { 1 };
        if end + step > bounds.right_col || classify_pane_char(grid.cell_char(r, end + step)) != cls {
            break;
        }
        end += step;
    }
    if end < bounds.right_col
        && matches!(grid.cells.get(r).and_then(|x| x.get(end + 1)), Some(Cell::WideTail))
    {
        end += 1;
    }
    end
}

pub fn compute_word_bounds_in_pane(
    grid: &ScreenGrid,
    bounds: &PaneBounds,
    col: usize,
    row: usize,
) -> (usize, usize) {
    let r = row.clamp(bounds.top_row, bounds.bottom_row);
    let c = resolve_cell_anchor_col(grid, r, col.clamp(bounds.left_col, bounds.right_col), bounds.left_col);
    let cls = classify_pane_char(grid.cell_char(r, c));
    (word_scan_left(grid, bounds, r, c, cls), word_scan_right(grid, bounds, r, c, cls))
}

fn split_line_at_border_col(line: &str, border_col: usize) -> Option<(String, String)> {
    let mut col = 0usize;
    for (idx, ch) in line.char_indices() {
        let w = char_cell_width(ch);
        if col == border_col && is_pure_vert_char(ch) {
            let lhs = line[..idx].trim_end_matches(' ').to_owned();
            let rhs = line[idx + ch.len_utf8()..].trim_end_matches(' ').to_owned();
            return Some((lhs, rhs));
        }
        col += w;
        if col > border_col {
            break;
        }
    }
    None
}

fn split_first_vert_char(line: &str) -> Option<(String, String)> {
    for (idx, ch) in line.char_indices() {
        if is_pure_vert_char(ch) {
            let lhs = line[..idx].trim_end_matches(' ').to_owned();
            let rhs = line[idx + ch.len_utf8()..].trim_end_matches(' ').to_owned();
            return Some((lhs, rhs));
        }
    }
    None
}

fn find_matching_border_col(lines: &[&str], vert_cols: &[usize]) -> Option<usize> {
    for &c in vert_cols {
        if lines.iter().skip(1).any(|l| split_line_at_border_col(l, c).is_some()) {
            return Some(c);
        }
    }
    None
}

fn line_matches_screen_border(grid: &ScreenGrid, line: &str, vert_cols: &[usize]) -> bool {
    let Some((lhs, _)) = split_first_vert_char(line) else {
        return false;
    };
    let lhs_w: usize = lhs.chars().map(char_cell_width).sum();
    vert_cols.iter().any(|&vc| {
        lhs_w <= vc
            && (0..grid.rows).any(|r| {
                let seg = slice_line_cols(grid.line_at(r), vc.saturating_sub(lhs_w), vc.saturating_sub(1));
                seg.trim_end_matches(' ') == lhs
            })
    })
}

fn sorted_vert_cols(layout: &TmuxLayout) -> Vec<usize> {
    let mut cols: Vec<usize> = layout.vert_runs.iter().map(|r| r.col).collect();
    cols.sort_unstable();
    cols.dedup();
    cols
}

fn slice_lines_against_border(lines: &[&str], border_col: Option<usize>) -> Option<String> {
    let (first_lhs, first_rhs) = split_first_vert_char(lines[0])?;
    let take_left = !first_lhs.trim().is_empty() || first_rhs.trim().is_empty();
    let mut out = vec![if take_left { first_lhs } else { first_rhs }];
    for line in lines.iter().skip(1) {
        let pair = border_col
            .and_then(|c| split_line_at_border_col(line, c))
            .or_else(|| split_first_vert_char(line));
        match pair {
            Some((lhs, rhs)) => out.push(if take_left { lhs } else { rhs }),
            None => out.push((*line).trim_end_matches(' ').to_owned()),
        }
    }
    Some(out.join("\n"))
}

pub fn slice_native_selection_against_screen(screen: &str, cols: usize, rows: usize, selected: &str) -> String {
    let grid = ScreenGrid::from_text(screen, cols, rows);
    let Some(layout) = detect_tmux_layout(&grid) else { return selected.to_owned(); };
    let vert_cols = sorted_vert_cols(&layout);
    if vert_cols.is_empty() || !selected.chars().any(is_pure_vert_char) { return selected.to_owned(); }
    let lines: Vec<&str> = selected.split('\n').collect();
    let border_col = find_matching_border_col(&lines, &vert_cols);
    let valid_single = lines.len() == 1 && line_matches_screen_border(&grid, lines[0], &vert_cols);
    if !valid_single && (lines.len() == 1 || border_col.is_none()) { return selected.to_owned(); }
    slice_lines_against_border(&lines, border_col).unwrap_or_else(|| selected.to_owned())
}

fn build_highlight_rects(
    bounds: &PaneBounds,
    anchor: CellPos,
    cursor: CellPos,
    m: CellMetrics,
) -> Vec<HighlightRect> {
    let (s, e) = normalized_range(clamp_cell_to_bounds(anchor, bounds), clamp_cell_to_bounds(cursor, bounds));
    (s.row..=e.row)
        .map(|row| {
            let (c0, c1) = row_col_span(bounds, s, e, row);
            HighlightRect {
                x: m.pad_left + (c0 as f64) * m.cw,
                y: m.pad_top + (row as f64) * m.ch,
                w: ((c1 - c0 + 1) as f64) * m.cw,
                h: m.ch,
            }
        })
        .collect()
}

fn take_c_string(ptr: *mut std::ffi::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    unsafe { g_free(ptr as *mut c_void) };
    Some(text)
}

fn read_vte_padding(vte: *mut GtkWidget) -> GtkBorder {
    let mut pad = GtkBorder::default();
    unsafe {
        let ctx = gtk_widget_get_style_context(vte);
        if !ctx.is_null() {
            gtk_style_context_get_padding(ctx, GTK_STATE_FLAG_NORMAL, &mut pad);
        }
    }
    pad
}

fn read_cell_metrics(vte: *mut GtkWidget) -> Option<CellMetrics> {
    if vte.is_null() { return None; }
    unsafe {
        let (cols, rows) = (vte_terminal_get_column_count(vte), vte_terminal_get_row_count(vte));
        let (cw, ch) = (vte_terminal_get_char_width(vte), vte_terminal_get_char_height(vte));
        if cols <= 0 || rows <= 0 || cw <= 0 || ch <= 0 { return None; }
        let pad = read_vte_padding(vte);
        Some(CellMetrics {
            cw: cw as f64, ch: ch as f64,
            pad_left: f64::from(pad.left.max(0)), pad_top: f64::from(pad.top.max(0)),
            cols: cols as usize, rows: rows as usize,
        })
    }
}

fn read_vte_screen_grid(vte: *mut GtkWidget, m: CellMetrics) -> ScreenGrid {
    let raw = unsafe {
        take_c_string(vte_terminal_get_text_format(vte, VTE_FORMAT_TEXT))
    }
    .unwrap_or_default();
    ScreenGrid::from_text(&raw, m.cols, m.rows)
}

fn coords_to_cell(x: f64, y: f64, m: CellMetrics) -> CellPos {
    let col = (((x - m.pad_left) / m.cw).floor().max(0.0) as usize).min(m.cols.saturating_sub(1));
    let row = (((y - m.pad_top) / m.ch).floor().max(0.0) as usize).min(m.rows.saturating_sub(1));
    CellPos { row, col }
}

fn write_primary_clipboard(value: &str) {
    let Ok(c_text) = CString::new(value) else {
        return;
    };
    unsafe {
        let clip = gtk_clipboard_get(GDK_SELECTION_PRIMARY);
        if !clip.is_null() {
            gtk_clipboard_set_text(clip, c_text.as_ptr(), value.len() as c_int);
        }
    }
}

pub fn has_selection(vte: *mut GtkWidget) -> bool {
    if vte.is_null() {
        return false;
    }
    VTE_STATES.with(|map| map.borrow().get(&(vte as usize)).is_some_and(|s| s.active))
}

pub fn clear_selection(vte: *mut GtkWidget) {
    if vte.is_null() {
        return;
    }
    let changed = VTE_STATES.with(|map| map.borrow_mut().remove(&(vte as usize)).is_some());
    if changed {
        unsafe { gtk_widget_queue_draw(vte) };
    }
}

pub fn read_selection(vte: *mut GtkWidget) -> Option<String> {
    let state = VTE_STATES.with(|map| {
        map.borrow()
            .get(&(vte as usize))
            .filter(|s| s.active)
            .cloned()
    })?;
    let m = read_cell_metrics(vte)?;
    let grid = read_vte_screen_grid(vte, m);
    let text = extract_pane_selection_text(&grid, &state.bounds, state.anchor, state.cursor);
    (!text.is_empty()).then_some(text)
}

pub fn slice_native_selection_if_tmux(vte: *mut GtkWidget, selected: &str) -> String {
    let Some(m) = read_cell_metrics(vte) else {
        return selected.to_owned();
    };
    let raw = unsafe { take_c_string(vte_terminal_get_text_format(vte, VTE_FORMAT_TEXT)) }
        .unwrap_or_default();
    slice_native_selection_against_screen(&raw, m.cols, m.rows, selected)
}

fn selection_endpoints(ev_type: c_int, cell: CellPos, b: PaneBounds, grid: &ScreenGrid) -> (CellPos, CellPos, bool, bool) {
    match ev_type {
        GDK_2BUTTON_PRESS => {
            let (s, e) = compute_word_bounds_in_pane(grid, &b, cell.col, cell.row);
            (CellPos { row: cell.row, col: s }, CellPos { row: cell.row, col: e }, true, false)
        }
        GDK_3BUTTON_PRESS => (
            CellPos { row: cell.row, col: b.left_col }, CellPos { row: cell.row, col: b.right_col }, true, false,
        ),
        _ => (cell, cell, false, true),
    }
}

fn start_pane_selection(vte: *mut GtkWidget, ev_type: c_int, cell: CellPos, x: f64, y: f64, bounds: PaneBounds, grid: &ScreenGrid) {
    let (anchor, cursor, active, dragging) = selection_endpoints(ev_type, cell, bounds, grid);
    VTE_STATES.with(|map| {
        map.borrow_mut().insert(vte as usize, VteTmuxState { bounds, anchor, cursor, active, dragging, start_px: (x, y) });
    });
}

fn extract_press_event(event: *const GdkEvent) -> Option<(c_int, c_uint, c_uint, f64, f64)> {
    let (mut button, mut state, mut x, mut y) = (0u32, 0u32, 0.0, 0.0);
    let ev_type = unsafe { gdk_event_get_event_type(event) };
    if !matches!(ev_type, GDK_BUTTON_PRESS | GDK_2BUTTON_PRESS | GDK_3BUTTON_PRESS) {
        return None;
    }
    unsafe {
        if gdk_event_get_button(event, &mut button) == 0 || gdk_event_get_coords(event, &mut x, &mut y) == 0 {
            return None;
        }
        gdk_event_get_state(event, &mut state);
    }
    Some((ev_type, button, state, x, y))
}

fn resolve_pane_press(vte: *mut GtkWidget, x: f64, y: f64) -> Option<(CellPos, PaneBounds, ScreenGrid)> {
    let Some(m) = read_cell_metrics(vte) else {
        clear_selection(vte);
        return None;
    };
    let grid = read_vte_screen_grid(vte, m);
    let cell = coords_to_cell(x, y, m);
    let Some(bounds) = find_pane_bounds(&grid, cell.col, cell.row) else {
        clear_selection(vte);
        return None;
    };
    Some((cell, bounds, grid))
}

fn apply_pane_press(vte: *mut GtkWidget, ev_type: c_int, x: f64, y: f64) -> c_int {
    let Some((cell, bounds, grid)) = resolve_pane_press(vte, x, y) else {
        return 0;
    };
    unsafe {
        gtk_widget_grab_focus(vte);
        vte_terminal_unselect_all(vte);
    }
    start_pane_selection(vte, ev_type, cell, x, y, bounds, &grid);
    unsafe { gtk_widget_queue_draw(vte) };
    1
}

extern "C" fn on_vte_tmux_button_press(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    if vte.is_null() || event.is_null() {
        return 0;
    }
    let Some((ev_type, button, state, x, y)) = extract_press_event(event) else {
        return 0;
    };
    if button != 1 || (state & (GDK_CONTROL_MASK | GDK_SHIFT_MASK)) != 0 {
        return 0;
    }
    apply_pane_press(vte, ev_type, x, y)
}

fn update_drag_motion(vte: *mut GtkWidget, x: f64, y: f64, cell: CellPos) -> bool {
    VTE_STATES.with(|map| {
        let mut guard = map.borrow_mut();
        let Some(st) = guard.get_mut(&(vte as usize)).filter(|s| s.dragging) else {
            return false;
        };
        let clamped = clamp_cell_to_bounds(cell, &st.bounds);
        if clamped != st.anchor || (x - st.start_px.0).hypot(y - st.start_px.1) >= 3.0 {
            st.active = true;
        }
        st.cursor = clamped;
        true
    })
}

extern "C" fn on_vte_tmux_motion(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    let (mut x, mut y) = (0.0, 0.0);
    if vte.is_null() || event.is_null() || unsafe { gdk_event_get_coords(event, &mut x, &mut y) } == 0 {
        return 0;
    }
    let Some(m) = read_cell_metrics(vte) else {
        return 0;
    };
    if update_drag_motion(vte, x, y, coords_to_cell(x, y, m)) {
        unsafe { gtk_widget_queue_draw(vte) };
        return 1;
    }
    0
}

fn finish_drag_release(vte: *mut GtkWidget, event: *const GdkEvent) -> Option<bool> {
    let (mut x, mut y) = (0.0, 0.0);
    let has_coords = unsafe { gdk_event_get_coords(event, &mut x, &mut y) } != 0;
    let m = read_cell_metrics(vte);
    VTE_STATES.with(|map| {
        let mut guard = map.borrow_mut();
        let st = guard.get_mut(&(vte as usize))?;
        if st.dragging {
            if let (true, Some(metrics)) = (has_coords, m) {
                st.cursor = clamp_cell_to_bounds(coords_to_cell(x, y, metrics), &st.bounds);
            }
            st.dragging = false;
        }
        Some(st.active)
    })
}

fn finalize_release_state(vte: *mut GtkWidget, active: bool) {
    if active {
        if let Some(text) = read_selection(vte) {
            write_primary_clipboard(&text);
        }
    } else {
        clear_selection(vte);
    }
    unsafe { gtk_widget_queue_draw(vte) };
}

extern "C" fn on_vte_tmux_button_release(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    let mut button: c_uint = 0;
    if vte.is_null() || event.is_null() || unsafe { gdk_event_get_button(event, &mut button) } == 0 || button != 1 {
        return 0;
    }
    let Some(active) = finish_drag_release(vte, event) else {
        return 0;
    };
    finalize_release_state(vte, active);
    1
}

fn zinc_highlight_rgba() -> (f64, f64, f64, f64) {
    match current_theme_mode() {
        ThemeMode::Dark => (0.631, 0.631, 0.667, 0.34),
        ThemeMode::Light => (0.322, 0.322, 0.357, 0.25),
    }
}

fn paint_highlight_rects(cr: *mut cairo_t, rects: &[HighlightRect]) {
    let (r, g, b, a) = zinc_highlight_rgba();
    unsafe {
        cairo_save(cr);
        cairo_set_source_rgba(cr, r, g, b, a);
        for rect in rects {
            cairo_rectangle(cr, rect.x, rect.y, rect.w, rect.h);
        }
        cairo_fill(cr);
        cairo_restore(cr);
    }
}

extern "C" fn on_vte_tmux_draw(vte: *mut GtkWidget, cr: *mut cairo_t, _data: *mut c_void) -> c_int {
    if vte.is_null() || cr.is_null() { return 0; }
    let state = VTE_STATES.with(|map| map.borrow().get(&(vte as usize)).filter(|s| s.active).cloned());
    let (Some(st), Some(m)) = (state, read_cell_metrics(vte)) else { return 0; };
    let rects = build_highlight_rects(&st.bounds, st.anchor, st.cursor, m);
    if !rects.is_empty() { paint_highlight_rects(cr, &rects); }
    0
}

fn should_preserve_selection(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if matches!(keyval, 0xffe1..=0xffee | 0xfe03 | 0xff7f | 0xff14) {
        return true;
    }
    (state & GDK_CONTROL_MASK) != 0
        && (crate::paste_route::is_c_key(keyval, keycode) || keyval == 0xff63)
}

extern "C" fn on_vte_tmux_key_press(vte: *mut GtkWidget, event: *const GdkEvent, _data: *mut c_void) -> c_int {
    if vte.is_null() || event.is_null() || !has_selection(vte) { return 0; }
    let (mut keyval, mut state, mut keycode) = (0u32, 0u32, 0u16);
    unsafe {
        if gdk_event_get_keyval(event, &mut keyval) == 0 { return 0; }
        gdk_event_get_state(event, &mut state);
        gdk_event_get_keycode(event, &mut keycode);
    }
    if !should_preserve_selection(keyval, state, keycode) { clear_selection(vte); }
    0
}

extern "C" fn on_vte_tmux_destroy(vte: *mut GtkWidget, _data: *mut c_void) {
    if !vte.is_null() {
        VTE_STATES.with(|map| { map.borrow_mut().remove(&(vte as usize)); });
    }
}

pub fn attach_vte_handlers(vte: *mut GtkWidget) {
    if vte.is_null() { return; }
    let signals: [(&CStr, *const c_void, c_int); 6] = [
        (c"button-press-event", on_vte_tmux_button_press as *const c_void, 0),
        (c"motion-notify-event", on_vte_tmux_motion as *const c_void, 0),
        (c"button-release-event", on_vte_tmux_button_release as *const c_void, 0),
        (c"draw", on_vte_tmux_draw as *const c_void, G_CONNECT_AFTER),
        (c"key-press-event", on_vte_tmux_key_press as *const c_void, 0),
        (c"destroy", on_vte_tmux_destroy as *const c_void, 0),
    ];
    unsafe {
        gtk_widget_add_events(vte, GDK_BUTTON_PRESS_MASK | GDK_BUTTON_RELEASE_MASK | GDK_BUTTON1_MOTION_MASK);
        for (sig, cb, flags) in signals { g_signal_connect_data(vte, sig.as_ptr(), cb, ptr::null_mut(), ptr::null(), flags); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use crate::app_state::APP;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::{context_menu, session::SessionCategory, session_ops, shortcuts, tabs::Tab};

    fn check_vert_border(border: char) {
        let screen = [
            format!("Создал Михаилу 42 слота     {border} right line 1   "),
            format!("12 ноября: 14:00-19:00      {border} right line 2   "),
            format!("https://akhcheck.ru/def     {border} right line 3   "),
            format!("Worked for 4m 2s            {border} right line 4   "),
            "[0] 0:codex*                \"gpu01\" 01:22".to_owned(),
        ].join("\n");
        let grid = ScreenGrid::from_text(&screen, 48, 5);
        assert_eq!(find_pane_bounds(&grid, 28, 1), None);
        assert_eq!(find_pane_bounds(&grid, 5, 4), None);
        let left = find_pane_bounds(&grid, 4, 0).expect("left pane");
        assert_eq!(left, PaneBounds { left_col: 0, right_col: 27, top_row: 0, bottom_row: 3 });
        let right = find_pane_bounds(&grid, 35, 2).expect("right pane");
        assert_eq!(right, PaneBounds { left_col: 29, right_col: 47, top_row: 0, bottom_row: 3 });
        let extracted = extract_pane_selection_text(&grid, &left, CellPos { row: 0, col: 0 }, CellPos { row: 2, col: 44 });
        assert_eq!(extracted, "Создал Михаилу 42 слота\n12 ноября: 14:00-19:00\nhttps://akhcheck.ru/def");
    }

    #[test]
    fn vertical_split_detects_border_and_clamps_trailing_padding() {
        for border in ['│', '┃', '║'] {
            check_vert_border(border);
        }
    }

    #[test]
    fn horizontal_split_and_bottom_status_bar_exclusion() {
        for sep in ["──────────────────────────────", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"] {
            let screen = [
                "top pane line 1", "top pane line 2", sep,
                "bottom pane line 1", "bottom pane line 2", "[dev] 0:bash* 1:vim-  \"box\" 12:00",
            ].join("\n");
            let grid = ScreenGrid::from_text(&screen, 30, 6);
            assert_eq!(find_pane_bounds(&grid, 5, 2), None);
            assert_eq!(find_pane_bounds(&grid, 5, 5), None);
            assert_eq!(find_pane_bounds(&grid, 2, 0), Some(PaneBounds { left_col: 0, right_col: 29, top_row: 0, bottom_row: 1 }));
            assert_eq!(find_pane_bounds(&grid, 2, 3), Some(PaneBounds { left_col: 0, right_col: 29, top_row: 3, bottom_row: 4 }));
        }
    }

    #[test]
    fn three_pane_t_junction_right_split() {
        let right_split = "left 1    │ top-right 1\nleft 2    │ top-right 2\nleft 3    ├────────────\nleft 4    │ bot-right 1\nleft 5    │ bot-right 2";
        let g1 = ScreenGrid::from_text(right_split, 23, 5);
        assert_eq!(find_pane_bounds(&g1, 2, 2), Some(PaneBounds { left_col: 0, right_col: 9, top_row: 0, bottom_row: 4 }));
        assert_eq!(find_pane_bounds(&g1, 14, 1), Some(PaneBounds { left_col: 11, right_col: 22, top_row: 0, bottom_row: 1 }));
        assert_eq!(find_pane_bounds(&g1, 14, 4), Some(PaneBounds { left_col: 11, right_col: 22, top_row: 3, bottom_row: 4 }));
    }

    #[test]
    fn three_pane_t_junction_bottom_split() {
        let bottom_split = "top full line 1\ntop full line 2\ntop full line 3\n──────────┬────────────\nbot-left  │ bot-right\nbot-left2 │ bot-right2";
        let g2 = ScreenGrid::from_text(bottom_split, 23, 6);
        assert_eq!(find_pane_bounds(&g2, 15, 1), Some(PaneBounds { left_col: 0, right_col: 22, top_row: 0, bottom_row: 2 }));
        assert_eq!(find_pane_bounds(&g2, 2, 4), Some(PaneBounds { left_col: 0, right_col: 9, top_row: 4, bottom_row: 5 }));
        assert_eq!(find_pane_bounds(&g2, 14, 5), Some(PaneBounds { left_col: 11, right_col: 22, top_row: 4, bottom_row: 5 }));
    }

    #[test]
    fn unicode_cell_widths_match_vte_grid() {
        assert_eq!(char_cell_width('А'), 1);
        assert_eq!(char_cell_width('│'), 1);
        assert_eq!(char_cell_width('中'), 2);
        assert_eq!(char_cell_width('🚀'), 2);
        assert_eq!(char_cell_width('\u{0301}'), 0);
    }

    #[test]
    fn cyrillic_url_wide_chars_and_word_bounds() {
        let line = "e\u{0301} 中文 🚀 https://akhcheck.ru/defenses  │ right";
        let raw = format!("{line}\nСоздал Михаилу                          │ right\nfoo bar                                 │ right");
        let grid = ScreenGrid::from_text(&raw, 48, 3);
        let bounds = find_pane_bounds(&grid, 0, 0).expect("left pane");
        assert_eq!(bounds.right_col, 39);
        let (ws, we) = compute_word_bounds_in_pane(&grid, &bounds, 12, 0);
        assert_eq!(slice_line_cols(line, ws, we), "https://akhcheck.ru/defenses");
        let (cs, ce) = compute_word_bounds_in_pane(&grid, &bounds, 3, 0);
        assert_eq!(slice_line_cols(line, cs, ce), "中文");
        let (rs, re) = compute_word_bounds_in_pane(&grid, &bounds, 8, 1);
        assert_eq!(slice_line_cols(grid.line_at(1), rs, re), "Михаилу");
    }

    #[test]
    fn plain_screen_bypasses_tmux_copy() {
        let plain = "╭──────────╮\n│ prompt   │\n╰──────────╯\nnormal line\n";
        let plain_grid = ScreenGrid::from_text(plain, 40, 12);
        assert_eq!(find_pane_bounds(&plain_grid, 3, 1), None);
        assert_eq!(slice_native_selection_against_screen(plain, 40, 12, "│ prompt   │"), "│ prompt   │");
    }

    #[test]
    fn fallback_native_selection_slicer_trims_right_pane() {
        let tmux_screen = [
            "alpha line 1   │ beta line 1",
            "alpha line 2   │ beta line 2",
            "alpha line 3   │ beta line 3",
            "alpha line 4   │ beta line 4",
        ].join("\n");
        let native = "line 1   │ beta line 1\nalpha line 2   │ beta line 2";
        assert_eq!(slice_native_selection_against_screen(&tmux_screen, 28, 4, native), "line 1\nalpha line 2");
    }

    #[repr(C)] struct TestButtonEvent { ev_type: c_int, window: *mut c_void, send_event: i8, time: u32, x: f64, y: f64, axes: *mut f64, state: c_uint, button: c_uint, device: *mut c_void, x_root: f64, y_root: f64 }
    #[repr(C)] struct TestMotionEvent { ev_type: c_int, window: *mut c_void, send_event: i8, time: u32, x: f64, y: f64, axes: *mut f64, state: c_uint, is_hint: i16, device: *mut c_void, x_root: f64, y_root: f64 }
    #[repr(C)] struct TestKeyEvent { ev_type: c_int, window: *mut c_void, send_event: i8, time: u32, state: c_uint, keyval: c_uint, length: c_int, string: *mut std::ffi::c_char, hardware_keycode: u16, group: u8, flags: c_uint }

    fn read_gtk_clipboard_text(atom: GdkAtom) -> Option<String> {
        unsafe {
            let clip = gtk_clipboard_get(atom);
            let raw = gtk_clipboard_wait_for_text(clip);
            if raw.is_null() {
                return None;
            }
            let text = CStr::from_ptr(raw).to_string_lossy().into_owned();
            g_free(raw as *mut c_void);
            Some(text)
        }
    }

    fn send_vte_button_ev(vte: *mut GtkWidget, ev_type: c_int, col: usize, row: usize, state: c_uint, m: CellMetrics) -> c_int {
        unsafe {
            let ev = gdk_event_new(ev_type);
            let btn = &mut *(ev as *mut TestButtonEvent);
            btn.x = m.pad_left + (col as f64 + 0.5) * m.cw;
            btn.y = m.pad_top + (row as f64 + 0.5) * m.ch;
            btn.button = 1;
            btn.state = state;
            let res = on_vte_tmux_button_press(vte, ev, ptr::null_mut());
            gdk_event_free(ev);
            res
        }
    }

    fn send_vte_motion_ev(vte: *mut GtkWidget, col: usize, row: usize, m: CellMetrics) -> c_int {
        unsafe {
            let ev = gdk_event_new(GDK_MOTION_NOTIFY);
            let mot = &mut *(ev as *mut TestMotionEvent);
            mot.x = m.pad_left + (col as f64 + 0.5) * m.cw;
            mot.y = m.pad_top + (row as f64 + 0.5) * m.ch;
            mot.state = 1 << 8;
            let res = on_vte_tmux_motion(vte, ev, ptr::null_mut());
            gdk_event_free(ev);
            res
        }
    }

    fn send_vte_release_ev(vte: *mut GtkWidget, col: usize, row: usize, m: CellMetrics) -> c_int {
        unsafe {
            let ev = gdk_event_new(GDK_BUTTON_RELEASE);
            let btn = &mut *(ev as *mut TestButtonEvent);
            btn.x = m.pad_left + (col as f64 + 0.5) * m.cw;
            btn.y = m.pad_top + (row as f64 + 0.5) * m.ch;
            btn.button = 1;
            let res = on_vte_tmux_button_release(vte, ev, ptr::null_mut());
            gdk_event_free(ev);
            res
        }
    }

    fn send_window_ctrl_key(win: *mut GtkWidget, keyval: c_uint, state: c_uint, keycode: u16) -> c_int {
        unsafe {
            let ev = gdk_event_new(GDK_KEY_PRESS);
            let kev = &mut *(ev as *mut TestKeyEvent);
            kev.keyval = keyval;
            kev.state = state;
            kev.hardware_keycode = keycode;
            let res = (shortcuts::window_key_press_cb())(win, ev, ptr::null_mut());
            gdk_event_free(ev);
            res
        }
    }

    fn send_vte_key_ev(vte: *mut GtkWidget, keyval: c_uint, state: c_uint, keycode: u16) -> c_int {
        unsafe {
            let ev = gdk_event_new(GDK_KEY_PRESS);
            let kev = &mut *(ev as *mut TestKeyEvent);
            kev.keyval = keyval;
            kev.state = state;
            kev.hardware_keycode = keycode;
            let res = on_vte_tmux_key_press(vte, ev, ptr::null_mut());
            gdk_event_free(ev);
            res
        }
    }

    fn verify_broadway_drag_and_ctrl_c(win: *mut GtkWidget, vte: *mut GtkWidget, m: CellMetrics) {
        assert_eq!(send_vte_button_ev(vte, GDK_BUTTON_PRESS, 2, 0, GDK_CONTROL_MASK, m), 0);
        assert_eq!(send_vte_button_ev(vte, GDK_BUTTON_PRESS, 2, 0, GDK_SHIFT_MASK, m), 0);
        assert_eq!(send_vte_button_ev(vte, GDK_BUTTON_PRESS, 0, 0, 0, m), 1);
        assert_eq!(send_vte_motion_ev(vte, 45, 1, m), 1);
        assert_eq!(send_vte_release_ev(vte, 45, 1, m), 1);
        assert_eq!(unsafe { vte_terminal_get_has_selection(vte) }, 0);
        let sel = context_menu::read_vte_selection(vte).expect("pane selection");
        assert_eq!(sel, "Создал Михаилу\nhttps://akhcheck.ru/defenses");
        pump_gtk_events();
        assert_eq!(read_gtk_clipboard_text(GDK_SELECTION_PRIMARY).as_deref(), Some(sel.as_str()));
        assert_eq!(send_window_ctrl_key(win, b'c' as u32, GDK_CONTROL_MASK, 54), 1);
        pump_gtk_events();
        assert_eq!(read_gtk_clipboard_text(GDK_SELECTION_CLIPBOARD).as_deref(), Some(sel.as_str()));
        assert!(!context_menu::has_vte_selection(vte));
        assert_eq!(send_window_ctrl_key(win, b'c' as u32, GDK_CONTROL_MASK, 54), 0);
    }

    fn verify_broadway_word_and_line_clicks(win: *mut GtkWidget, vte: *mut GtkWidget, m: CellMetrics) {
        assert_eq!(send_vte_button_ev(vte, GDK_2BUTTON_PRESS, 9, 0, 0, m), 1);
        assert_eq!(send_vte_release_ev(vte, 9, 0, m), 1);
        assert_eq!(context_menu::read_vte_selection(vte).as_deref(), Some("Михаилу"));
        assert_eq!(send_vte_button_ev(vte, GDK_3BUTTON_PRESS, 3, 1, 0, m), 1);
        assert_eq!(send_vte_release_ev(vte, 3, 1, m), 1);
        assert_eq!(context_menu::read_vte_selection(vte).as_deref(), Some("https://akhcheck.ru/defenses"));
        assert_eq!(send_window_ctrl_key(win, b'C' as u32, GDK_CONTROL_MASK | GDK_SHIFT_MASK, 54), 1);
        assert!(context_menu::has_vte_selection(vte));
    }

    fn verify_broadway_clear_paths(vte: *mut GtkWidget, m: CellMetrics) {
        assert_eq!(send_vte_key_ev(vte, 0xffe3, 0, 37), 0);
        assert!(context_menu::has_vte_selection(vte));
        assert_eq!(send_vte_key_ev(vte, b'a' as u32, 0, 38), 0);
        assert!(!context_menu::has_vte_selection(vte));
        assert_eq!(send_vte_button_ev(vte, GDK_3BUTTON_PRESS, 3, 0, 0, m), 1);
        assert!(context_menu::has_vte_selection(vte));
        assert_eq!(send_vte_button_ev(vte, GDK_BUTTON_PRESS, 3, 0, 0, m), 1);
        assert_eq!(send_vte_release_ev(vte, 3, 0, m), 1);
        assert!(!context_menu::has_vte_selection(vte));
    }

    fn feed_tmux_screen(vte: *mut GtkWidget, m: CellMetrics) {
        let mut feed = String::from(
            "\x1b[2J\x1b[H\
             Создал Михаилу                │ RIGHT_PANE_1\r\n\
             https://akhcheck.ru/defenses  │ RIGHT_PANE_2\r\n",
        );
        for r in 2..m.rows.saturating_sub(1) {
            feed.push_str(&format!("строка {r:<23}│ RIGHT_PANE_{r}\r\n"));
        }
        feed.push_str("[0] 0:codex*                 \"gpu01\" 01:22");
        unsafe { vte_terminal_feed(vte, feed.as_ptr() as *const _, feed.len() as isize) };
        std::thread::sleep(Duration::from_millis(40));
        pump_gtk_events();
    }

    fn verify_broadway_plain_bypass(vte: *mut GtkWidget, m: CellMetrics) {
        unsafe { vte_terminal_reset(vte, 0, 1) };
        let plain = "plain single pane output\r\n";
        unsafe { vte_terminal_feed(vte, plain.as_ptr() as *const _, plain.len() as isize) };
        std::thread::sleep(Duration::from_millis(30));
        pump_gtk_events();
        assert_eq!(send_vte_button_ev(vte, GDK_BUTTON_PRESS, 2, 0, 0, m), 0);
    }

    #[test]
    fn headless_broadway_tmux_drag_double_triple_click_and_ctrl_c() {
        with_headless_gtk(|| {
            let (win, _) = crate::tests::install_test_window();
            unsafe { gtk_window_set_default_size(win, 920, 420) };
            let sid = session_ops::add_tab("Tmux", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
            let vte = APP.with(|c| c.borrow().as_ref().unwrap().sessions[0].vte);
            unsafe { gtk_widget_show_all(win) };
            std::thread::sleep(Duration::from_millis(40));
            pump_gtk_events();
            let m = read_cell_metrics(vte).expect("vte metrics");
            feed_tmux_screen(vte, m);
            verify_broadway_drag_and_ctrl_c(win, vte, m);
            verify_broadway_word_and_line_clicks(win, vte, m);
            verify_broadway_clear_paths(vte, m);
            verify_broadway_plain_bypass(vte, m);
            crate::tests::cleanup_test_window(win, &[sid]);
        });
    }

    #[test]
    fn tmux_copy_module_maintains_grade_b_mi() {
        let src = include_str!("tmux_copy.rs");
        let (sloc, funcs, mi) = crate::mi::compute_file_mi(src);
        eprintln!("tmux_copy.rs: sloc={sloc}, funcs={funcs}, mi={mi:.2}");
        assert!(mi >= 60.0, "tmux_copy.rs MI {mi:.2} < 60.0");
    }
}
