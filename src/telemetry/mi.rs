//! Per-function Maintainability Index (MI) analyzer (SEI / Radon 0..100 standard).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FileMetrics {
    pub rel_path: String,
    pub sloc: usize,
    pub cc: usize,
    pub mi: f64,
}

#[derive(Clone, Debug)]
pub struct ProjectMiReport {
    pub root: String,
    pub avg_mi: f64,
    pub grade: &'static str,
    pub files_count: usize,
    pub total_sloc: usize,
    pub avg_cc: f64,
    pub worst_files: Vec<FileMetrics>,
}

enum TokenKind {
    Branch,
    Operator,
    Operand,
}

struct BlockStats<'a> {
    sloc: usize,
    cc: usize,
    n1: usize,
    n2: usize,
    ops: HashSet<&'a str>,
    opnds: HashSet<&'a str>,
}

fn new_block_stats<'a>() -> BlockStats<'a> {
    BlockStats {
        sloc: 0,
        cc: 1,
        n1: 0,
        n2: 0,
        ops: HashSet::new(),
        opnds: HashSet::new(),
    }
}

fn is_code_line(trimmed: &str) -> bool {
    !trimmed.is_empty() && !trimmed.starts_with("//") && !trimmed.starts_with('#')
}

fn is_token_delim(c: char) -> bool {
    c.is_whitespace() || "(),;[]{}".contains(c)
}

fn is_branch_token(token: &str) -> bool {
    const TOKENS: &[&str] = &[
        "if", "elif", "for", "while", "match", "case", "catch", "except",
        "&&", "||", "and", "or", "?",
    ];
    TOKENS.contains(&token)
}

fn is_operator_token(token: &str) -> bool {
    const TOKENS: &[&str] = &[
        "fn", "def", "return", "let", "const", "mut", "pub", "async", "await",
        "class", "struct", "impl", "=", "==", "!=", "+=", "-=", "->", "=>",
    ];
    TOKENS.contains(&token)
}

fn classify_token(token: &str) -> TokenKind {
    if is_branch_token(token) {
        TokenKind::Branch
    } else if is_operator_token(token) {
        TokenKind::Operator
    } else {
        TokenKind::Operand
    }
}

fn record_operator<'a>(stats: &mut BlockStats<'a>, token: &'a str, is_branch: bool) {
    if is_branch {
        stats.cc += 1;
    }
    stats.n1 += 1;
    stats.ops.insert(token);
}

fn record_operand<'a>(stats: &mut BlockStats<'a>, token: &'a str) {
    stats.n2 += 1;
    if stats.opnds.len() < 256 {
        stats.opnds.insert(token);
    }
}

fn record_token<'a>(stats: &mut BlockStats<'a>, token: &'a str) {
    match classify_token(token) {
        TokenKind::Branch => record_operator(stats, token, true),
        TokenKind::Operator => record_operator(stats, token, false),
        TokenKind::Operand => record_operand(stats, token),
    }
}

fn scan_code_line<'a>(stats: &mut BlockStats<'a>, line: &'a str) {
    stats.sloc += 1;
    for token in line.split(is_token_delim).filter(|t| !t.is_empty()) {
        record_token(stats, token);
    }
}

fn score_block(sloc: usize, cc: usize, n_total: usize, eta_total: usize) -> f64 {
    let n = n_total.max(2) as f64;
    let eta = eta_total.max(2) as f64;
    let volume = (n * eta.log2()).max(1.0);
    let raw_mi = 171.0 - 5.2 * volume.ln() - 0.23 * (cc as f64) - 16.2 * (sloc as f64).ln();
    (raw_mi * 100.0 / 171.0).clamp(0.0, 100.0)
}

fn compute_block_mi(lines: &[&str]) -> (usize, usize, f64) {
    let mut stats = new_block_stats();
    for line in lines.iter().map(|raw| raw.trim()).filter(|l| is_code_line(l)) {
        scan_code_line(&mut stats, line);
    }
    if stats.sloc == 0 {
        return (0, 1, 100.0);
    }
    let mi = score_block(
        stats.sloc,
        stats.cc,
        stats.n1 + stats.n2,
        stats.ops.len() + stats.opnds.len(),
    );
    (stats.sloc, stats.cc, mi)
}

fn is_fn_boundary(trimmed: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "fn ", "pub fn ", "pub(crate) fn ", "extern \"C\" fn ", "async fn ",
        "def ", "async def ", "function ", "export function ", "export async function ",
    ];
    PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix))
}

fn extract_code_lines(content: &str) -> Vec<&str> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| is_code_line(line))
        .collect()
}

fn split_function_blocks<'a>(code_lines: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for &line in code_lines {
        if (is_fn_boundary(line) && cur.len() >= 6) || cur.len() >= 55 {
            blocks.push(std::mem::take(&mut cur));
        }
        cur.push(line);
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }
    blocks
}

fn aggregate_blocks(blocks: &[Vec<&str>]) -> (usize, usize, f64) {
    let (mut total_sloc, mut total_cc, mut mi_sum, mut count) = (0, 0, 0.0, 0);
    for blk in blocks {
        let (s, c, m) = compute_block_mi(blk);
        if s > 0 {
            total_sloc += s;
            total_cc += c;
            mi_sum += m;
            count += 1;
        }
    }
    let avg_mi = if count > 0 { mi_sum / (count as f64) } else { 85.0 };
    let avg_cc = if count > 0 { (total_cc / count).max(1) } else { 1 };
    (total_sloc, avg_cc, avg_mi)
}

pub fn compute_file_mi(content: &str) -> (usize, usize, f64) {
    let code_lines = extract_code_lines(content);
    if code_lines.is_empty() {
        return (0, 1, 100.0);
    }
    let blocks = split_function_blocks(&code_lines);
    aggregate_blocks(&blocks)
}

fn should_visit_dir(name: &str) -> bool {
    const SKIP_DIRS: &[&str] = &[
        ".git", "node_modules", "target", ".venv", "venv", "__pycache__",
        "dist", "build", ".next", ".cache", "akhterm_vendor", "archive",
    ];
    !name.starts_with('.') && !SKIP_DIRS.contains(&name)
}

fn is_code_file(path: &Path, entry: &fs::DirEntry) -> bool {
    const CODE_EXTS: &[&str] = &["rs", "py", "ts", "tsx", "js", "go", "cpp", "c"];
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    if !CODE_EXTS.contains(&ext) {
        return false;
    }
    entry
        .metadata()
        .is_ok_and(|meta| meta.len() > 60 && meta.len() < 350_000)
}

fn visit_dir_entry(root: &Path, entry: fs::DirEntry, depth: usize, out: &mut Vec<PathBuf>) {
    let path = entry.path();
    let name = entry.file_name();
    let name_str = name.to_string_lossy();
    if path.is_dir() {
        if should_visit_dir(&name_str) {
            collect_code_files(root, &path, depth + 1, out);
        }
    } else if is_code_file(&path, &entry) {
        out.push(path);
    }
}

fn collect_code_files(root: &Path, current: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 || out.len() >= 180 {
        return;
    }
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= 180 {
            break;
        }
        visit_dir_entry(root, entry, depth, out);
    }
}

pub fn grade_for_mi(mi: f64) -> &'static str {
    if mi >= 75.0 {
        "A"
    } else if mi >= 58.0 {
        "B"
    } else {
        "C"
    }
}

fn measure_file(root_path: &Path, path: &Path) -> Option<FileMetrics> {
    let content = fs::read_to_string(path).ok()?;
    let (sloc, cc, mi) = compute_file_mi(&content);
    if sloc < 4 {
        return None;
    }
    let rel_path = path
        .strip_prefix(root_path)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    Some(FileMetrics {
        rel_path,
        sloc,
        cc,
        mi,
    })
}

fn summarize_metrics(root_dir: &str, mut metrics: Vec<FileMetrics>) -> ProjectMiReport {
    let files_count = metrics.len();
    let total_sloc: usize = metrics.iter().map(|m| m.sloc).sum();
    let total_cc: usize = metrics.iter().map(|m| m.cc).sum();
    let mi_sum: f64 = metrics.iter().map(|m| m.mi).sum();
    let avg_mi = if files_count > 0 { mi_sum / (files_count as f64) } else { 85.0 };
    let avg_cc = if files_count > 0 { (total_cc as f64) / (files_count as f64) } else { 1.0 };
    metrics.sort_by(|a, b| a.mi.partial_cmp(&b.mi).unwrap_or(std::cmp::Ordering::Equal));
    ProjectMiReport {
        root: root_dir.to_string(),
        avg_mi,
        grade: grade_for_mi(avg_mi),
        files_count,
        total_sloc,
        avg_cc,
        worst_files: metrics.into_iter().take(4).collect(),
    }
}

pub fn analyze_project_mi(root_dir: &str) -> ProjectMiReport {
    let root_path = Path::new(root_dir);
    let mut files = Vec::new();
    collect_code_files(root_path, root_path, 0, &mut files);
    let metrics: Vec<FileMetrics> = files
        .iter()
        .filter_map(|path| measure_file(root_path, path))
        .collect();
    summarize_metrics(root_dir, metrics)
}
