//! Pure-Rust unified diff parser, single-hunk patch builder, and Markdown batch formatter.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    NoNewline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
}

impl DiffLine {
    pub fn diff_prefix(&self) -> &'static str {
        match self.kind {
            DiffLineKind::Context => " ",
            DiffLineKind::Added => "+",
            DiffLineKind::Removed => "-",
            DiffLineKind::NoNewline => "",
        }
    }

    pub fn raw_patch_line(&self) -> String {
        format!("{}{}", self.diff_prefix(), self.text)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    pub old_path: String,
    pub new_path: String,
    pub display_path: String,
    pub is_new: bool,
    pub is_deleted: bool,
    pub is_binary: bool,
    pub insertions: usize,
    pub deletions: usize,
    pub file_headers: Vec<String>,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderedLineKind {
    FileHeader,
    HunkHeader,
    Context,
    Added,
    Removed,
    NoNewline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedLine {
    pub char_start: i32,
    pub char_end: i32,
    pub kind: RenderedLineKind,
    pub hunk_idx: Option<usize>,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub diff_text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderedDiff {
    pub text: String,
    pub lines: Vec<RenderedLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedExcerpt {
    pub hunk_idx: usize,
    pub start_line: u32,
    pub end_line: u32,
    pub excerpt: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewComment {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub excerpt: String,
    pub comment: String,
    pub remote_host: Option<String>,
}

fn decode_octal_escape(bytes: &[u8], pos: usize) -> Option<(u8, usize)> {
    let slice = bytes.get(pos..pos + 3)?;
    if !slice.iter().all(|b| (b'0'..=b'7').contains(b)) {
        return None;
    }
    let value = ((slice[0] - b'0') << 6) | ((slice[1] - b'0') << 3) | (slice[2] - b'0');
    Some((value, pos + 3))
}

fn decode_escape_byte(bytes: &[u8], pos: usize) -> (u8, usize) {
    if let Some(octal) = decode_octal_escape(bytes, pos) {
        return octal;
    }
    let ch = match bytes.get(pos).copied().unwrap_or(b'\\') {
        b'n' => b'\n',
        b't' => b'\t',
        b'r' => b'\r',
        b'"' => b'"',
        b'\\' => b'\\',
        other => other,
    };
    (ch, (pos + 1).min(bytes.len()))
}

fn unescape_quoted_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        let (byte, next) = if bytes[idx] == b'\\' && idx + 1 < bytes.len() {
            decode_escape_byte(bytes, idx + 1)
        } else {
            (bytes[idx], idx + 1)
        };
        out.push(byte);
        idx = next;
    }
    out
}

pub fn unescape_git_path(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some(inner) = trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return trimmed.to_owned();
    };
    String::from_utf8_lossy(&unescape_quoted_bytes(inner.as_bytes())).into_owned()
}

fn strip_ab_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")) {
        return rest.to_owned();
    }
    path.to_owned()
}

fn scan_quoted_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
        } else if bytes[i] == b'"' {
            return i + 1;
        } else {
            i += 1;
        }
    }
    i
}

fn next_path_token(rest: &str, from: usize) -> Option<(String, usize)> {
    let bytes = rest.as_bytes();
    let start = (from..bytes.len()).find(|&idx| bytes[idx] != b' ')?;
    let end = if bytes[start] == b'"' {
        scan_quoted_end(bytes, start)
    } else {
        (start..bytes.len()).find(|&idx| bytes[idx] == b' ').unwrap_or(bytes.len())
    };
    Some((strip_ab_prefix(&unescape_git_path(&rest[start..end])), end))
}

fn split_quoted_pair(rest: &str) -> Option<(String, String)> {
    let (first, pos) = next_path_token(rest, 0)?;
    let (second, _) = next_path_token(rest, pos)?;
    Some((first, second))
}

fn parse_diff_git_paths(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("diff --git ")?;
    if rest.starts_with('"') || rest.contains(" \"b/") {
        return split_quoted_pair(rest);
    }
    if let Some((left, right)) = rest.split_once(" b/") {
        return Some((strip_ab_prefix(left), right.to_owned()));
    }
    split_quoted_pair(rest)
}

fn parse_header_path(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?;
    let raw = rest.split('\t').next().unwrap_or(rest).trim();
    if raw == "/dev/null" {
        return Some("/dev/null".to_owned());
    }
    Some(strip_ab_prefix(&unescape_git_path(raw)))
}

fn parse_hunk_range(token: &str, sign: char) -> Option<(u32, u32)> {
    let body = token.strip_prefix(sign)?;
    if let Some((start, count)) = body.split_once(',') {
        return Some((start.parse().ok()?, count.parse().ok()?));
    }
    Some((body.parse().ok()?, 1))
}

pub fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let mut tokens = ranges.split_whitespace();
    let (old_start, old_count) = parse_hunk_range(tokens.next()?, '-')?;
    let (new_start, new_count) = parse_hunk_range(tokens.next()?, '+')?;
    Some(DiffHunk {
        old_start,
        old_count,
        new_start,
        new_count,
        header: line.to_owned(),
        lines: Vec::new(),
    })
}

fn new_diff_file(old_path: String, new_path: String, header_line: &str) -> DiffFile {
    let display_path = if new_path.is_empty() || new_path == "/dev/null" {
        old_path.clone()
    } else {
        new_path.clone()
    };
    DiffFile {
        old_path,
        new_path,
        display_path,
        is_new: false,
        is_deleted: false,
        is_binary: false,
        insertions: 0,
        deletions: 0,
        file_headers: vec![header_line.to_owned()],
        hunks: Vec::new(),
    }
}

fn refresh_display_path(file: &mut DiffFile) {
    file.display_path = if file.new_path.is_empty() || file.new_path == "/dev/null" {
        file.old_path.clone()
    } else {
        file.new_path.clone()
    };
}

fn apply_file_path_meta(file: &mut DiffFile, line: &str) {
    if let Some(path) = parse_header_path(line, "--- ") {
        file.is_new |= path == "/dev/null";
        file.old_path = path;
    } else if let Some(path) = parse_header_path(line, "+++ ") {
        file.is_deleted |= path == "/dev/null";
        file.new_path = path;
        refresh_display_path(file);
    } else if let Some(rest) = line.strip_prefix("rename to ") {
        file.new_path = unescape_git_path(rest);
        refresh_display_path(file);
    }
}

fn apply_file_meta_line(file: &mut DiffFile, line: &str) {
    file.is_new |= line.starts_with("new file mode ");
    file.is_deleted |= line.starts_with("deleted file mode ");
    file.is_binary |= line.starts_with("Binary files ") || line == "GIT binary patch";
    apply_file_path_meta(file, line);
    file.file_headers.push(line.to_owned());
}

fn classify_hunk_line(line: &str, old_ln: &mut u32, new_ln: &mut u32) -> Option<(DiffLine, usize, usize)> {
    if line.starts_with("\\ ") {
        let dl = DiffLine { kind: DiffLineKind::NoNewline, old_line: None, new_line: None, text: line.to_owned() };
        return Some((dl, 0, 0));
    }
    let (prefix, rest) = line.split_at(line.chars().next().map_or(0, char::len_utf8));
    let (kind, ol, nl, ins, del) = match prefix {
        "+" => { let n = *new_ln; *new_ln += 1; (DiffLineKind::Added, None, Some(n), 1, 0) }
        "-" => { let o = *old_ln; *old_ln += 1; (DiffLineKind::Removed, Some(o), None, 0, 1) }
        " " => { let (o, n) = (*old_ln, *new_ln); *old_ln += 1; *new_ln += 1; (DiffLineKind::Context, Some(o), Some(n), 0, 0) }
        _ => return None,
    };
    Some((DiffLine { kind, old_line: ol, new_line: nl, text: rest.to_owned() }, ins, del))
}

fn push_hunk_line(hunk: &mut DiffHunk, line: &str, old_ln: &mut u32, new_ln: &mut u32) -> (usize, usize) {
    let Some((diff_line, ins, del)) = classify_hunk_line(line, old_ln, new_ln) else {
        return (0, 0);
    };
    hunk.lines.push(diff_line);
    (ins, del)
}

fn apply_diff_body_line(file: &mut DiffFile, line: &str, old_ln: &mut u32, new_ln: &mut u32, in_hunk: &mut bool) {
    if let Some(hunk) = parse_hunk_header(line) {
        *old_ln = hunk.old_start;
        *new_ln = hunk.new_start;
        file.hunks.push(hunk);
        *in_hunk = true;
        return;
    }
    if !*in_hunk {
        apply_file_meta_line(file, line);
        return;
    }
    if let Some(hunk) = file.hunks.last_mut() {
        let (ins, del) = push_hunk_line(hunk, line, old_ln, new_ln);
        file.insertions += ins;
        file.deletions += del;
    }
}

pub fn parse_unified_diff(raw: &str) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    let (mut old_ln, mut new_ln, mut in_hunk) = (0u32, 0u32, false);
    for raw_line in raw.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if let Some((old_p, new_p)) = parse_diff_git_paths(line) {
            files.push(new_diff_file(old_p, new_p, line));
            in_hunk = false;
        } else if let Some(file) = files.last_mut() {
            apply_diff_body_line(file, line, &mut old_ln, &mut new_ln, &mut in_hunk);
        }
    }
    files
}

fn keep_patch_header(line: &str, include_mode: bool) -> bool {
    line.starts_with("diff --git ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
        || (include_mode && (line.starts_with("new file mode ") || line.starts_with("deleted file mode ")))
}

fn append_hunk_lines(out: &mut String, hunk: &DiffHunk) {
    out.push_str(&hunk.header);
    out.push('\n');
    for line in &hunk.lines {
        out.push_str(&line.raw_patch_line());
        out.push('\n');
    }
}

pub fn reconstruct_hunk_patch(file: &DiffFile, hunk_idx: usize) -> Option<String> {
    if file.is_binary {
        return None;
    }
    let hunk = file.hunks.get(hunk_idx)?;
    let include_mode = file.hunks.len() == 1 && (file.is_new || file.is_deleted);
    let mut out = String::new();
    for header in file.file_headers.iter().filter(|h| keep_patch_header(h, include_mode)) {
        out.push_str(header);
        out.push('\n');
    }
    append_hunk_lines(&mut out, hunk);
    Some(out)
}

fn format_lineno(num: Option<u32>) -> String {
    num.map_or_else(|| "    ".to_owned(), |n| format!("{n:>4}"))
}

fn push_rendered_line(
    out: &mut RenderedDiff,
    cursor: &mut i32,
    display: &str,
    kind: RenderedLineKind,
    hunk_idx: Option<usize>,
    nums: (Option<u32>, Option<u32>),
    diff_text: String,
) {
    let start = *cursor;
    let chars = display.chars().count() as i32;
    out.text.push_str(display);
    out.text.push('\n');
    *cursor += chars + 1;
    out.lines.push(RenderedLine {
        char_start: start, char_end: start + chars, kind, hunk_idx,
        old_line: nums.0, new_line: nums.1, diff_text,
    });
}

fn map_diff_line_kind(kind: DiffLineKind) -> RenderedLineKind {
    match kind {
        DiffLineKind::Context => RenderedLineKind::Context,
        DiffLineKind::Added => RenderedLineKind::Added,
        DiffLineKind::Removed => RenderedLineKind::Removed,
        DiffLineKind::NoNewline => RenderedLineKind::NoNewline,
    }
}

fn render_hunk(out: &mut RenderedDiff, cursor: &mut i32, hunk_idx: usize, hunk: &DiffHunk) {
    push_rendered_line(out, cursor, &hunk.header, RenderedLineKind::HunkHeader, Some(hunk_idx), (None, None), hunk.header.clone());
    for line in &hunk.lines {
        let diff_text = line.raw_patch_line();
        let display = format!("{} {} │ {diff_text}", format_lineno(line.old_line), format_lineno(line.new_line));
        push_rendered_line(out, cursor, &display, map_diff_line_kind(line.kind), Some(hunk_idx), (line.old_line, line.new_line), diff_text);
    }
}

pub fn render_file_diff(file: &DiffFile) -> RenderedDiff {
    let mut out = RenderedDiff::default();
    let mut cursor = 0i32;
    if file.is_binary {
        let msg = format!("Binary file: {}", file.display_path);
        push_rendered_line(&mut out, &mut cursor, &msg, RenderedLineKind::FileHeader, None, (None, None), msg.clone());
        return out;
    }
    for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
        render_hunk(&mut out, &mut cursor, hunk_idx, hunk);
    }
    out
}

pub fn active_hunk_at_offset(rendered: &RenderedDiff, char_offset: i32) -> Option<usize> {
    let pos = char_offset.max(0);
    for line in &rendered.lines {
        if pos <= line.char_end + 1 {
            if let Some(idx) = line.hunk_idx {
                return Some(idx);
            }
        }
    }
    rendered.lines.iter().rev().find_map(|l| l.hunk_idx)
}

fn line_number_for_review(line: &RenderedLine) -> Option<u32> {
    line.new_line.or(line.old_line)
}

fn build_excerpt_from_lines(lines: &[&RenderedLine], hunk_idx: usize) -> Option<SelectedExcerpt> {
    if lines.is_empty() {
        return None;
    }
    let start_line = lines.iter().find_map(|l| line_number_for_review(l)).unwrap_or(1);
    let end_line = lines.iter().rev().find_map(|l| line_number_for_review(l)).unwrap_or(start_line);
    let excerpt = lines.iter().map(|l| l.diff_text.as_str()).collect::<Vec<_>>().join("\n");
    Some(SelectedExcerpt {
        hunk_idx,
        start_line,
        end_line: end_line.max(start_line),
        excerpt,
    })
}

fn select_range_excerpt(rendered: &RenderedDiff, lo: i32, hi: i32) -> Option<SelectedExcerpt> {
    let picked: Vec<&RenderedLine> = rendered
        .lines
        .iter()
        .filter(|l| l.char_end >= lo && l.char_start < hi && l.hunk_idx.is_some())
        .collect();
    let hunk_idx = picked.first().and_then(|l| l.hunk_idx)?;
    build_excerpt_from_lines(&picked, hunk_idx)
}

pub fn extract_selection_excerpt(
    rendered: &RenderedDiff,
    start_char: i32,
    end_char: i32,
    fallback_hunk: usize,
) -> Option<SelectedExcerpt> {
    let (lo, hi) = (start_char.min(end_char), start_char.max(end_char));
    if lo < hi {
        if let Some(hit) = select_range_excerpt(rendered, lo, hi) {
            return Some(hit);
        }
    }
    let hunk_idx = active_hunk_at_offset(rendered, lo).unwrap_or(fallback_hunk);
    let hunk_lines: Vec<&RenderedLine> = rendered.lines.iter().filter(|l| l.hunk_idx == Some(hunk_idx)).collect();
    build_excerpt_from_lines(&hunk_lines, hunk_idx)
}

pub fn hunk_start_char(rendered: &RenderedDiff, hunk_idx: usize) -> Option<i32> {
    rendered
        .lines
        .iter()
        .find(|l| l.hunk_idx == Some(hunk_idx) && l.kind == RenderedLineKind::HunkHeader)
        .map(|l| l.char_start)
}

pub fn has_unsafe_control(text: &str) -> bool {
    text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
}

fn safe_fence(text: &str) -> String {
    let run = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(3.max(run + 1))
}

pub fn format_comment_location(item: &ReviewComment) -> String {
    let lines = if item.start_line == item.end_line {
        format!("L{}", item.start_line)
    } else {
        format!("L{}-L{}", item.start_line, item.end_line)
    };
    match &item.remote_host {
        Some(host) if !host.is_empty() => format!("{host}:{}:{lines}", item.file_path),
        _ => format!("{}:{lines}", item.file_path),
    }
}

fn is_safe_review_item(location: &str, excerpt: &str, comment: &str) -> bool {
    !comment.is_empty()
        && !has_unsafe_control(location)
        && !has_unsafe_control(excerpt)
        && !has_unsafe_control(comment)
}

fn format_single_review_item(idx: usize, item: &ReviewComment) -> Option<String> {
    let location = format_comment_location(item);
    let comment = item.comment.trim();
    if !is_safe_review_item(&location, &item.excerpt, comment) {
        return None;
    }
    let excerpt = item.excerpt.replace("\r\n", "\n");
    let fence = safe_fence(&excerpt);
    let nl = if excerpt.ends_with('\n') { "" } else { "\n" };
    Some(format!("{idx}. `{location}`\n{fence}diff\n{excerpt}{nl}{fence}\n{comment}"))
}

pub fn format_review_batch(comments: &[ReviewComment], scope_label: Option<&str>) -> Option<String> {
    if comments.is_empty() {
        return None;
    }
    let mut blocks = Vec::with_capacity(comments.len());
    for (idx, item) in comments.iter().enumerate() {
        blocks.push(format_single_review_item(idx + 1, item)?);
    }
    let header = match scope_label {
        Some(scope) if !scope.is_empty() && !has_unsafe_control(scope) => {
            format!("## Diff Review ({scope})\n")
        }
        _ => "## Diff Review\n".to_owned(),
    };
    Some(format!("{header}\n{}\n", blocks.join("\n\n")))
}
