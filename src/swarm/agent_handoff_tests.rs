//! Unit and headless GTK tests for context rot classification and Agent Handoff Pack.

use super::*;
use std::ffi::CStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::ptr;

use crate::proc_radar::tests::with_headless_gtk;
use crate::session::SessionCategory;
use crate::telemetry::{TabTokenRow, TelemetrySource, TokensMenu};
use crate::theme_mode::{build_stylesheet, ThemeMode};

fn make_temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("akhterm_agent_handoff_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C").arg(dir).args(args)
        .env("GIT_AUTHOR_NAME", "AkhTerm Test")
        .env("GIT_AUTHOR_EMAIL", "test@akhterm.local")
        .env("GIT_COMMITTER_NAME", "AkhTerm Test")
        .env("GIT_COMMITTER_EMAIL", "test@akhterm.local")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .status().expect("git command");
    assert!(status.success(), "git {args:?} failed");
}

fn has_css_class(widget: *mut GtkWidget, cls: &str) -> bool {
    let c = CString::new(cls).unwrap();
    unsafe { gtk_style_context_has_class(gtk_widget_get_style_context(widget), c.as_ptr()) != 0 }
}

fn init_temp_git_repo(tag: &str, branch: &str, file: &str, v1: &str, v2: &str) -> PathBuf {
    let repo = make_temp_dir(tag);
    run_git(&repo, &["init", "-b", branch]);
    fs::write(repo.join(file), v1).unwrap();
    run_git(&repo, &["add", "."]);
    run_git(&repo, &["commit", "-m", "init"]);
    fs::write(repo.join(file), v2).unwrap();
    repo
}

fn read_clipboard_text() -> String {
    let clip = unsafe { gtk_clipboard_get(GDK_SELECTION_CLIPBOARD) };
    let raw = unsafe { gtk_clipboard_wait_for_text(clip) };
    assert!(!raw.is_null(), "handoff pack must be copied to clipboard");
    let text = unsafe { CStr::from_ptr(raw).to_string_lossy().into_owned() };
    unsafe { g_free(raw as *mut c_void) };
    text
}

fn make_token_row(pct: u8, cache_pairs: &[(u64, u64)]) -> TabTokenRow {
    let mut snap = TokenSnapshot {
        source: TelemetrySource::Claude,
        context_tokens: u64::from(pct) * 2_000,
        context_pct: Some(pct),
        ..Default::default()
    };
    for (idx, &(read, input)) in cache_pairs.iter().enumerate() {
        snap.requests.record(Some(read), input, Some(idx as u64 + 1));
    }
    TabTokenRow {
        title: "Agent".to_owned(), agent: "Claude".to_owned(), is_active: true,
        working_secs: None, last_turn_secs: Some(12), snap,
    }
}

#[test]
fn classifies_context_rot_fill_thresholds() {
    assert_eq!(ContextHealth::classify(Some(0), &[]).severity, ContextSeverity::Healthy);
    assert_eq!(ContextHealth::classify(Some(69), &[]).severity, ContextSeverity::Healthy);
    assert_eq!(ContextHealth::classify(Some(70), &[]).severity, ContextSeverity::Warning);
    assert_eq!(ContextHealth::classify(Some(84), &[]).severity, ContextSeverity::Warning);
    assert_eq!(ContextHealth::classify(Some(85), &[]).severity, ContextSeverity::Critical);
    assert_eq!(ContextHealth::classify(Some(100), &[]).severity, ContextSeverity::Critical);
}

#[test]
fn classifies_cache_drops_and_recovery() {
    let sharp = ContextHealth::classify(Some(40), &[90, 65]);
    assert!(sharp.has_cache_drop());
    assert_eq!((sharp.cache_prev_pct, sharp.cache_curr_pct), (Some(90), Some(65)));
    let gradual = ContextHealth::classify(Some(50), &[80, 62, 48]);
    assert!(gradual.has_cache_drop());
    assert_eq!((gradual.cache_prev_pct, gradual.cache_curr_pct), (Some(80), Some(48)));
    assert!(!ContextHealth::classify(Some(50), &[90, 66]).has_cache_drop());
    assert!(!ContextHealth::classify(Some(50), &[60, 40]).has_cache_drop());
    assert!(!ContextHealth::classify(Some(50), &[90, 40, 88]).has_cache_drop());
}

#[test]
fn snapshot_health_and_bilingual_banners_have_no_arrows() {
    let mut snap = TokenSnapshot { source: TelemetrySource::Claude, context_tokens: 140_000, ..Default::default() };
    assert_eq!(ContextHealth::from_snapshot(&snap).severity, ContextSeverity::Warning);
    snap.context_tokens = 170_000;
    snap.requests.record(Some(90_000), 100_000, Some(1));
    snap.requests.record(Some(35_000), 100_000, Some(2));
    let health = ContextHealth::from_snapshot(&snap);
    assert!(health.is_critical() && health.has_cache_drop());
    for loc in [UiLocale::Ru, UiLocale::En] {
        let banner = health.banner_text(loc).unwrap();
        assert!(!banner.contains("->") && !banner.contains('→') && !banner.contains("=>"));
    }
}

fn sample_repo_pack(repo: &Path, branch: Option<String>, diff: Option<String>, brief: Option<PathBuf>) -> String {
    let tail = "error[E0308]: mismatched types\n```rust\nlet x = 1;\n```\n````\nnested\n````\n";
    build_handoff_pack(&HandoffPackInput {
        session_title: "Work".to_owned(), source_agent: "Claude".to_owned(),
        target_agent: HandoffAgent::Codex, cwd: repo.to_string_lossy().into_owned(),
        git_branch: branch, git_diff_stat: diff, terminal_tail: tail.to_owned(),
        active_brief: brief, health: ContextHealth::classify(Some(88), &[92, 40]), locale: UiLocale::En,
    })
}

fn verify_repo_pack_and_staging(repo: &Path, pack: &str) {
    assert!(pack.contains("feature/handoff-test") && pack.contains("lib.rs") && pack.contains("brief.md"));
    assert!(pack.contains("`````text") && pack.contains("Critical context 88%"));
    assert!(!pack.contains("->") && !pack.contains('→') && !pack.contains("=>"));
    let staged = stage_handoff_pack_in(repo, HandoffAgent::Codex, pack, 1_700_000_001).unwrap();
    assert_eq!(fs::read_to_string(&staged).unwrap(), pack);
    assert_eq!(fs::metadata(&staged).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn builds_and_stages_handoff_pack_in_temp_git_repo_with_safe_fences() {
    let repo = init_temp_git_repo("repo", "feature/handoff-test", "lib.rs", "pub fn v() -> i32 { 1 }\n", "pub fn v() -> i32 { 42 }\n");
    let brief = repo.join("brief.md");
    fs::write(&brief, "# Task Brief\n").unwrap();
    let (branch, diff_stat) = collect_git_context(&repo);
    assert_eq!(branch.as_deref(), Some("feature/handoff-test"));
    assert!(diff_stat.as_deref().unwrap_or("").contains("lib.rs"));
    let found_brief = find_active_brief(&repo, "Opened brief.md for review\n", None);
    assert_eq!(found_brief.as_deref(), Some(brief.as_path()));
    let pack = sample_repo_pack(&repo, branch, diff_stat, found_brief);
    verify_repo_pack_and_staging(&repo, &pack);
    let _ = fs::remove_dir_all(&repo);
}

fn verify_css_mode_parses(mode: ThemeMode, dark_css: &str, extra_css: &str) {
    let sheet = build_stylesheet(mode, dark_css, extra_css);
    assert!(sheet.contains(".ctx-warn") && sheet.contains(".ctx-crit") && sheet.contains(".ctx-cache-drop"));
    let c_css = CString::new(sheet).unwrap();
    let mut err: *mut c_void = ptr::null_mut();
    unsafe {
        let provider = gtk_css_provider_new();
        let ok = gtk_css_provider_load_from_data(provider, c_css.as_ptr(), -1, &mut err);
        assert_ne!(ok, 0, "GTK CSS parse failed for {mode:?}");
        assert!(err.is_null(), "unexpected GError for {mode:?}");
    }
}

#[test]
fn dark_and_light_css_parse_context_health_classes_cleanly() {
    let dark_css = include_str!("../ui/theme.css");
    let extra_css = concat!(
        include_str!("../workspace/tab_strip.css"),
        include_str!("../workspace/sidebar.css"),
        include_str!("../ui/palette.css"),
    );
    with_headless_gtk(|| {
        verify_css_mode_parses(ThemeMode::Dark, dark_css, extra_css);
        verify_css_mode_parses(ThemeMode::Light, dark_css, extra_css);
    });
}

fn assert_tokens_health_classes(menu: &TokensMenu) {
    let warn_view = crate::telemetry::build_tokens_view_for_mode(&[make_token_row(74, &[])], ThemeMode::Dark);
    menu.apply(&warn_view);
    assert!(has_css_class(menu.button_widget(), "ctx-warn") && !has_css_class(menu.button_widget(), "ctx-crit"));
    assert!(has_css_class(menu.health_banner_widget(), "ctx-warn"));
    let crit_view = crate::telemetry::build_tokens_view_for_mode(
        &[make_token_row(89, &[(90_000, 100_000), (38_000, 100_000)])],
        ThemeMode::Light,
    );
    menu.apply(&crit_view);
    assert!(has_css_class(menu.button_widget(), "ctx-crit") && has_css_class(menu.button_widget(), "ctx-cache-drop"));
    assert!(has_css_class(menu.health_banner_widget(), "ctx-crit") && has_css_class(menu.health_banner_widget(), "ctx-cache-drop"));
}

#[test]
fn headless_tokens_popover_and_sidebar_apply_context_health_classes() {
    with_headless_gtk(|| {
        let (win, _) = crate::tests::install_test_window();
        let menu = app_state::with_app(|app| app.tokens_menu).unwrap();
        assert_tokens_health_classes(&menu);
        let sid = session_ops::add_tab("Work", "", "/tmp", SessionCategory::Project, &["bash", "--noprofile", "--norc", "-i"], Tab::local("shell"));
        let metric_widget = app_state::with_app_mut(|app| {
            let s = &mut app.sessions[0];
            s.telemetry.vte_fallback = make_token_row(89, &[(88_000, 100_000), (40_000, 100_000)]).snap;
            s.sidebar_row.update(0, s, None, "", true);
            s.sidebar_row.metric_widget()
        }).unwrap();
        assert!(has_css_class(metric_widget, "ctx-crit") && has_css_class(metric_widget, "ctx-cache-drop"));
        crate::tests::cleanup_test_window(win, &[sid]);
    });
}

#[test]
fn palette_includes_bilingual_handoff_items_for_all_agents() {
    for locale in [UiLocale::Ru, UiLocale::En] {
        let items = build_palette_handoff_items(locale);
        assert_eq!(items.len(), 3);
        for (agent, item) in HANDOFF_AGENTS.iter().zip(items.iter()) {
            assert_eq!(item.action, PaletteAction::HandOffAgent(agent.code()));
            assert!(item.title.contains(agent.label()));
            assert!(!item.title.contains("->") && !item.detail.contains("->"));
        }
    }
}

#[test]
fn headless_one_click_handoff_opens_new_tab_and_copies_pack() {
    let repo = init_temp_git_repo("launcher", "feature/live-handoff", "main.rs", "fn main() {}\n", "fn main() { println!(\"hi\"); }\n");
    with_headless_gtk(|| {
        let (win, footer) = crate::tests::install_test_window();
        let dir = repo.to_string_lossy().into_owned();
        let tab = Tab::Local { profile: "shell".to_owned(), cwd: dir.clone() };
        let sid1 = session_ops::add_tab("Shell", "", &dir, SessionCategory::Project, &["bash", "--noprofile", "--norc", "-i"], tab);
        let sid2 = launch_handoff_into(HandoffAgent::Claude, &repo.join(".handoffs")).expect("new handoff tab");
        assert_ne!(sid1, sid2);
        let new_cwd = app_state::with_app(|app| app.sessions.iter().find(|s| s.id == sid2).map(|s| s.cwd.clone())).flatten();
        assert_eq!(new_cwd.as_deref(), Some(dir.as_str()));
        let clip_text = read_clipboard_text();
        assert!(clip_text.contains("feature/live-handoff") && clip_text.contains("main.rs"));
        let footer_text = unsafe { CStr::from_ptr(gtk_label_get_text(footer)).to_string_lossy().into_owned() };
        assert!(footer_text.contains("Claude"), "footer should report Claude handoff: {footer_text}");
        crate::tests::cleanup_test_window(win, &[sid1, sid2]);
    });
    let _ = fs::remove_dir_all(&repo);
}
