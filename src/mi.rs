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

const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".next",
    ".cache",
    "akhterm_vendor",
    "archive",
];

const CODE_EXTS: &[&str] = &["rs", "py", "ts", "tsx", "js", "go", "cpp", "c"];

fn compute_block_mi(lines: &[&str]) -> (usize, usize, f64) {
    let mut sloc = 0usize;
    let mut cc = 1usize;
    let mut n1_total = 0usize;
    let mut n2_total = 0usize;
    let mut uniq_ops: HashSet<&str> = HashSet::new();
    let mut uniq_opnds: HashSet<&str> = HashSet::new();

    for raw_line in lines {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        sloc += 1;
        for token in line.split(|c: char| c.is_whitespace() || "(),;[]{}".contains(c)) {
            if token.is_empty() {
                continue;
            }
            match token {
                "if" | "elif" | "for" | "while" | "match" | "case" | "catch" | "except"
                | "&&" | "||" | "and" | "or" | "?" => {
                    cc += 1;
                    n1_total += 1;
                    uniq_ops.insert(token);
                }
                "fn" | "def" | "return" | "let" | "const" | "mut" | "pub" | "async" | "await"
                | "class" | "struct" | "impl" | "=" | "==" | "!=" | "+=" | "-=" | "->" | "=>" => {
                    n1_total += 1;
                    uniq_ops.insert(token);
                }
                _ => {
                    n2_total += 1;
                    if uniq_opnds.len() < 256 {
                        uniq_opnds.insert(token);
                    }
                }
            }
        }
    }

    if sloc == 0 {
        return (0, 1, 100.0);
    }

    let n = (n1_total + n2_total).max(2) as f64;
    let eta = (uniq_ops.len() + uniq_opnds.len()).max(2) as f64;
    let volume = (n * eta.log2()).max(1.0);
    let sloc_f = sloc as f64;
    let cc_f = cc as f64;

    let raw_mi = 171.0 - 5.2 * volume.ln() - 0.23 * cc_f - 16.2 * sloc_f.ln();
    let normalized = (raw_mi * 100.0 / 171.0).clamp(0.0, 100.0);
    (sloc, cc, normalized)
}

fn is_fn_boundary(trimmed: &str) -> bool {
    trimmed.starts_with("fn ")
        || trimmed.starts_with("pub fn ")
        || trimmed.starts_with("pub(crate) fn ")
        || trimmed.starts_with("extern \"C\" fn ")
        || trimmed.starts_with("async fn ")
        || trimmed.starts_with("def ")
        || trimmed.starts_with("async def ")
        || trimmed.starts_with("function ")
        || trimmed.starts_with("export function ")
        || trimmed.starts_with("export async function ")
}

pub fn compute_file_mi(content: &str) -> (usize, usize, f64) {
    let code_lines: Vec<&str> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
        .collect();

    if code_lines.is_empty() {
        return (0, 1, 100.0);
    }

    // Split into function blocks (or 45-line logical chunks if top-level script)
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in &code_lines {
        if (is_fn_boundary(line) && cur.len() >= 6) || cur.len() >= 55 {
            blocks.push(std::mem::take(&mut cur));
        }
        cur.push(*line);
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }

    let mut total_sloc = 0usize;
    let mut total_cc = 0usize;
    let mut mi_sum = 0.0f64;
    let mut count = 0usize;

    for blk in &blocks {
        let (s, c, m) = compute_block_mi(blk);
        if s > 0 {
            total_sloc += s;
            total_cc += c;
            mi_sum += m;
            count += 1;
        }
    }

    let avg_mi = if count > 0 { mi_sum / (count as f64) } else { 85.0 };
    let avg_block_cc = if count > 0 { (total_cc / count).max(1) } else { 1 };
    (total_sloc, avg_block_cc, avg_mi)
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
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if path.is_dir() {
            if !name_str.starts_with('.') && !SKIP_DIRS.contains(&name_str.as_ref()) {
                collect_code_files(root, &path, depth + 1, out);
            }
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if CODE_EXTS.contains(&ext) {
                if let Ok(meta) = entry.metadata() {
                    if meta.len() > 60 && meta.len() < 350_000 {
                        out.push(path);
                    }
                }
            }
        }
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

pub fn analyze_project_mi(root_dir: &str) -> ProjectMiReport {
    let root_path = Path::new(root_dir);
    let mut files = Vec::new();
    collect_code_files(root_path, root_path, 0, &mut files);

    let mut metrics = Vec::new();
    let mut total_sloc = 0usize;
    let mut mi_sum = 0.0f64;
    let mut total_cc = 0usize;

    for path in files {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let (sloc, cc, mi) = compute_file_mi(&content);
        if sloc < 4 {
            continue;
        }
        let rel = path
            .strip_prefix(root_path)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        total_sloc += sloc;
        total_cc += cc;
        mi_sum += mi;
        metrics.push(FileMetrics {
            rel_path: rel,
            sloc,
            cc,
            mi,
        });
    }

    let files_count = metrics.len();
    let avg_mi = if files_count > 0 {
        mi_sum / (files_count as f64)
    } else {
        85.0
    };
    let avg_cc = if files_count > 0 {
        (total_cc as f64) / (files_count as f64)
    } else {
        1.0
    };

    metrics.sort_by(|a, b| a.mi.partial_cmp(&b.mi).unwrap_or(std::cmp::Ordering::Equal));
    let worst_files = metrics.into_iter().take(4).collect();
    let grade = grade_for_mi(avg_mi);

    ProjectMiReport {
        root: root_dir.to_string(),
        avg_mi,
        grade,
        files_count,
        total_sloc,
        avg_cc,
        worst_files,
    }
}
