//! Idempotent, non-destructive AkhTerm hook installers for Claude Code and Codex CLI.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::sys::json_lite::{format_pretty, parse_json, JsonVal};

const AKHTERM_HOOK_MARKER: &str = "notify --pane \"$AKHTERM_PANE_ID\"";
const CODEX_BEGIN: &str = "# BEGIN AKHTERM HOOKS";
const CODEX_END: &str = "# END AKHTERM HOOKS";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookAgent {
    Claude,
    Codex,
    All,
}

impl HookAgent {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallStatus {
    Created,
    Updated,
    Unchanged,
    DryRun,
    ManualRequired,
}

impl InstallStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
            Self::DryRun => "dry-run",
            Self::ManualRequired => "manual",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookInstallOutcome {
    pub agent: &'static str,
    pub path: PathBuf,
    pub status: InstallStatus,
    pub summary: String,
    pub preview: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub agent: HookAgent,
    pub dry_run: bool,
    pub home_dir: PathBuf,
    pub akhterm_bin: String,
}

impl InstallOptions {
    pub fn for_current_env(agent: HookAgent, dry_run: bool) -> Self {
        let home_dir = PathBuf::from(crate::sys::paths::home_dir());
        let akhterm_bin = default_akhterm_bin();
        Self { agent, dry_run, home_dir, akhterm_bin }
    }
}

fn default_akhterm_bin() -> String {
    let path_env = std::env::var_os("PATH");
    if crate::sys::platform::find_in_path("akhterm", path_env.as_deref()).is_some() {
        return "akhterm".to_owned();
    }
    if let Ok(exe) = std::env::current_exe() {
        let raw = exe.to_string_lossy();
        if !raw.contains("/deps/") && !raw.is_empty() {
            return raw.into_owned();
        }
    }
    "akhterm".to_owned()
}

pub fn install_hooks(opts: &InstallOptions) -> Result<Vec<HookInstallOutcome>, String> {
    let mut results = Vec::new();
    if matches!(opts.agent, HookAgent::Claude | HookAgent::All) {
        results.push(install_claude_hooks(opts)?);
    }
    if matches!(opts.agent, HookAgent::Codex | HookAgent::All) {
        results.push(install_codex_hooks(opts)?);
    }
    Ok(results)
}

fn claude_hook_command(bin: &str, state: &str, message: &str, desktop: bool) -> String {
    let flag = if desktop { " --desktop" } else { "" };
    format!(
        "[ -n \"$AKHTERM_PANE_ID\" ] && \"{bin}\" notify --pane \"$AKHTERM_PANE_ID\" --state {state} --title \"Claude Code\" --message \"{message}\"{flag} || true"
    )
}

fn make_claude_matcher(command: String) -> JsonVal {
    let hook = JsonVal::Object(vec![
        ("type".into(), JsonVal::Str("command".into())),
        ("command".into(), JsonVal::Str(command)),
    ]);
    JsonVal::Object(vec![
        ("matcher".into(), JsonVal::Str(String::new())),
        ("hooks".into(), JsonVal::Array(vec![hook])),
    ])
}

fn is_akhterm_hook_cmd(hook: &JsonVal) -> bool {
    hook.get("command")
        .and_then(JsonVal::as_str)
        .is_some_and(|cmd| cmd.contains(AKHTERM_HOOK_MARKER) || cmd.contains("akhterm notify"))
}

fn is_akhterm_matcher(entry: &JsonVal) -> bool {
    entry.get("hooks").and_then(JsonVal::as_array).is_some_and(|hs| hs.iter().any(is_akhterm_hook_cmd))
}

fn upsert_matcher_in_array(arr: &mut Vec<JsonVal>, matcher: JsonVal) {
    if let Some(slot) = arr.iter_mut().find(|item| is_akhterm_matcher(item)) {
        *slot = matcher;
    } else {
        arr.push(matcher);
    }
}

fn upsert_claude_event(hooks_obj: &mut JsonVal, event: &str, command: String) {
    let matcher = make_claude_matcher(command);
    if let Some(arr) = hooks_obj.get_mut(event).and_then(JsonVal::as_array_mut) {
        upsert_matcher_in_array(arr, matcher);
    } else {
        hooks_obj.set_field(event, JsonVal::Array(vec![matcher]));
    }
}

fn parse_claude_root(existing: Option<&str>) -> Result<JsonVal, String> {
    let root = match existing {
        Some(raw) if !raw.trim().is_empty() => parse_json(raw)?,
        _ => JsonVal::Object(Vec::new()),
    };
    if !matches!(root, JsonVal::Object(_)) {
        return Err("Claude settings.json root must be a JSON object".to_owned());
    }
    Ok(root)
}

fn take_hooks_object(root: &JsonVal) -> Result<JsonVal, String> {
    match root.get("hooks") {
        Some(val @ JsonVal::Object(_)) => Ok(val.clone()),
        None => Ok(JsonVal::Object(Vec::new())),
        Some(_) => Err("Claude settings.json 'hooks' must be a JSON object".to_owned()),
    }
}

pub fn merge_claude_settings(existing: Option<&str>, bin: &str) -> Result<String, String> {
    let mut root = parse_claude_root(existing)?;
    let mut hooks = take_hooks_object(&root)?;
    let specs = [
        ("PreToolUse", "working", "Running tool", false),
        ("Notification", "waiting", "Needs attention", true),
        ("Stop", "done", "Turn completed", true),
    ];
    for (event, state, msg, desktop) in specs {
        upsert_claude_event(&mut hooks, event, claude_hook_command(bin, state, msg, desktop));
    }
    root.set_field("hooks", hooks);
    Ok(format!("{}\n", format_pretty(&root)))
}

fn install_claude_hooks(opts: &InstallOptions) -> Result<HookInstallOutcome, String> {
    let path = opts.home_dir.join(".claude").join("settings.json");
    let existing = read_optional_file(&path)?;
    let merged = merge_claude_settings(existing.as_deref(), &opts.akhterm_bin)?;
    finalize_install("claude", path, existing, merged, opts.dry_run, "Claude Code hooks (PreToolUse, Notification, Stop)")
}

pub fn codex_hook_block(bin: &str) -> String {
    let escaped_bin = bin.replace('\\', "\\\\").replace('"', "\\\"");
    let cmd = format!(
        "[ -n \\\"$AKHTERM_PANE_ID\\\" ] && \\\"{escaped_bin}\\\" notify --pane \\\"$AKHTERM_PANE_ID\\\" --state done --title \\\"Codex\\\" --message \\\"Turn completed\\\" --desktop || true"
    );
    format!("{CODEX_BEGIN}\nnotify = [\"sh\", \"-c\", \"{cmd}\"]\n{CODEX_END}")
}

fn has_unmanaged_top_level_notify(content: &str) -> bool {
    let mut in_managed = false;
    for line in content.lines().map(str::trim) {
        if line == CODEX_BEGIN || line == CODEX_END {
            in_managed = line == CODEX_BEGIN;
            continue;
        }
        if line.starts_with('[') && !in_managed {
            return false;
        }
        if !in_managed && !line.starts_with('#') && is_notify_assignment(line) {
            return true;
        }
    }
    false
}

fn is_notify_assignment(trimmed: &str) -> bool {
    trimmed
        .strip_prefix("notify")
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

fn replace_managed_block(content: &str, block: &str) -> Option<String> {
    let start = content.find(CODEX_BEGIN)?;
    let end_marker = content[start..].find(CODEX_END)?;
    let end = start + end_marker + CODEX_END.len();
    let mut out = String::with_capacity(content.len() + block.len());
    out.push_str(&content[..start]);
    out.push_str(block);
    out.push_str(&content[end..]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

fn find_first_table_offset(content: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        if line.trim_start().starts_with('[') {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn insert_before_first_table(content: &str, block: &str) -> String {
    if let Some(offset) = find_first_table_offset(content) {
        let head = content[..offset].trim_end_matches('\n');
        let tail = &content[offset..];
        return if head.is_empty() { format!("{block}\n\n{tail}") } else { format!("{head}\n\n{block}\n\n{tail}") };
    }
    let trimmed = content.trim_end_matches('\n');
    if trimmed.is_empty() { format!("{block}\n") } else { format!("{trimmed}\n\n{block}\n") }
}

pub enum CodexMergeResult {
    Merged(String),
    ManualConflict { snippet: String },
}

pub fn merge_codex_config(existing: Option<&str>, bin: &str) -> CodexMergeResult {
    let block = codex_hook_block(bin);
    let raw = existing.unwrap_or("");
    if let Some(replaced) = replace_managed_block(raw, &block) {
        return CodexMergeResult::Merged(replaced);
    }
    if has_unmanaged_top_level_notify(raw) {
        return CodexMergeResult::ManualConflict { snippet: block };
    }
    CodexMergeResult::Merged(insert_before_first_table(raw, &block))
}

fn install_codex_hooks(opts: &InstallOptions) -> Result<HookInstallOutcome, String> {
    let path = opts.home_dir.join(".codex").join("config.toml");
    let existing = read_optional_file(&path)?;
    match merge_codex_config(existing.as_deref(), &opts.akhterm_bin) {
        CodexMergeResult::Merged(merged) => finalize_install(
            "codex", path, existing, merged, opts.dry_run, "Codex CLI notify hook",
        ),
        CodexMergeResult::ManualConflict { snippet } => Ok(HookInstallOutcome {
            agent: "codex",
            path,
            status: InstallStatus::ManualRequired,
            summary: "Existing custom 'notify' found in Codex config.toml; left untouched. Combine manually with the snippet below.".to_owned(),
            preview: Some(snippet),
        }),
    }
}

fn read_optional_file(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))
}

fn stage_atomic_write(tmp: &Path, path: &Path, content: &str) -> std::io::Result<()> {
    let mut f = OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(tmp)?;
    f.write_all(content.as_bytes())?;
    fs::rename(tmp, path)
}

fn write_file_atomic(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("akhterm-{}.tmp", std::process::id()));
    let res = stage_atomic_write(&tmp, path, content);
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn dry_run_outcome(agent: &'static str, path: PathBuf, existing: &Option<String>, merged: String, desc: &str) -> HookInstallOutcome {
    let state = if existing.as_deref() == Some(merged.as_str()) { "already up to date" } else if existing.is_some() { "would update" } else { "would create" };
    HookInstallOutcome { agent, path, status: InstallStatus::DryRun, summary: format!("{desc}: {state}"), preview: Some(merged) }
}

fn finalize_install(
    agent: &'static str,
    path: PathBuf,
    existing: Option<String>,
    merged: String,
    dry_run: bool,
    desc: &str,
) -> Result<HookInstallOutcome, String> {
    if dry_run {
        return Ok(dry_run_outcome(agent, path, &existing, merged, desc));
    }
    if existing.as_deref() == Some(merged.as_str()) {
        return Ok(HookInstallOutcome {
            agent, path, status: InstallStatus::Unchanged, summary: format!("{desc}: already installed"), preview: None,
        });
    }
    write_file_atomic(&path, &merged)?;
    let status = if existing.is_some() { InstallStatus::Updated } else { InstallStatus::Created };
    Ok(HookInstallOutcome { agent, path, status, summary: format!("{desc}: {}", status.as_str()), preview: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("akhterm-hooks-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp home");
        dir
    }

    fn write_sample_claude_settings(home: &Path) -> PathBuf {
        let path = home.join(".claude").join("settings.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let initial = "{\"model\":\"opus\",\"permissions\":{\"allow\":[\"Bash(git status)\"]},\"hooks\":{\"PostToolUse\":[{\"matcher\":\"Edit\",\"hooks\":[{\"type\":\"command\",\"command\":\"echo ok\"}]}],\"Stop\":[{\"matcher\":\"\",\"hooks\":[{\"type\":\"command\",\"command\":\"echo custom-stop\"}]}]}}";
        fs::write(&path, initial).unwrap();
        path
    }

    fn hook_len(parsed: &JsonVal, event: &str) -> usize {
        parsed.get("hooks").unwrap().get(event).and_then(JsonVal::as_array).unwrap().len()
    }

    #[test]
    fn merges_claude_settings_idempotently_and_preserves_unrelated_keys() {
        let home = temp_home("claude");
        let path = write_sample_claude_settings(&home);
        let opts = InstallOptions { agent: HookAgent::Claude, dry_run: false, home_dir: home.clone(), akhterm_bin: "akhterm".into() };
        assert_eq!(install_hooks(&opts).unwrap()[0].status, InstallStatus::Updated);
        let parsed = parse_json(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.get("model").and_then(JsonVal::as_str), Some("opus"));
        assert!(parsed.get("permissions").is_some() && hook_len(&parsed, "PostToolUse") == 1);
        assert_eq!((hook_len(&parsed, "Stop"), hook_len(&parsed, "PreToolUse"), hook_len(&parsed, "Notification")), (2, 1, 1));
        assert_eq!(install_hooks(&opts).unwrap()[0].status, InstallStatus::Unchanged);
        let _ = fs::remove_dir_all(&home);
    }

    fn verify_codex_conflict(path: &Path, real: &InstallOptions) {
        fs::write(path, "notify = [\"my-custom-hook\"]\n").unwrap();
        let conflict = install_hooks(real).unwrap();
        assert_eq!(conflict[0].status, InstallStatus::ManualRequired);
        assert_eq!(fs::read_to_string(path).unwrap(), "notify = [\"my-custom-hook\"]\n");
    }

    #[test]
    fn merges_codex_toml_before_tables_and_respects_custom_notify_and_dry_run() {
        let home = temp_home("codex");
        let path = home.join(".codex").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let existing = "model = \"gpt-5.4\"\n\n[mcp_servers.docs]\ncommand = \"npx\"\n";
        fs::write(&path, existing).unwrap();
        let dry = InstallOptions { agent: HookAgent::Codex, dry_run: true, home_dir: home.clone(), akhterm_bin: "akhterm".into() };
        assert_eq!(install_hooks(&dry).unwrap()[0].status, InstallStatus::DryRun);
        assert_eq!(fs::read_to_string(&path).unwrap(), existing);
        let real = InstallOptions { dry_run: false, ..dry };
        assert_eq!(install_hooks(&real).unwrap()[0].status, InstallStatus::Updated);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.find("notify = [").unwrap() < body.find("[mcp_servers.docs]").unwrap());
        assert_eq!(install_hooks(&real).unwrap()[0].status, InstallStatus::Unchanged);
        verify_codex_conflict(&path, &real);
        let _ = fs::remove_dir_all(&home);
    }
}
