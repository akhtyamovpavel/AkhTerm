//! Unit tests for the 1-click official agent hook installer (isolated temp homes only).

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn temp_home(tag: &str) -> PathBuf {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("akhterm-hook-inst-{tag}-{}-{seq}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create isolated temp home");
    dir
}

fn file_mode(path: &Path) -> u32 {
    fs::metadata(path).expect("stat file").permissions().mode() & 0o777
}

fn json_event_len(parsed: &JsonVal, event: &str) -> usize {
    parsed.get("hooks").and_then(|h| h.get(event)).and_then(JsonVal::as_array).map_or(0, <[JsonVal]>::len)
}

fn verify_target_install_and_reinstall(home: &Path, target: AgentHookTarget) {
    assert_eq!(inspect_status_with_bin(home, target, "akhterm"), HookInstallStatus::NotInstalled);
    let rep = install_hooks_with_bin(home, target, "akhterm").expect("install");
    assert!(rep.changed && rep.status == HookInstallStatus::Installed);
    assert_eq!(file_mode(&target.config_path(home)), 0o600);
    assert_eq!(inspect_status_with_bin(home, target, "akhterm"), HookInstallStatus::Installed);
    let again = install_hooks_with_bin(home, target, "akhterm").expect("reinstall");
    assert!(!again.changed);
}

#[test]
fn empty_home_reports_not_installed_and_installs_with_strict_modes() {
    let home = temp_home("empty");
    for target in AgentHookTarget::ALL {
        verify_target_install_and_reinstall(&home, target);
    }
    assert_eq!(file_mode(&agy_helper_script_path(&home)), 0o700);
    let _ = fs::remove_dir_all(&home);
}

fn seed_claude_settings(home: &Path) -> PathBuf {
    let path = AgentHookTarget::ClaudeCode.config_path(home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let initial = "{\"model\":\"opus\",\"permissions\":{\"allow\":[\"Bash(git status)\"]},\"hooks\":{\"PostToolUse\":[{\"matcher\":\"Edit\",\"hooks\":[{\"type\":\"command\",\"command\":\"echo ok\"}]}],\"Stop\":[{\"matcher\":\"\",\"hooks\":[{\"type\":\"command\",\"command\":\"echo user-stop\"}]}]}}";
    fs::write(&path, initial).unwrap();
    path
}

fn assert_claude_counts(path: &Path, pre: usize, notif: usize, stop: usize) {
    let parsed = parse_json(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(parsed.get("model").and_then(JsonVal::as_str), Some("opus"));
    assert!(parsed.get("permissions").is_some() && json_event_len(&parsed, "PostToolUse") == 1);
    assert_eq!((json_event_len(&parsed, "PreToolUse"), json_event_len(&parsed, "Notification"), json_event_len(&parsed, "Stop")), (pre, notif, stop));
}

#[test]
fn claude_preserves_user_keys_and_hooks_on_install_and_uninstall() {
    let home = temp_home("claude-preserve");
    let path = seed_claude_settings(&home);
    assert!(install_hooks_with_bin(&home, AgentHookTarget::ClaudeCode, "akhterm").unwrap().changed);
    assert_claude_counts(&path, 1, 1, 2);
    assert!(uninstall_hooks(&home, AgentHookTarget::ClaudeCode).unwrap().changed);
    assert_eq!(inspect_status_with_bin(&home, AgentHookTarget::ClaudeCode, "akhterm"), HookInstallStatus::NotInstalled);
    assert_claude_counts(&path, 0, 0, 1);
    let _ = fs::remove_dir_all(&home);
}

fn verify_codex_conflict(home: &Path, path: &Path) {
    let custom = "notify = [\"my-custom-notifier\"]\n";
    fs::write(path, custom).unwrap();
    assert!(install_hooks_with_bin(home, AgentHookTarget::CodexCli, "akhterm").is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), custom);
}

#[test]
fn codex_preserves_toml_comments_and_tables_and_handles_conflict_and_uninstall() {
    let home = temp_home("codex-preserve");
    let path = AgentHookTarget::CodexCli.config_path(&home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let initial = "# user comment\nmodel = \"gpt-5.4\"\n\n[mcp_servers.docs]\ncommand = \"npx\"\n";
    fs::write(&path, initial).unwrap();
    assert!(install_hooks_with_bin(&home, AgentHookTarget::CodexCli, "akhterm").unwrap().changed);
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("# user comment") && body.find("notify = [").unwrap() < body.find("[mcp_servers.docs]").unwrap());
    assert!(uninstall_hooks(&home, AgentHookTarget::CodexCli).unwrap().changed);
    assert_eq!(fs::read_to_string(&path).unwrap(), initial);
    verify_codex_conflict(&home, &path);
    let _ = fs::remove_dir_all(&home);
}

fn verify_agy_script_exec(script: &Path) {
    assert!(script.exists() && file_mode(script) == 0o700);
    assert!(Command::new("sh").arg("-n").arg(script).status().unwrap().success());
    let dry = Command::new(script).env_remove("AKHTERM_PANE_ID").arg("done").status().unwrap();
    assert!(dry.success());
}

fn assert_agy_counts(cfg: &Path, pre: usize, notif: usize, stop: usize) {
    let parsed = parse_json(&fs::read_to_string(cfg).unwrap()).unwrap();
    assert_eq!(parsed.get("profile").and_then(JsonVal::as_str), Some("default"));
    assert_eq!((json_event_len(&parsed, "PreToolUse"), json_event_len(&parsed, "Notification"), json_event_len(&parsed, "Stop")), (pre, notif, stop));
}

#[test]
fn agy_installs_valid_posix_script_preserves_user_hooks_and_cleans_up() {
    let home = temp_home("agy-preserve");
    let cfg = AgentHookTarget::AgyCli.config_path(&home);
    let script = agy_helper_script_path(&home);
    fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    let initial = "{\"profile\":\"default\",\"hooks\":{\"Stop\":[{\"matcher\":\"\",\"hooks\":[{\"type\":\"command\",\"command\":\"echo keep-me\"}]}]}}";
    fs::write(&cfg, initial).unwrap();
    assert!(install_hooks_with_bin(&home, AgentHookTarget::AgyCli, "akhterm").unwrap().changed);
    verify_agy_script_exec(&script);
    assert_agy_counts(&cfg, 1, 1, 2);
    assert!(uninstall_hooks(&home, AgentHookTarget::AgyCli).unwrap().changed && !script.exists());
    assert_agy_counts(&cfg, 0, 0, 1);
    assert_eq!(inspect_status_with_bin(&home, AgentHookTarget::AgyCli, "akhterm"), HookInstallStatus::NotInstalled);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn detects_outdated_hooks_across_all_three_targets_and_repairs_them() {
    let home = temp_home("outdated");
    for target in AgentHookTarget::ALL {
        install_hooks_with_bin(&home, target, "/old/bin/akhterm").unwrap();
        assert_eq!(inspect_status_with_bin(&home, target, "akhterm"), HookInstallStatus::Outdated);
        assert!(install_hooks_with_bin(&home, target, "akhterm").unwrap().changed);
        assert_eq!(inspect_status_with_bin(&home, target, "akhterm"), HookInstallStatus::Installed);
    }
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn install_all_and_inspect_all_work_on_isolated_temp_home() {
    let home = temp_home("all");
    let reports = install_all_hooks(&home).expect("install all");
    assert_eq!(reports.len(), 3);
    assert!(reports.iter().all(|r| r.status == HookInstallStatus::Installed && r.changed));
    assert!(inspect_all(&home).iter().all(|(_, s)| *s == HookInstallStatus::Installed));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn command_palette_exposes_bilingual_install_agent_hooks_entry() {
    let src = crate::palette::PaletteSource {
        sessions: &[], tmux_items: &[], profiles: &[], ssh_hosts: &[],
        briefs_dir: None, extra_handoffs: &[], active_git: None, active_is_worktree: false,
    };
    let ru = crate::palette::collect_items_for_locale(&src, crate::i18n::UiLocale::Ru);
    let en = crate::palette::collect_items_for_locale(&src, crate::i18n::UiLocale::En);
    assert!(ru.iter().any(|i| i.action == crate::palette::PaletteAction::InstallAgentHooks && i.title == "Установить хуки агентов"));
    assert!(en.iter().any(|i| i.action == crate::palette::PaletteAction::InstallAgentHooks && i.title == "Install Agent Hooks"));
}

#[test]
fn hook_installer_respects_minimum_mi() {
    let (_, _, score) = crate::mi::compute_file_mi(include_str!("hook_installer.rs"));
    assert!(score >= 60.0, "hook_installer.rs MI {score:.2} < 60.0");
}

#[test]
fn install_all_continues_to_agy_when_codex_has_custom_notify_conflict() {
    let home = temp_home("codex-conflict-all");
    let codex_cfg = AgentHookTarget::CodexCli.config_path(&home);
    fs::create_dir_all(codex_cfg.parent().unwrap()).unwrap();
    let custom = "notify = [\"custom-notifier\"]\n";
    fs::write(&codex_cfg, custom).unwrap();
    let err = install_all_hooks(&home).expect_err("codex conflict should report error");
    assert!(err.contains("Codex CLI"));
    assert_eq!(fs::read_to_string(&codex_cfg).unwrap(), custom);
    assert_eq!(inspect_status(&home, AgentHookTarget::ClaudeCode), HookInstallStatus::Installed);
    assert_eq!(inspect_status(&home, AgentHookTarget::AgyCli), HookInstallStatus::Installed);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn claude_and_agy_escape_shell_metacharacters_in_binary_path() {
    let home = temp_home("escape-bin");
    let tricky_bin = "/tmp/my$dir/\"akhterm\"";
    install_hooks_with_bin(&home, AgentHookTarget::ClaudeCode, tricky_bin).unwrap();
    install_hooks_with_bin(&home, AgentHookTarget::AgyCli, tricky_bin).unwrap();
    assert_eq!(inspect_status_with_bin(&home, AgentHookTarget::ClaudeCode, tricky_bin), HookInstallStatus::Installed);
    assert_eq!(inspect_status_with_bin(&home, AgentHookTarget::AgyCli, tricky_bin), HookInstallStatus::Installed);
    let claude_raw = fs::read_to_string(AgentHookTarget::ClaudeCode.config_path(&home)).unwrap();
    assert!(claude_raw.contains("\\$dir"));
    let _ = fs::remove_dir_all(&home);
}
