//! Antigravity CLI (AGY) transcript discovery and live VTE `/context` panel parser.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::telemetry::{clean_status_line, parse_human_tokens, parse_percent_token, strip_punct};

pub const AGY_SNIPPET_BYTES: u64 = 65_536;
pub const AGY_CANDIDATE_CAP: usize = 64;
const MIN_CONTEXT_PANEL_LIMIT: u64 = 10_000;
const BREAKDOWN_MARKERS: [&str; 14] = [
    "checkpoint buffer", "autocompact buffer", "free space", "user messages", "agent responses",
    "tool calls", "tool results", "system prompt", "system tools", "system messages", "skills:",
    "subagents:", ".md:", "skill.md",
];

pub(crate) fn metadata_stamp(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64)
}

pub(crate) fn trim_trailing_slash(path: &str) -> &str {
    if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    }
}

pub fn collect_subagent_ids(brain_root: &Path) -> HashSet<String> {
    let mut ids = HashSet::new();
    let Ok(entries) = fs::read_dir(brain_root) else { return ids };
    for entry in entries.flatten() {
        collect_json_stems(&entry.path().join(".system_generated/subagents"), &mut ids);
    }
    ids
}

fn json_stem_of(path: &Path) -> Option<String> {
    let is_json = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    is_json
        .then(|| path.file_stem().and_then(|s| s.to_str()))
        .flatten()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn collect_json_stems(dir: &Path, ids: &mut HashSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if let Some(stem) = json_stem_of(&entry.path()) {
            ids.insert(stem);
        }
    }
}

pub fn collect_agy_candidates(brain_root: &Path) -> Vec<(u64, PathBuf)> {
    let Ok(entries) = fs::read_dir(brain_root) else { return Vec::new() };
    let subagents = collect_subagent_ids(brain_root);
    let mut list = Vec::new();
    for entry in entries.flatten() {
        if !subagents.contains(entry.file_name().to_string_lossy().as_ref()) {
            push_candidate_if_file(&entry.path(), &mut list);
        }
    }
    list.sort_by(|a, b| b.0.cmp(&a.0));
    list.truncate(AGY_CANDIDATE_CAP);
    list
}

fn push_candidate_if_file(brain_entry: &Path, list: &mut Vec<(u64, PathBuf)>) {
    let candidate = brain_entry.join(".system_generated/logs/transcript.jsonl");
    if let Ok(meta) = fs::metadata(&candidate) {
        if meta.is_file() {
            list.push((metadata_stamp(&meta), candidate));
        }
    }
}

pub fn transcript_from_fd_target(target: &Path, brain_root: &Path) -> Option<PathBuf> {
    let rel = target.strip_prefix(brain_root).ok()?;
    let id = rel.components().next()?.as_os_str();
    if id.is_empty() {
        return None;
    }
    let candidate = brain_root.join(id).join(".system_generated/logs/transcript.jsonl");
    candidate.is_file().then_some(candidate)
}

pub(crate) fn discover_agy_by_fd(root_pid: i32, home: &Path) -> Option<PathBuf> {
    let brain_root = home.join(".gemini/antigravity-cli/brain");
    if !brain_root.is_dir() {
        return None;
    }
    let subagents = collect_subagent_ids(&brain_root);
    let mut fallback = None;
    for pid in collect_tree_pids(root_pid) {
        if let Some(exact) = scan_pid_fds_for_agy(pid, &brain_root, &subagents, &mut fallback) {
            return Some(exact);
        }
    }
    fallback.map(|(_, path)| path)
}

fn collect_tree_pids(root_pid: i32) -> Vec<i32> {
    if root_pid <= 0 {
        return Vec::new();
    }
    let mut visited = Vec::new();
    let mut queue = vec![root_pid];
    while let Some(pid) = queue.pop() {
        if visited.len() < 64 && !visited.contains(&pid) {
            visited.push(pid);
            push_child_pids(pid, &mut queue);
        }
    }
    visited
}

fn push_child_pids(pid: i32, queue: &mut Vec<i32>) {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else { return };
    for task in tasks.flatten() {
        if let Ok(raw) = fs::read_to_string(task.path().join("children")) {
            queue.extend(raw.split_whitespace().filter_map(|s| s.parse::<i32>().ok()));
        }
    }
}

fn resolve_fd_transcript(
    target: &Path,
    brain_root: &Path,
    subagents: &HashSet<String>,
) -> Option<(bool, PathBuf)> {
    let transcript = transcript_from_fd_target(target, brain_root)?;
    let comp = transcript.strip_prefix(brain_root).ok().and_then(|r| r.components().next());
    if comp.is_some_and(|c| subagents.contains(c.as_os_str().to_string_lossy().as_ref())) {
        return None;
    }
    let direct = target.file_name().is_some_and(|n| n.to_string_lossy().starts_with("transcript"));
    Some((direct, transcript))
}

fn scan_pid_fds_for_agy(
    pid: i32,
    brain_root: &Path,
    subagents: &HashSet<String>,
    fallback: &mut Option<(u64, PathBuf)>,
) -> Option<PathBuf> {
    let entries = fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    for entry in entries.flatten() {
        let Ok(target) = fs::read_link(entry.path()) else { continue };
        if let Some((direct, transcript)) = resolve_fd_transcript(&target, brain_root, subagents) {
            if direct {
                return Some(transcript);
            }
            update_fd_fallback(&transcript, fallback);
        }
    }
    None
}

fn update_fd_fallback(transcript: &Path, fallback: &mut Option<(u64, PathBuf)>) {
    let stamp = fs::metadata(transcript).map_or(0, |m| metadata_stamp(&m));
    if fallback.as_ref().is_none_or(|(best, _)| stamp > *best) {
        *fallback = Some((stamp, transcript.to_path_buf()));
    }
}

fn has_nested_git_marker(rel_path: &str, abs_cwd: &Path) -> bool {
    let mut cur = abs_cwd.to_path_buf();
    for seg in rel_path.split('/').filter(|s| !s.is_empty() && *s != ".") {
        if seg == ".." {
            return true;
        }
        cur.push(seg);
        if cur.join(".git").exists() {
            return true;
        }
    }
    false
}

fn subpath_belongs_to_workspace(after_slash: &str, abs_cwd: &Path) -> bool {
    if after_slash.starts_with(".gemini/antigravity-cli") {
        return false;
    }
    let end = after_slash
        .find(|c: char| c.is_whitespace() || "\"'`\\)]},:;<>|#?".contains(c))
        .unwrap_or(after_slash.len());
    !has_nested_git_marker(&after_slash[..end], abs_cwd)
}

fn boundary_matches_dir(rest: &str, abs_cwd: &Path) -> bool {
    match rest.chars().next() {
        Some('/') => subpath_belongs_to_workspace(&rest[1..], abs_cwd),
        Some(c) if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') => false,
        _ => true,
    }
}

pub fn text_mentions_prefix_for_dir(text: &str, prefix: &str, abs_cwd: &Path) -> bool {
    let clean = trim_trailing_slash(prefix);
    if clean.is_empty() || clean == "/" {
        return false;
    }
    text.match_indices(clean)
        .any(|(idx, _)| boundary_matches_dir(&text[idx + clean.len()..], abs_cwd))
}

pub fn text_mentions_workspace(text: &str, cwd: &str) -> Option<bool> {
    let clean = trim_trailing_slash(cwd.trim());
    if clean.is_empty() || clean == "/" {
        return None;
    }
    text_mentions_prefix_for_dir(text, clean, Path::new(clean)).then_some(true)
}

fn resolve_abs_cwd(home: &Path, cwd: &str) -> PathBuf {
    let norm = trim_trailing_slash(cwd.trim());
    if norm == "~" {
        return home.to_path_buf();
    }
    if let Some(rel) = norm.strip_prefix("~/") {
        return home.join(rel);
    }
    PathBuf::from(norm)
}

fn push_unique_prefix(out: &mut Vec<String>, candidate: &str) {
    let clean = trim_trailing_slash(candidate.trim());
    if !clean.is_empty() && clean != "/" && !out.iter().any(|p| p == clean) {
        out.push(clean.to_owned());
    }
}

fn push_tilde_prefix(out: &mut Vec<String>, base_home: &Path, target: &Path) {
    if let Ok(rel) = target.strip_prefix(base_home) {
        if !rel.as_os_str().is_empty() {
            let tilde = format!("~/{}", rel.to_string_lossy());
            push_unique_prefix(out, &tilde);
        }
    }
}

fn build_workspace_prefixes(home: &Path, cwd: &str, abs_cwd: &Path, canon: Option<&Path>) -> Vec<String> {
    let mut out = Vec::with_capacity(4);
    push_unique_prefix(&mut out, cwd);
    push_unique_prefix(&mut out, &abs_cwd.to_string_lossy());
    push_tilde_prefix(&mut out, home, abs_cwd);
    if let Some(cp) = canon {
        push_unique_prefix(&mut out, &cp.to_string_lossy());
        if let Ok(canon_home) = fs::canonicalize(home) {
            push_tilde_prefix(&mut out, &canon_home, cp);
        }
    }
    out
}

fn read_chunk_at(file: &mut File, offset: u64, max_bytes: u64) -> Option<String> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = Vec::new();
    file.take(max_bytes).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn read_head_and_tail(path: &Path, max_bytes: u64) -> Option<(String, Option<String>)> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let head = read_chunk_at(&mut file, 0, max_bytes)?;
    if len <= max_bytes {
        return Some((head, None));
    }
    let tail = read_chunk_at(&mut file, len - max_bytes, max_bytes)?;
    Some((head, Some(tail)))
}

fn snippet_matches_any(snippet: &str, prefixes: &[String], abs_cwd: &Path) -> bool {
    prefixes.iter().any(|prefix| {
        if Path::new(prefix) == abs_cwd {
            text_mentions_workspace(snippet, prefix).is_some()
        } else {
            text_mentions_prefix_for_dir(snippet, prefix, abs_cwd)
        }
    })
}

fn match_agy_candidate(path: &Path, prefixes: &[String], abs_cwd: &Path) -> bool {
    let Some((head, tail)) = read_head_and_tail(path, AGY_SNIPPET_BYTES) else { return false };
    if snippet_matches_any(&head, prefixes, abs_cwd) {
        return true;
    }
    tail.as_deref().is_some_and(|t| snippet_matches_any(t, prefixes, abs_cwd))
}

pub fn discover_agy_transcript(home: &Path, cwd: &str, root_pid: i32) -> Option<PathBuf> {
    if let Some(found) = discover_agy_by_fd(root_pid, home) {
        return Some(found);
    }
    let abs_cwd = resolve_abs_cwd(home, cwd);
    if abs_cwd.as_os_str().is_empty() || abs_cwd == Path::new("/") {
        return None;
    }
    let canon = fs::canonicalize(&abs_cwd).ok();
    let check_root = canon.as_deref().unwrap_or(&abs_cwd);
    let prefixes = build_workspace_prefixes(home, cwd, &abs_cwd, canon.as_deref());
    collect_agy_candidates(&home.join(".gemini/antigravity-cli/brain"))
        .into_iter()
        .map(|(_, path)| path)
        .find(|path| match_agy_candidate(path, &prefixes, check_root))
}

pub fn is_vte_breakdown_line(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    let low = line.to_ascii_lowercase();
    BREAKDOWN_MARKERS.iter().any(|marker| low.contains(marker))
}

fn split_vte_words(line: &str) -> Vec<&str> {
    let words: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || matches!(c, '|' | '·' | '•'))
        .filter(|s| !s.is_empty())
        .collect();
    words
}

fn parse_panel_ratio(raw: &str) -> Option<(u64, u64)> {
    let (used_s, limit_s) = strip_punct(raw).split_once('/')?;
    let used = parse_human_tokens(used_s)?;
    let limit = parse_human_tokens(limit_s)?;
    let valid = limit >= MIN_CONTEXT_PANEL_LIMIT && used <= limit.saturating_mul(2);
    valid.then_some((used, limit))
}

fn resolve_panel_pct(used: u64, limit: u64, same_tail: &[&str], next_line: Option<&str>) -> u8 {
    if let Some(pct) = same_tail.iter().find_map(|w| parse_percent_token(w)) {
        return pct;
    }
    if let Some(pct) = next_line.and_then(|l| split_vte_words(l).first().copied()).and_then(parse_percent_token) {
        return pct;
    }
    let num = used.saturating_mul(100).saturating_add(limit / 2);
    (num / limit).min(100) as u8
}

fn parse_panel_line(words: &[&str], prev_last: Option<&str>, next_line: Option<&str>) -> Option<(u64, Option<u8>)> {
    for (idx, word) in words.iter().enumerate() {
        let low = strip_punct(word).to_lowercase();
        if !matches!(low.as_str(), "tokens" | "token" | "токенов" | "токен" | "токена") {
            continue;
        }
        let ratio_tok = if idx > 0 { Some(words[idx - 1]) } else { prev_last };
        if let Some((used, limit)) = ratio_tok.and_then(parse_panel_ratio) {
            return Some((used, Some(resolve_panel_pct(used, limit, &words[idx + 1..], next_line))));
        }
    }
    None
}

pub fn parse_vte_context_panel(visible_text: &str) -> Option<(u64, Option<u8>)> {
    let lines: Vec<&str> = visible_text.lines().map(clean_status_line).filter(|l| !l.is_empty()).collect();
    let mut found = None;
    let mut prev_last = None;
    for (idx, &line) in lines.iter().enumerate() {
        let words = split_vte_words(line);
        if !is_vte_breakdown_line(line) {
            found = parse_panel_line(&words, prev_last, lines.get(idx + 1).copied()).or(found);
        }
        prev_last = words.last().copied();
    }
    found
}
