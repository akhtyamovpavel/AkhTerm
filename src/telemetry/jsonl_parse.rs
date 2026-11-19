//! Zero-dependency JSONL scanner and Claude/Codex/AGY token snapshot extraction.

use super::{TelemetrySource, TokenSnapshot};

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

pub(crate) fn is_complete_json_object(line: &str) -> bool {
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

pub(crate) fn json_direct_value<'a>(obj: &'a str, target_key: &str) -> Option<&'a str> {
    let trimmed = obj.trim();
    if trimmed.as_bytes().first() != Some(&b'{') {
        return None;
    }
    scan_direct_value(trimmed, target_key)
}

fn scan_direct_value<'a>(trimmed: &'a str, target_key: &str) -> Option<&'a str> {
    let bytes = trimmed.as_bytes();
    let mut idx = 1;
    while idx < bytes.len() {
        idx = skip_ws(bytes, idx);
        match bytes.get(idx) {
            None | Some(b'}') => return None,
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

pub(crate) fn json_str(obj: &str, key: &str) -> Option<String> {
    let val = json_direct_value(obj, key)?;
    let inner = val.strip_prefix('"')?.strip_suffix('"')?;
    Some(unescape_json_str(inner))
}

pub(crate) fn json_u64(obj: &str, key: &str) -> Option<u64> {
    let val = json_direct_value(obj, key)?;
    let digits: String = val.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| digits.parse::<u64>().ok()).flatten()
}

pub(crate) fn json_obj<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    json_direct_value(obj, key).filter(|v| v.starts_with('{'))
}

pub(crate) fn json_obj_prefix<'a>(obj: &'a str, target_key: &str) -> Option<&'a str> {
    if let Some(full) = json_obj(obj, target_key) {
        return Some(full);
    }
    let trimmed = obj.trim();
    (trimmed.as_bytes().first() == Some(&b'{'))
        .then(|| scan_obj_prefix(trimmed, target_key))
        .flatten()
}

fn scan_obj_prefix<'a>(trimmed: &'a str, target_key: &str) -> Option<&'a str> {
    let bytes = trimmed.as_bytes();
    let mut idx = 1;
    while idx < bytes.len() {
        idx = skip_ws(bytes, idx);
        match bytes.get(idx) {
            None | Some(b'}') => return None,
            Some(b',') => idx += 1,
            _ => {
                let (key, val_start) = parse_prefix_key(trimmed, idx)?;
                if key == target_key {
                    return (bytes.get(val_start) == Some(&b'{')).then_some(&trimmed[val_start..]);
                }
                idx = skip_json_value(bytes, val_start)?;
            }
        }
    }
    None
}

fn parse_prefix_key(trimmed: &str, idx: usize) -> Option<(&str, usize)> {
    let bytes = trimmed.as_bytes();
    let key_end = skip_json_string(bytes, idx)?;
    let colon = skip_ws(bytes, key_end);
    (bytes.get(colon) == Some(&b':'))
        .then(|| (&trimmed[idx + 1..key_end - 1], skip_ws(bytes, colon + 1)))
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
        TelemetrySource::OpenCode => crate::telemetry::opencode::apply_usage(line, snap),
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
        Some("PLANNER_RESPONSE") => apply_agy_planner(line, snap, in_tok, cache_tok, out_tok),
        Some("CHECKPOINT") => apply_agy_checkpoint(snap, in_tok, cache_tok, out_tok),
        _ => {}
    }
}

fn apply_agy_planner(line: &str, snap: &mut TokenSnapshot, in_tok: u64, cache_tok: u64, out_tok: u64) {
    if !accumulate_agy_usage(snap, in_tok, cache_tok, out_tok) {
        return;
    }
    let prompt = in_tok.saturating_add(cache_tok);
    snap.requests.record(json_u64(line, "cache_read_tokens"), prompt, None);
    if prompt > 0 {
        snap.context_tokens = prompt.saturating_add(out_tok);
    }
}

fn apply_agy_checkpoint(snap: &mut TokenSnapshot, in_tok: u64, cache_tok: u64, out_tok: u64) {
    let _ = accumulate_agy_usage(snap, in_tok, cache_tok, out_tok);
    if out_tok > 0 {
        snap.context_tokens = out_tok;
    }
    snap.context_pct = None;
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
    let repeated = !msg_id.is_empty() && msg_id == *last_msg_id;
    apply_claude_turn_state(snap, msg_id, cur, repeated, last_msg_id, last_msg_usage);
    let read = claude_cache_read_present(line).then_some(last_msg_usage.2);
    snap.requests.claude_update(read, snap.context_tokens, repeated);
}

fn apply_claude_turn_state(
    snap: &mut TokenSnapshot,
    msg_id: String,
    cur: (u64, u64, u64, u64),
    repeated: bool,
    last_msg_id: &mut String,
    last_msg_usage: &mut (u64, u64, u64, u64),
) {
    if repeated {
        replace_claude_turn(snap, *last_msg_usage, cur);
        *last_msg_usage = max_tuple4(*last_msg_usage, cur);
    } else {
        add_claude_turn(snap, cur);
        *last_msg_id = msg_id;
        *last_msg_usage = cur;
    }
}

fn claude_cache_read_present(line: &str) -> bool {
    let usage = json_obj(line, "message")
        .and_then(|m| json_obj(m, "usage"))
        .or_else(|| json_obj(line, "usage"));
    usage.and_then(|u| json_u64(u, "cache_read_input_tokens")).is_some()
}

fn max_tuple4(a: (u64, u64, u64, u64), b: (u64, u64, u64, u64)) -> (u64, u64, u64, u64) {
    (a.0.max(b.0), a.1.max(b.1), a.2.max(b.2), a.3.max(b.3))
}

fn set_claude_context(snap: &mut TokenSnapshot, in_t: u64, cw_t: u64, cr_t: u64) {
    let ctx = in_t.saturating_add(cw_t).saturating_add(cr_t);
    if ctx > 0 {
        snap.context_tokens = ctx;
    }
}

fn add_claude_turn(snap: &mut TokenSnapshot, (in_t, cw_t, cr_t, out_t): (u64, u64, u64, u64)) {
    snap.input_tokens = snap.input_tokens.saturating_add(in_t);
    snap.cache_write_tokens = snap.cache_write_tokens.saturating_add(cw_t);
    snap.cache_tokens = snap.cache_tokens.saturating_add(cr_t);
    snap.output_tokens = snap.output_tokens.saturating_add(out_t);
    set_claude_context(snap, in_t, cw_t, cr_t);
}

fn replace_claude_turn(
    snap: &mut TokenSnapshot,
    prev: (u64, u64, u64, u64),
    next: (u64, u64, u64, u64),
) {
    let m = max_tuple4(prev, next);
    snap.input_tokens = snap.input_tokens.saturating_sub(prev.0).saturating_add(m.0);
    snap.cache_write_tokens = snap.cache_write_tokens.saturating_sub(prev.1).saturating_add(m.1);
    snap.cache_tokens = snap.cache_tokens.saturating_sub(prev.2).saturating_add(m.2);
    snap.output_tokens = snap.output_tokens.saturating_sub(prev.3).saturating_add(m.3);
    set_claude_context(snap, m.0, m.1, m.2);
}

struct CodexUsage {
    uncached_in: u64,
    cached_in: u64,
    cache_read: Option<u64>,
    output: u64,
    context: u64,
}

fn parse_codex_usage_obj(obj: &str) -> Option<CodexUsage> {
    let in_raw = json_u64(obj, "input_tokens").unwrap_or(0);
    let cache_read = json_u64(obj, "cached_input_tokens");
    let cached = cache_read.unwrap_or(0);
    let output = json_u64(obj, "output_tokens").unwrap_or(0);
    let total = json_u64(obj, "total_tokens");
    if in_raw.saturating_add(cached).saturating_add(output).saturating_add(total.unwrap_or(0)) == 0 {
        return None;
    }
    let (uncached_in, context) = split_codex_input(in_raw, cached, output, total);
    Some(CodexUsage { uncached_in, cached_in: cached, cache_read, output, context })
}

fn split_codex_input(in_raw: u64, cached: u64, output: u64, total: Option<u64>) -> (u64, u64) {
    if is_codex_cached_disjoint(in_raw, cached, output, total) {
        (in_raw, in_raw.saturating_add(cached))
    } else {
        (in_raw.saturating_sub(cached), in_raw)
    }
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
    if let Some(last) = json_obj(info, "last_token_usage").and_then(parse_codex_usage_obj) {
        snap.context_tokens = last.context;
        snap.requests.record(last.cache_read, last.context, Some(tot.context.saturating_add(tot.output)));
    }
    if let Some(win) = json_u64(info, "model_context_window").filter(|w| *w > 0) {
        snap.context_pct = (snap.context_tokens > 0)
            .then(|| ((snap.context_tokens.saturating_mul(100)) / win).min(100) as u8);
    }
}

fn apply_codex_step(step: CodexUsage, snap: &mut TokenSnapshot) {
    snap.requests.record(step.cache_read, step.context, None);
    snap.source = TelemetrySource::Codex;
    snap.input_tokens = snap.input_tokens.saturating_add(step.uncached_in);
    snap.cache_tokens = snap.cache_tokens.saturating_add(step.cached_in);
    snap.output_tokens = snap.output_tokens.saturating_add(step.output);
    if step.context > 0 {
        snap.context_tokens = step.context;
    }
}
