//! 1-click official agent hook installer for Claude Code, Codex CLI, and AGY CLI.

use std::fs::{self, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::sys::agent_hooks::{
    codex_hook_block, merge_claude_settings, merge_codex_config, CodexMergeResult,
};
use crate::sys::json_lite::{format_pretty, parse_json, JsonVal};

const AKHTERM_CLAUDE_MARKER: &str = "notify --pane \"$AKHTERM_PANE_ID\"";
const CODEX_BEGIN: &str = "# BEGIN AKHTERM HOOKS";
const CODEX_END: &str = "# END AKHTERM HOOKS";
const AGY_SCRIPT_MARKER: &str = "# AKHTERM-MANAGED-HOOK";
const AGY_MANAGED_BY: &str = "akhterm";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentHookTarget {
    ClaudeCode,
    CodexCli,
    AgyCli,
}

impl AgentHookTarget {
    pub const ALL: [Self; 3] = [Self::ClaudeCode, Self::CodexCli, Self::AgyCli];

    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::CodexCli => "Codex CLI",
            Self::AgyCli => "AGY CLI",
        }
    }

    pub fn config_path(self, home: &Path) -> PathBuf {
        match self {
            Self::ClaudeCode => home.join(".claude").join("settings.json"),
            Self::CodexCli => home.join(".codex").join("config.toml"),
            Self::AgyCli => home.join(".gemini").join("antigravity").join("hooks.json"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookInstallStatus {
    Installed,
    NotInstalled,
    Outdated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookActionReport {
    pub target: AgentHookTarget,
    pub status: HookInstallStatus,
    pub path: PathBuf,
    pub changed: bool,
    pub detail: String,
}

fn report(
    target: AgentHookTarget,
    status: HookInstallStatus,
    path: PathBuf,
    changed: bool,
    detail: impl Into<String>,
) -> HookActionReport {
    HookActionReport { target, status, path, changed, detail: detail.into() }
}

pub fn agy_helper_script_path(home: &Path) -> PathBuf {
    home.join(".local").join("share").join("akhterm").join("hooks").join("akhterm-agent-hook.sh")
}

pub fn default_akhterm_bin() -> String {
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

#[allow(dead_code)]
pub fn inspect_status(home: &Path, target: AgentHookTarget) -> HookInstallStatus {
    let bin = default_akhterm_bin();
    inspect_status_with_bin(home, target, &bin)
}

pub fn inspect_status_with_bin(home: &Path, target: AgentHookTarget, bin: &str) -> HookInstallStatus {
    match target {
        AgentHookTarget::ClaudeCode => inspect_claude(home, bin),
        AgentHookTarget::CodexCli => inspect_codex(home, bin),
        AgentHookTarget::AgyCli => inspect_agy(home, bin),
    }
}

pub fn inspect_all(home: &Path) -> [(AgentHookTarget, HookInstallStatus); 3] {
    let bin = default_akhterm_bin();
    [
        (AgentHookTarget::ClaudeCode, inspect_claude(home, &bin)),
        (AgentHookTarget::CodexCli, inspect_codex(home, &bin)),
        (AgentHookTarget::AgyCli, inspect_agy(home, &bin)),
    ]
}

pub fn install_hooks(home: &Path, target: AgentHookTarget) -> Result<HookActionReport, String> {
    let bin = default_akhterm_bin();
    install_hooks_with_bin(home, target, &bin)
}

pub fn install_hooks_with_bin(
    home: &Path,
    target: AgentHookTarget,
    bin: &str,
) -> Result<HookActionReport, String> {
    match target {
        AgentHookTarget::ClaudeCode => install_claude(home, bin),
        AgentHookTarget::CodexCli => install_codex(home, bin),
        AgentHookTarget::AgyCli => install_agy(home, bin),
    }
}

pub fn install_all_hooks(home: &Path) -> Result<Vec<HookActionReport>, String> {
    let bin = default_akhterm_bin();
    let (mut reports, mut errors) = (Vec::with_capacity(3), Vec::new());
    for target in AgentHookTarget::ALL {
        match install_hooks_with_bin(home, target, &bin) {
            Ok(rep) => reports.push(rep),
            Err(err) => errors.push(format!("{}: {err}", target.label())),
        }
    }
    if errors.is_empty() { Ok(reports) } else { Err(errors.join("; ")) }
}

pub fn uninstall_hooks(home: &Path, target: AgentHookTarget) -> Result<HookActionReport, String> {
    match target {
        AgentHookTarget::ClaudeCode => uninstall_claude(home),
        AgentHookTarget::CodexCli => uninstall_codex(home),
        AgentHookTarget::AgyCli => uninstall_agy(home),
    }
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))
}

fn stage_write_mode(tmp: &Path, path: &Path, content: &str, mode: u32) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).truncate(true).write(true).mode(mode).open(tmp)?;
    file.write_all(content.as_bytes())?;
    file.flush()?;
    fs::set_permissions(tmp, Permissions::from_mode(mode))?;
    fs::rename(tmp, path)
}

fn write_atomic_mode(path: &Path, content: &str, mode: u32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("hook");
    let tmp = path.with_file_name(format!(".{name}.akhterm-{}-{seq}.tmp", std::process::id()));
    let res = stage_write_mode(&tmp, path, content, mode);
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn claude_specs(bin: &str) -> [(&'static str, String); 3] {
    let safe_bin = escape_sh_dq(bin);
    let mk = |state: &str, msg: &str, desktop: bool| {
        let flag = if desktop { " --desktop" } else { "" };
        format!(
            "[ -n \"$AKHTERM_PANE_ID\" ] && \"{safe_bin}\" notify --pane \"$AKHTERM_PANE_ID\" --state {state} --title \"Claude Code\" --message \"{msg}\"{flag} || true"
        )
    };
    [
        ("PreToolUse", mk("working", "Running tool", false)),
        ("Notification", mk("waiting", "Needs attention", true)),
        ("Stop", mk("done", "Turn completed", true)),
    ]
}

fn is_claude_akhterm_hook(hook: &JsonVal) -> bool {
    hook.get("command")
        .and_then(JsonVal::as_str)
        .is_some_and(|cmd| cmd.contains(AKHTERM_CLAUDE_MARKER) || cmd.contains("akhterm notify"))
}

fn matcher_has_exact_cmd(entry: &JsonVal, expected: &str) -> bool {
    entry
        .get("hooks")
        .and_then(JsonVal::as_array)
        .is_some_and(|hs| hs.iter().any(|h| h.get("command").and_then(JsonVal::as_str) == Some(expected)))
}

fn event_has_managed(hooks: &JsonVal, event: &str, pred: fn(&JsonVal) -> bool) -> bool {
    hooks
        .get(event)
        .and_then(JsonVal::as_array)
        .is_some_and(|arr| {
            arr.iter().any(|m| {
                m.get("hooks").and_then(JsonVal::as_array).is_some_and(|hs| hs.iter().any(pred))
            })
        })
}

fn event_has_exact(hooks: &JsonVal, event: &str, expected: &str) -> bool {
    hooks
        .get(event)
        .and_then(JsonVal::as_array)
        .is_some_and(|arr| arr.iter().any(|m| matcher_has_exact_cmd(m, expected)))
}

fn inspect_json_hooks(
    hooks: &JsonVal,
    specs: &[(&str, String); 3],
    pred: fn(&JsonVal) -> bool,
) -> HookInstallStatus {
    let managed = specs.iter().filter(|(ev, _)| event_has_managed(hooks, ev, pred)).count();
    let current = specs.iter().filter(|(ev, cmd)| event_has_exact(hooks, ev, cmd)).count();
    if current == specs.len() {
        HookInstallStatus::Installed
    } else if managed > 0 {
        HookInstallStatus::Outdated
    } else {
        HookInstallStatus::NotInstalled
    }
}

fn inspect_claude(home: &Path, bin: &str) -> HookInstallStatus {
    let path = AgentHookTarget::ClaudeCode.config_path(home);
    let Some(root) = read_optional(&path).ok().flatten().and_then(|s| parse_json(&s).ok()) else {
        return HookInstallStatus::NotInstalled;
    };
    root.get("hooks")
        .map_or(HookInstallStatus::NotInstalled, |h| inspect_json_hooks(h, &claude_specs(bin), is_claude_akhterm_hook))
}

fn install_claude(home: &Path, bin: &str) -> Result<HookActionReport, String> {
    let path = AgentHookTarget::ClaudeCode.config_path(home);
    let existing = read_optional(&path)?;
    let safe_bin = escape_sh_dq(bin);
    let merged = merge_claude_settings(existing.as_deref(), &safe_bin)?;
    let changed = existing.as_deref() != Some(merged.as_str());
    if changed {
        write_atomic_mode(&path, &merged, 0o600)?;
    }
    let detail = if changed { "Claude Code hooks installed" } else { "Claude Code hooks already up to date" };
    Ok(report(AgentHookTarget::ClaudeCode, HookInstallStatus::Installed, path, changed, detail))
}

fn prune_matcher_hooks(matcher: &mut JsonVal, pred: fn(&JsonVal) -> bool) -> (bool, bool) {
    let Some(arr) = matcher.get_mut("hooks").and_then(JsonVal::as_array_mut) else {
        return (false, false);
    };
    let before = arr.len();
    arr.retain(|h| !pred(h));
    let removed = arr.len() < before;
    (removed, removed && arr.is_empty())
}

fn prune_event_array(val: &mut JsonVal, pred: fn(&JsonVal) -> bool) -> (bool, bool) {
    let Some(matchers) = val.as_array_mut() else {
        return (false, false);
    };
    let mut removed_any = false;
    let mut kept = Vec::with_capacity(matchers.len());
    for mut item in matchers.drain(..) {
        let (removed, drop_matcher) = prune_matcher_hooks(&mut item, pred);
        removed_any |= removed;
        if !drop_matcher {
            kept.push(item);
        }
    }
    *matchers = kept;
    (removed_any, removed_any && matchers.is_empty())
}

fn prune_hooks_events(val: &mut JsonVal, pred: fn(&JsonVal) -> bool) -> (bool, bool) {
    let JsonVal::Object(events) = val else {
        return (false, false);
    };
    let mut removed_any = false;
    let mut next = Vec::with_capacity(events.len());
    for (name, mut ev) in events.drain(..) {
        let (removed, drop_ev) = prune_event_array(&mut ev, pred);
        removed_any |= removed;
        if !drop_ev {
            next.push((name, ev));
        }
    }
    *events = next;
    (removed_any, removed_any && events.is_empty())
}

fn prune_hooks_in_root(root: &mut JsonVal, pred: fn(&JsonVal) -> bool) -> bool {
    let JsonVal::Object(pairs) = root else {
        return false;
    };
    let Some(idx) = pairs.iter().position(|(k, _)| k == "hooks") else {
        return false;
    };
    let (removed, drop_hooks) = prune_hooks_events(&mut pairs[idx].1, pred);
    if drop_hooks {
        pairs.remove(idx);
    }
    removed
}

fn uninstall_json_target(
    path: PathBuf,
    target: AgentHookTarget,
    pred: fn(&JsonVal) -> bool,
    desc: &str,
) -> Result<HookActionReport, String> {
    let Some(raw) = read_optional(&path)? else {
        return Ok(report(target, HookInstallStatus::NotInstalled, path, false, format!("{desc}: nothing to remove")));
    };
    let mut root = parse_json(&raw)?;
    let changed = prune_hooks_in_root(&mut root, pred);
    if changed {
        write_atomic_mode(&path, &format!("{}\n", format_pretty(&root)), 0o600)?;
    }
    let detail = if changed { format!("{desc} removed") } else { format!("{desc}: nothing to remove") };
    Ok(report(target, HookInstallStatus::NotInstalled, path, changed, detail))
}

fn uninstall_claude(home: &Path) -> Result<HookActionReport, String> {
    let path = AgentHookTarget::ClaudeCode.config_path(home);
    uninstall_json_target(path, AgentHookTarget::ClaudeCode, is_claude_akhterm_hook, "Claude Code hooks")
}

fn extract_codex_managed_block(raw: &str) -> Option<&str> {
    let start = raw.find(CODEX_BEGIN)?;
    let end_rel = raw[start..].find(CODEX_END)?;
    let end = start + end_rel + CODEX_END.len();
    Some(&raw[start..end])
}

fn inspect_codex(home: &Path, bin: &str) -> HookInstallStatus {
    let path = AgentHookTarget::CodexCli.config_path(home);
    let Ok(Some(raw)) = read_optional(&path) else {
        return HookInstallStatus::NotInstalled;
    };
    if let Some(block) = extract_codex_managed_block(&raw) {
        return if block == codex_hook_block(bin) {
            HookInstallStatus::Installed
        } else {
            HookInstallStatus::Outdated
        };
    }
    if raw.contains(CODEX_BEGIN) || raw.contains("akhterm notify") || raw.contains("notify --pane") {
        HookInstallStatus::Outdated
    } else {
        HookInstallStatus::NotInstalled
    }
}

fn install_codex(home: &Path, bin: &str) -> Result<HookActionReport, String> {
    let path = AgentHookTarget::CodexCli.config_path(home);
    let existing = read_optional(&path)?;
    let CodexMergeResult::Merged(merged) = merge_codex_config(existing.as_deref(), bin) else {
        return Err("Existing custom 'notify' found in Codex config.toml; left untouched".to_owned());
    };
    let changed = existing.as_deref() != Some(merged.as_str());
    if changed {
        write_atomic_mode(&path, &merged, 0o600)?;
    }
    let detail = if changed { "Codex CLI hook installed" } else { "Codex CLI hook already up to date" };
    Ok(report(AgentHookTarget::CodexCli, HookInstallStatus::Installed, path, changed, detail))
}

fn join_codex_parts(head: &str, tail: &str) -> String {
    let mut out = match (head.is_empty(), tail.is_empty()) {
        (true, true) => String::new(),
        (true, false) => tail.to_owned(),
        (false, true) => format!("{head}\n"),
        (false, false) => format!("{head}\n\n{tail}"),
    };
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn strip_codex_managed_block(raw: &str) -> Option<String> {
    let start = raw.find(CODEX_BEGIN)?;
    let end_rel = raw[start..].find(CODEX_END)?;
    let end = start + end_rel + CODEX_END.len();
    Some(join_codex_parts(raw[..start].trim_end_matches('\n'), raw[end..].trim_start_matches('\n')))
}

fn uninstall_codex(home: &Path) -> Result<HookActionReport, String> {
    let path = AgentHookTarget::CodexCli.config_path(home);
    let Some(raw) = read_optional(&path)? else {
        return Ok(report(AgentHookTarget::CodexCli, HookInstallStatus::NotInstalled, path, false, "Codex CLI hook: nothing to remove"));
    };
    let Some(stripped) = strip_codex_managed_block(&raw) else {
        let status = inspect_codex(home, &default_akhterm_bin());
        return Ok(report(AgentHookTarget::CodexCli, status, path, false, "Codex CLI hook: no managed block found"));
    };
    write_atomic_mode(&path, &stripped, 0o600)?;
    Ok(report(AgentHookTarget::CodexCli, HookInstallStatus::NotInstalled, path, true, "Codex CLI hook removed"))
}

fn escape_sh_dq(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '"' | '$' | '`') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

const AGY_SCRIPT_BODY: &str = concat!(
    "#!/bin/sh\n",
    "# AKHTERM-MANAGED-HOOK v1\n",
    "# Official AkhTerm agent hook bridge for AGY CLI.\n",
    "[ -n \"${AKHTERM_PANE_ID:-}\" ] || exit 0\n",
    "EVENT=\"${1:-done}\"\n",
    "TITLE=\"${2:-AGY}\"\n",
    "MSG=\"${3:-}\"\n",
    "case \"$EVENT\" in\n",
    "  working|PreToolUse|start) STATE=\"working\"; DEF_MSG=\"Running tool\"; DESKTOP=\"\" ;;\n",
    "  waiting|Notification|input) STATE=\"waiting\"; DEF_MSG=\"Needs attention\"; DESKTOP=\"--desktop\" ;;\n",
    "  error|fail) STATE=\"error\"; DEF_MSG=\"Action failed\"; DESKTOP=\"--desktop\" ;;\n",
    "  *) STATE=\"done\"; DEF_MSG=\"Turn completed\"; DESKTOP=\"--desktop\" ;;\n",
    "esac\n",
    "[ -n \"$MSG\" ] || MSG=\"$DEF_MSG\"\n",
);

pub fn agy_script_content(bin: &str) -> String {
    let safe = escape_sh_dq(bin);
    format!(
        "{AGY_SCRIPT_BODY}if [ -n \"${{AKHTERM_SOCKET:-}}\" ]; then\n  \"{safe}\" notify --socket \"$AKHTERM_SOCKET\" --pane \"$AKHTERM_PANE_ID\" --state \"$STATE\" --title \"$TITLE\" --message \"$MSG\" $DESKTOP || true\nelse\n  \"{safe}\" notify --pane \"$AKHTERM_PANE_ID\" --state \"$STATE\" --title \"$TITLE\" --message \"$MSG\" $DESKTOP || true\nfi\n"
    )
}

fn agy_specs(script_path: &Path) -> [(&'static str, String); 3] {
    let safe = escape_sh_dq(&script_path.to_string_lossy());
    [
        ("PreToolUse", format!("\"{safe}\" working \"AGY\"")),
        ("Notification", format!("\"{safe}\" waiting \"AGY\"")),
        ("Stop", format!("\"{safe}\" done \"AGY\"")),
    ]
}

fn is_agy_akhterm_hook(hook: &JsonVal) -> bool {
    let by_tag = hook.get("managedBy").and_then(JsonVal::as_str) == Some(AGY_MANAGED_BY);
    let by_cmd = hook
        .get("command")
        .and_then(JsonVal::as_str)
        .is_some_and(|c| c.contains("akhterm-agent-hook.sh"));
    by_tag || by_cmd
}

fn make_agy_matcher(command: String) -> JsonVal {
    let hook = JsonVal::Object(vec![
        ("type".into(), JsonVal::Str("command".into())),
        ("command".into(), JsonVal::Str(command)),
        ("managedBy".into(), JsonVal::Str(AGY_MANAGED_BY.into())),
    ]);
    JsonVal::Object(vec![
        ("matcher".into(), JsonVal::Str(String::new())),
        ("hooks".into(), JsonVal::Array(vec![hook])),
    ])
}

fn upsert_agy_event(hooks: &mut JsonVal, event: &str, command: String) {
    let matcher = make_agy_matcher(command);
    if let Some(arr) = hooks.get_mut(event).and_then(JsonVal::as_array_mut) {
        let pos = arr.iter().position(|item| {
            item.get("hooks").and_then(JsonVal::as_array).is_some_and(|hs| hs.iter().any(is_agy_akhterm_hook))
        });
        if let Some(idx) = pos {
            arr[idx] = matcher;
        } else {
            arr.push(matcher);
        }
    } else {
        hooks.set_field(event, JsonVal::Array(vec![matcher]));
    }
}

fn parse_json_object(existing: Option<&str>, err_msg: &str) -> Result<JsonVal, String> {
    let root = match existing {
        Some(raw) if !raw.trim().is_empty() => parse_json(raw)?,
        _ => JsonVal::Object(Vec::new()),
    };
    if matches!(root, JsonVal::Object(_)) { Ok(root) } else { Err(err_msg.to_owned()) }
}

fn take_json_hooks(root: &JsonVal) -> Result<JsonVal, String> {
    match root.get("hooks") {
        Some(val @ JsonVal::Object(_)) => Ok(val.clone()),
        None => Ok(JsonVal::Object(Vec::new())),
        Some(_) => Err("AGY hooks.json 'hooks' must be a JSON object".to_owned()),
    }
}

fn merge_agy_hooks_json(existing: Option<&str>, script_path: &Path) -> Result<String, String> {
    let mut root = parse_json_object(existing, "AGY hooks.json root must be a JSON object")?;
    let mut hooks = take_json_hooks(&root)?;
    for (event, cmd) in agy_specs(script_path) {
        upsert_agy_event(&mut hooks, event, cmd);
    }
    root.set_field("hooks", hooks);
    Ok(format!("{}\n", format_pretty(&root)))
}

fn script_is_executable(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.permissions().mode() & 0o100 != 0).unwrap_or(false)
}

fn inspect_agy_config(home: &Path, script_path: &Path) -> HookInstallStatus {
    let cfg_path = AgentHookTarget::AgyCli.config_path(home);
    let Some(root) = read_optional(&cfg_path).ok().flatten().and_then(|s| parse_json(&s).ok()) else {
        return HookInstallStatus::NotInstalled;
    };
    root.get("hooks")
        .map_or(HookInstallStatus::NotInstalled, |h| inspect_json_hooks(h, &agy_specs(script_path), is_agy_akhterm_hook))
}

fn inspect_agy(home: &Path, bin: &str) -> HookInstallStatus {
    let script_path = agy_helper_script_path(home);
    let script_raw = read_optional(&script_path).ok().flatten();
    let cfg_status = inspect_agy_config(home, &script_path);
    if script_raw.is_none() && cfg_status == HookInstallStatus::NotInstalled {
        return HookInstallStatus::NotInstalled;
    }
    let script_ok = script_raw.as_deref() == Some(agy_script_content(bin).as_str())
        && script_is_executable(&script_path);
    if script_ok && cfg_status == HookInstallStatus::Installed {
        HookInstallStatus::Installed
    } else {
        HookInstallStatus::Outdated
    }
}

fn sync_agy_script(script_path: &Path, bin: &str) -> Result<bool, String> {
    let expected = agy_script_content(bin);
    let existing = read_optional(script_path)?;
    let changed = existing.as_deref() != Some(expected.as_str()) || !script_is_executable(script_path);
    if changed {
        write_atomic_mode(script_path, &expected, 0o700)?;
    }
    Ok(changed)
}

fn install_agy(home: &Path, bin: &str) -> Result<HookActionReport, String> {
    let script_path = agy_helper_script_path(home);
    let script_changed = sync_agy_script(&script_path, bin)?;
    let cfg_path = AgentHookTarget::AgyCli.config_path(home);
    let existing_cfg = read_optional(&cfg_path)?;
    let merged_cfg = merge_agy_hooks_json(existing_cfg.as_deref(), &script_path)?;
    let cfg_changed = existing_cfg.as_deref() != Some(merged_cfg.as_str());
    if cfg_changed {
        write_atomic_mode(&cfg_path, &merged_cfg, 0o600)?;
    }
    let changed = script_changed || cfg_changed;
    let detail = if changed { "AGY CLI hooks and helper installed" } else { "AGY CLI hooks already up to date" };
    Ok(report(AgentHookTarget::AgyCli, HookInstallStatus::Installed, cfg_path, changed, detail))
}

fn remove_managed_agy_script(script_path: &Path) -> Result<bool, String> {
    let Some(raw) = read_optional(script_path)? else {
        return Ok(false);
    };
    if !raw.contains(AGY_SCRIPT_MARKER) {
        return Ok(false);
    }
    fs::remove_file(script_path)
        .map_err(|e| format!("failed to remove {}: {e}", script_path.display()))?;
    Ok(true)
}

fn uninstall_agy(home: &Path) -> Result<HookActionReport, String> {
    let cfg_path = AgentHookTarget::AgyCli.config_path(home);
    let mut report = uninstall_json_target(
        cfg_path,
        AgentHookTarget::AgyCli,
        is_agy_akhterm_hook,
        "AGY CLI hooks",
    )?;
    let script_removed = remove_managed_agy_script(&agy_helper_script_path(home))?;
    report.changed |= script_removed;
    if report.changed {
        report.detail = "AGY CLI hooks removed".to_owned();
    }
    Ok(report)
}

#[cfg(test)]
#[path = "hook_installer_tests.rs"]
mod tests;
