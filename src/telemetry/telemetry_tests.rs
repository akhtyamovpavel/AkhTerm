use super::*;
use crate::telemetry_agy::{text_mentions_workspace, transcript_from_fd_target};
use crate::theme_mode::ThemeMode;
use std::ffi::{c_char, c_void, CStr};
use std::io::Write;
use std::process::{Command, Stdio};
use std::ptr;
use std::thread;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("akhterm_tel_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn poll_new_tailer(path: &Path, kind: TelemetrySource) -> TranscriptTailer {
    let mut tailer = TranscriptTailer::default();
    tailer.poll_file(path, kind).unwrap();
    tailer
}

#[test]
fn incremental_agy_tailing_seeks_only_appended_bytes() {
    let dir = temp_dir("agy_tail");
    let log = dir.join("transcript.jsonl");
    let l1 = "{\"step_index\":0,\"type\":\"GENERIC\",\"content\":\"{\\\"type\\\":\\\"PLANNER_RESPONSE\\\",\\\"input_tokens\\\":999999}\"}\n";
    let l2 = "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":4500,\"cache_read_tokens\":20500,\"output_tokens\":600}\n";
    fs::write(&log, format!("{l1}{l2}")).unwrap();

    let mut tailer = TranscriptTailer::default();
    assert!(tailer.poll_file(&log, TelemetrySource::Agy).unwrap());
    assert_eq!((tailer.offset, tailer.snapshot.context_tokens), ((l1.len() + l2.len()) as u64, 25_600));
    assert!(!tailer.poll_file(&log, TelemetrySource::Agy).unwrap());
    overwrite_prefix_and_append(&log, l1.len() + l2.len() - 1);
    assert!(tailer.poll_file(&log, TelemetrySource::Agy).unwrap());
    let s = &tailer.snapshot;
    assert_eq!((s.input_tokens, s.cache_tokens, s.output_tokens, s.context_tokens), (14_500, 94_500, 1_500, 84_900));
    let _ = fs::remove_dir_all(&dir);
}

fn overwrite_prefix_and_append(log: &Path, scrub_len: usize) {
    let mut f = fs::OpenOptions::new().write(true).open(log).unwrap();
    f.seek(SeekFrom::Start(0)).unwrap();
    f.write_all(&vec![b' '; scrub_len]).unwrap();
    f.seek(SeekFrom::End(0)).unwrap();
    let l3 = "{\"step_index\":2,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":10000,\"cache_read_tokens\":74000,\"output_tokens\":900}\n";
    f.write_all(l3.as_bytes()).unwrap();
}

#[test]
fn tailer_resets_safely_when_transcript_is_truncated() {
    let dir = temp_dir("agy_rot");
    let log = dir.join("transcript.jsonl");
    let long = "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":30000,\"cache_read_tokens\":90000,\"output_tokens\":1200}\n";
    fs::write(&log, format!("{long}{long}")).unwrap();
    let mut tailer = poll_new_tailer(&log, TelemetrySource::Agy);
    assert_eq!(tailer.snapshot.input_tokens, 60_000);

    let short = "{\"step_index\":0,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":1200,\"cache_read_tokens\":3800,\"output_tokens\":150}\n";
    fs::write(&log, short).unwrap();
    assert!(tailer.poll_file(&log, TelemetrySource::Agy).unwrap());
    assert_eq!((tailer.offset, tailer.snapshot.input_tokens, tailer.snapshot.context_tokens), (short.len() as u64, 1_200, 5_150));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn agy_checkpoint_resets_stale_pre_compaction_context_tokens() {
    let dir = temp_dir("agy_ckpt");
    let log = dir.join("transcript.jsonl");
    let pre = "{\"step_index\":10,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":38000,\"cache_read_tokens\":218000,\"output_tokens\":2000}\n";
    let ckpt = "{\"step_index\":11,\"type\":\"CHECKPOINT\",\"input_tokens\":258000,\"cache_read_tokens\":0,\"output_tokens\":4200}\n";
    fs::write(&log, pre).unwrap();
    let mut tailer = poll_new_tailer(&log, TelemetrySource::Agy);
    tailer.snapshot.context_pct = Some(26);
    assert_eq!(tailer.snapshot.context_tokens, 258_000);

    fs::write(&log, format!("{pre}{ckpt}")).unwrap();
    tailer.poll_file(&log, TelemetrySource::Agy).unwrap();
    let s = &tailer.snapshot;
    assert_eq!((s.context_tokens, s.context_pct, s.output_tokens), (4_200, None, 6_200));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discovers_and_parses_claude_project_jsonl() {
    let base = temp_dir("claude_disc");
    let (home, repo) = (base.join("home"), base.join("home/work/my.repo"));
    fs::create_dir_all(repo.join("src")).unwrap();
    let encoded = encode_claude_cwd(&repo.to_string_lossy());
    assert!(encoded.ends_with("-work-my-repo"));
    let claude_dir = home.join(".claude/projects").join(&encoded);
    fs::create_dir_all(&claude_dir).unwrap();
    write_claude_fixture(&claude_dir.join("session-a.jsonl"));

    let (found, kind) = discover_transcript(&home, &repo.join("src").to_string_lossy(), 0, "Claude").unwrap();
    let s = poll_new_tailer(&found, kind).snapshot;
    assert_eq!((s.effective_input(), s.cache_tokens, s.output_tokens, s.context_tokens), (5_500, 65_000, 650, 35_500));
    let _ = fs::remove_dir_all(&base);
}

fn write_claude_fixture(path: &Path) {
    let c1 = "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":200,\"cache_creation_input_tokens\":4800,\"cache_read_input_tokens\":30000,\"output_tokens\":120}}}\n";
    let c1_dup = "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":200,\"cache_creation_input_tokens\":4800,\"cache_read_input_tokens\":30000,\"output_tokens\":400}}}\n";
    let c2 = "{\"type\":\"assistant\",\"message\":{\"id\":\"msg_2\",\"usage\":{\"input_tokens\":500,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":35000,\"output_tokens\":250}}}\n";
    fs::write(path, format!("{c1}{c1_dup}{c2}")).unwrap();
}

#[test]
fn discovers_and_parses_codex_session_jsonl() {
    let base = temp_dir("codex_disc");
    let (home, repo, sibling) = (base.join("home"), base.join("home/work/app"), base.join("home/work/app-wt"));
    let codex_day = home.join(".codex/sessions/2026/11/11");
    for d in [&repo, &sibling, &codex_day] {
        fs::create_dir_all(d).unwrap();
    }
    write_codex_fixture(&codex_day, &repo, &sibling);

    let (found, kind) = discover_transcript(&home, &repo.to_string_lossy(), 0, "Codex").unwrap();
    assert_eq!(found, codex_day.join("rollout-main.jsonl"));
    let s = poll_new_tailer(&found, kind).snapshot;
    assert_eq!((s.input_tokens, s.cache_tokens, s.output_tokens, s.context_tokens, s.context_pct), (24_000, 72_000, 3_400, 58_000, Some(29)));
    let _ = fs::remove_dir_all(&base);
}

fn write_codex_fixture(day: &Path, repo: &Path, sibling: &Path) {
    fs::write(
        day.join("rollout-wt.jsonl"),
        format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n", sibling.display()),
    )
    .unwrap();
    let hdr = format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n", repo.display());
    let ev = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":96000,\"cached_input_tokens\":72000,\"output_tokens\":3400,\"total_tokens\":99400},\"last_token_usage\":{\"input_tokens\":58000,\"cached_input_tokens\":50000,\"output_tokens\":1100,\"total_tokens\":59100},\"model_context_window\":200000}}}\n";
    fs::write(day.join("rollout-main.jsonl"), format!("{hdr}{ev}")).unwrap();
}

#[test]
fn matches_agy_fd_and_workspace_boundaries_cleanly() {
    let base = temp_dir("agy_disc");
    let home = base.join("home");
    let logs_dir = home.join(".gemini/antigravity-cli/brain/abc-123/.system_generated/logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let transcript = logs_dir.join("transcript.jsonl");
    fs::write(&transcript, "{\"type\":\"SYSTEM_MESSAGE\",\"content\":\"Workspace: /tmp/proj-alpha\\n\"}\n").unwrap();

    let brain_root = home.join(".gemini/antigravity-cli/brain");
    assert_eq!(transcript_from_fd_target(&transcript, &brain_root).as_deref(), Some(transcript.as_path()));
    assert_eq!(text_mentions_workspace("dir: /tmp/proj-alpha-wt\n", "/tmp/proj-alpha"), None);
    assert_eq!(discover_transcript(&home, "/tmp/proj-alpha", 0, "AGY").map(|p| p.0), Some(transcript));
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn parses_vte_status_bar_fallback_patterns() {
    let screen = "999k tokens · 99% context · $9.99\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n╭──────────────────────╮\n│ sonnet · 142k tokens · 68% context · $0.42 │\n╰──────────────────────╯\n\n";
    let s1 = parse_vte_status_fallback(screen).unwrap();
    assert_eq!((s1.context_tokens, s1.context_pct, s1.cost_usd, s1.alert_level()), (142_000, Some(68), Some(0.42), ContextAlert::Warn));
    let s2 = parse_vte_status_fallback("agent ready\nused 12,400 tokens | 25% context left\n").unwrap();
    assert_eq!((s2.context_tokens, s2.context_pct), (12_400, Some(75)));
    let s3 = parse_vte_status_fallback("status: ctx:84k cost:$1.05\n").unwrap();
    assert_eq!((s3.context_tokens, s3.cost_usd), (84_000, Some(1.05)));
    assert!(parse_vte_status_fallback("user@host:~/repo$ ls -la\n-rw-r--r-- 1 u u 12400 a.txt\n").is_none());
}

#[test]
fn parses_ansi_and_ratio_vte_status_patterns() {
    let ansi = "\x1b]0;remote-claude\x07\x1b[38;5;244mmodel · 84.5k/200k · $0.42\x1b[0m\n\nl1\nl2\nl3\nl4\nl5\n";
    let s1 = parse_vte_status_fallback(ansi).unwrap();
    assert_eq!((s1.context_tokens, s1.context_pct, s1.cost_usd), (84_500, Some(42), Some(0.42)));
    let spaced = parse_vte_status_fallback("\x1b[1m84k / 200k\x1b[0m\n").unwrap();
    assert_eq!((spaced.context_tokens, spaced.context_pct), (84_000, Some(42)));
    let named = parse_vte_status_fallback("Context: 64% · ctx: 84.2k\n").unwrap();
    assert_eq!((named.context_tokens, named.context_pct), (84_200, Some(64)));
    let comma = parse_vte_status_fallback("ctx 84k | 120,450 tok\n").unwrap();
    assert_eq!(comma.context_tokens, 120_450);
    assert!(parse_vte_status_fallback("[1/3] Compiling crate v0.4.0\n").is_none());
}

#[test]
fn formats_timers_compact_tokens_and_popover_markup() {
    crate::i18n::with_locale_lock(|| {
        crate::i18n::set_locale(crate::i18n::UiLocale::Ru);
        assert_duration_formatting();
        assert_compact_token_formatting();
        let view = build_tokens_view(&[sample_high_row()]);
        assert_eq!(view.button_label.as_deref(), Some("Токены · 184k ▾"));
        assert_eq!(view.alert, ContextAlert::High);
        let markup = view.markup.unwrap();
        assert!(markup.contains("R&amp;D &lt;main&gt;") && markup.contains("Итого по окну (1):"));
    });
}

fn assert_duration_formatting() {
    assert_eq!(format_turn_duration(42), "42s");
    assert_eq!(format_turn_duration(72), "1m 12s");
    assert_eq!(format_turn_duration(134), "2m 14s");
    assert_eq!(format_turn_duration(3_725), "1h 02m");
    assert_eq!(format_working_badge(42), "⏱ 42s");
    assert_eq!(format_working_sidebar_status(72), "В работе · 1m 12s");
}

fn assert_compact_token_formatting() {
    let cases = [
        (84_200, "84k"), (43_510, "44k"), (130_700, "131k"), (999_499, "999k"),
        (999_500, "1M"), (999_600, "1M"), (1_000_000, "1M"), (4_200, "4.2k"),
    ];
    for (tokens, expected) in cases {
        assert_eq!(format_compact_tokens(tokens), expected);
    }
}

fn sample_high_snapshot() -> TokenSnapshot {
    TokenSnapshot {
        source: TelemetrySource::Claude, input_tokens: 14_000, cache_tokens: 170_000,
        cache_write_tokens: 0, output_tokens: 3_200, context_tokens: 184_000,
        context_pct: Some(92), cost_usd: Some(0.42), ..Default::default()
    }
}

fn sample_high_row() -> TabTokenRow {
    TabTokenRow {
        title: "R&D <main>".to_owned(), agent: "Claude".to_owned(),
        is_active: true, working_secs: Some(134), last_turn_secs: None,
        snap: sample_high_snapshot(),
    }
}

#[test]
fn headless_broadway_tokens_menu_and_vte_fallback() {
    if std::env::var_os("AKHTERM_TELEMETRY_BWAY_CHILD").is_none() {
        let exe = std::env::current_exe().unwrap();
        let st = Command::new(exe)
            .args(["--exact", "telemetry::telemetry::tests::headless_broadway_tokens_menu_and_vte_fallback"])
            .env("AKHTERM_TELEMETRY_BWAY_CHILD", "1")
            .status()
            .unwrap();
        assert!(st.success());
        return;
    }
    run_broadway_tokens_check();
}

fn run_broadway_tokens_check() {
    let dir = temp_dir("bway17");
    let Ok(mut bway) = spawn_test_broadway(&dir) else {
        let _ = fs::remove_dir_all(&dir);
        return;
    };
    std::env::set_var("XDG_RUNTIME_DIR", &dir);
    std::env::set_var("GDK_BACKEND", "broadway");
    std::env::set_var("BROADWAY_DISPLAY", ":17");
    if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } != 0 {
        exercise_tokens_widgets();
    }
    let _ = bway.kill();
    let _ = bway.wait();
    let _ = fs::remove_dir_all(&dir);
}

fn spawn_test_broadway(dir: &Path) -> io::Result<std::process::Child> {
    let child = Command::new("broadwayd")
        .arg("-u")
        .arg(dir.join("http.sock"))
        .arg(":17")
        .env("XDG_RUNTIME_DIR", dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    for _ in 0..80 {
        if dir.join("broadway18.socket").exists() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(child)
}

extern "C" fn noop_click(_btn: *mut GtkWidget, _data: *mut c_void) {}

fn pump_gtk() {
    for _ in 0..60 {
        if unsafe { gtk_events_pending() } == 0 {
            break;
        }
        unsafe { gtk_main_iteration_do(0) };
    }
}

fn exercise_tokens_widgets() {
    unsafe {
        let (win, vte, menu) = create_test_tokens_window();
        pump_gtk();
        let line = "remote tmux · 142k tokens · 68% context · $0.42\r\n";
        vte_terminal_feed(vte, line.as_ptr() as *const c_char, line.len() as isize);
        thread::sleep(Duration::from_millis(35));
        pump_gtk();
        verify_vte_poll_and_menu(vte, menu);
        verify_tokens_popover_scroll(menu);
        verify_vte_context_panel_widget(vte);
        gtk_widget_destroy(win);
    }
}

fn build_scroll_test_rows() -> Vec<TabTokenRow> {
    let mut rows: Vec<_> = (1..=24)
        .map(|n| TabTokenRow {
            title: format!("Фоновая вкладка {n}"), agent: String::new(),
            is_active: false, working_secs: None, last_turn_secs: Some(2),
            snap: TokenSnapshot::default(),
        })
        .collect();
    rows.push(sample_high_row());
    rows
}

fn verify_tokens_popover_scroll(menu: TokensMenu) {
    let view = build_tokens_view_for_mode(&build_scroll_test_rows(), ThemeMode::Dark);
    let markup = view.markup.as_deref().unwrap();
    assert!(markup.starts_with("▸ "));
    assert!(markup.contains("ctx 184k") && !markup.contains("input 0"));
    menu.apply(&view);
    assert_popover_scrolled(menu);
}

fn assert_popover_scrolled(menu: TokensMenu) {
    unsafe { gtk_popover_popup(menu.popover()) };
    thread::sleep(Duration::from_millis(55));
    pump_gtk();
    let viewport = unsafe { gtk_widget_get_parent(menu.body_label) };
    let visible = unsafe { gtk_widget_get_allocated_height(viewport) };
    let content = unsafe { gtk_widget_get_allocated_height(menu.body_label) };
    assert!(visible >= 180 && content > visible, "visible={visible}, content={content}");
    unsafe { gtk_popover_popdown(menu.popover()) };
}

fn verify_vte_context_panel_widget(vte: *mut GtkWidget) {
    let panel = format!("\x1b[2J\x1b[H{}", sample_agy_context_screen().replace('\n', "\r\n"));
    unsafe { vte_terminal_feed(vte, panel.as_ptr() as *const c_char, panel.len() as isize) };
    thread::sleep(Duration::from_millis(35));
    pump_gtk();
    assert_vte_panel_job_override(vte);
}

fn assert_vte_panel_job_override(vte: *mut GtkWidget) {
    let mut seeded = SessionTelemetry::default();
    seeded.tailer.snapshot = TokenSnapshot {
        source: TelemetrySource::Agy, input_tokens: 9_000, context_tokens: 43_510, ..Default::default()
    };
    seeded.tailer.offset = 512;
    let job = make_poll_job(2, vte, "/nonexistent", false, "AGY", seeded);
    let (_, telem) = run_poll_job(job, Path::new("/nonexistent"), Instant::now());
    assert_eq!((telem.snapshot().context_tokens, telem.snapshot().context_pct), (130_700, Some(13)));
}

fn create_test_tokens_window() -> (*mut GtkWidget, *mut GtkWidget, TokensMenu) {
    unsafe {
        let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        gtk_window_set_default_size(win, 800, 480);
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        let topbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        let vte = vte_terminal_new();
        let menu = TokensMenu::new(topbar, noop_click);
        gtk_box_pack_start(vbox, topbar, 0, 1, 0);
        gtk_box_pack_start(vbox, vte, 1, 1, 0);
        gtk_container_add(win, vbox);
        gtk_widget_show_all(win);
        (win, vte, menu)
    }
}

fn make_poll_job(
    sid: usize,
    vte: *mut GtkWidget,
    cwd: &str,
    has_ssh: bool,
    agent: &str,
    telemetry: SessionTelemetry,
) -> SessionPollJob {
    SessionPollJob {
        sid, root_pid: 0, vte, local_cwd: Some(cwd.to_owned()),
        has_ssh, has_tmux: false, tmux_window: None,
        agent: agent.to_owned(), is_daemon: false, telemetry,
    }
}

fn verify_vte_poll_and_menu(vte: *mut GtkWidget, menu: TokensMenu) {
    let plain_local = make_poll_job(1, vte, "/tmp", false, "", SessionTelemetry::default());
    let (_, plain_telem) = run_poll_job(plain_local, Path::new("/nonexistent"), Instant::now());
    assert!(!plain_telem.snapshot().has_data());

    let mut seeded = SessionTelemetry::default();
    seeded.tailer.snapshot.context_tokens = 99_000;
    let job = make_poll_job(1, vte, "/tmp", true, "Claude", seeded);
    let (_, telem) = run_poll_job(job, Path::new("/nonexistent"), Instant::now());
    assert!(!telem.tailer.snapshot.has_data());
    assert_eq!(telem.snapshot().context_tokens, 142_000);
    apply_and_assert_menu(menu, &telem);
}

fn apply_and_assert_menu(menu: TokensMenu, telem: &SessionTelemetry) {
    let row = TabTokenRow {
        title: "srv · Claude".to_owned(), agent: "Claude".to_owned(),
        is_active: true, working_secs: Some(42), last_turn_secs: None,
        snap: telem.snapshot().clone(),
    };
    let light = build_tokens_view_for_mode(std::slice::from_ref(&row), ThemeMode::Light);
    let m = light.markup.as_deref().unwrap_or("");
    assert!(m.contains("#b45309") && m.contains("#1d4ed8") && m.contains("#5b21b6"));
    menu.apply(&build_tokens_view_for_mode(&[row], ThemeMode::Dark));
    let lbl = unsafe { CStr::from_ptr(gtk_label_get_text(menu.body_label)) }.to_string_lossy();
    assert!(lbl.contains("srv · Claude"));
}

fn assert_vte_fallback_scope_rules() {
    assert!(!should_poll_vte_fallback(Some("/tmp/repo"), false, ""));
    assert!(should_poll_vte_fallback(None, false, ""));
    assert!(should_poll_vte_fallback(Some("/tmp/repo"), true, ""));
    assert!(should_poll_vte_fallback(Some("/tmp/repo"), false, "Kimi"));
}

fn seed_claude_project(home: &Path, repo: &Path, file: &str) {
    fs::create_dir_all(repo).unwrap();
    let dir = home.join(".claude/projects").join(encode_claude_cwd(&repo.to_string_lossy()));
    fs::create_dir_all(&dir).unwrap();
    write_claude_fixture(&dir.join(file));
}

#[test]
fn scopes_ssh_override_and_vte_fallback_cleanly() {
    let base = temp_dir("ssh_scope");
    let (home, repo) = (base.join("home"), base.join("home/work/proj"));
    seed_claude_project(&home, &repo, "local.jsonl");
    assert_vte_fallback_scope_rules();
    let repo_s = repo.to_string_lossy();
    let (_, local_tel) = run_poll_job(make_poll_job(7, ptr::null_mut(), &repo_s, false, "Claude", SessionTelemetry::default()), &home, Instant::now());
    assert_eq!(local_tel.snapshot().context_tokens, 35_500);
    let (_, ssh_tel) = run_poll_job(make_poll_job(7, ptr::null_mut(), &repo_s, true, "Claude", local_tel), &home, Instant::now());
    assert!(!ssh_tel.snapshot().has_data() && ssh_tel.tailer.path.is_none());
    let _ = fs::remove_dir_all(&base);
}

fn write_brain_log(brain_root: &Path, id: &str, body: &str, age_secs: u64) -> PathBuf {
    let logs = brain_root.join(id).join(".system_generated/logs");
    fs::create_dir_all(&logs).unwrap();
    let path = logs.join("transcript.jsonl");
    fs::write(&path, body).unwrap();
    let stamp = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_760_000_000 + age_secs);
    fs::OpenOptions::new().write(true).open(&path).unwrap().set_modified(stamp).unwrap();
    path
}

fn register_subagent(brain_root: &Path, parent_id: &str, sub_id: &str) {
    let dir = brain_root.join(parent_id).join(".system_generated/subagents");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{sub_id}.json")), "{}").unwrap();
}

fn seed_productivity_layout(home: &Path) -> PathBuf {
    let prod = home.join("Productivity");
    fs::create_dir_all(prod.join("board")).unwrap();
    fs::create_dir_all(prod.join("akhterm-rs/.git")).unwrap();
    fs::create_dir_all(prod.join("akhterm-wt")).unwrap();
    fs::write(prod.join("akhterm-wt/.git"), "gitdir: /tmp/fake-wt\n").unwrap();
    prod
}

fn build_large_productivity_transcript() -> String {
    let pad = "x".repeat(70_000);
    let head = format!("{{\"step_index\":0,\"type\":\"GENERIC\",\"content\":\"{pad}\"}}\n");
    let ws = "{\"step_index\":40,\"type\":\"USER_INPUT\",\"content\":\"~/Productivity/board/РЕЕСТР-УЧЕБНЫХ-ССЫЛОК.md\"}\n";
    let turn = "{\"step_index\":41,\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":10000,\"cache_read_tokens\":120000,\"output_tokens\":700}\n";
    format!("{head}{ws}{turn}")
}

fn seed_competing_brains(brain: &Path, prod: &Path) {
    for idx in 0..20 {
        let body = format!("{{\"type\":\"USER_INPUT\",\"content\":\"/tmp/unrelated-{idx}\"}}\n");
        write_brain_log(brain, &format!("other-{idx}"), &body, 20 + idx);
    }
    let child = format!(
        "{{\"content\":\"{}/src/main.rs ~/Productivity/akhterm-rs\"}}\n{{\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":3000,\"cache_read_tokens\":40000,\"output_tokens\":510}}\n",
        prod.join("akhterm-rs").display()
    );
    write_brain_log(brain, "akhterm-child", &child, 60);
    write_brain_log(brain, "akhterm-wt", "{\"content\":\"~/Productivity/akhterm-wt/Cargo.toml\"}\n", 70);
    write_brain_log(brain, "swarm-sub", "{\"content\":\"~/Productivity/board/РЕЕСТР-УЧЕБНЫХ-ССЫЛОК.md\"}\n", 90);
    register_subagent(brain, "akhterm-child", "swarm-sub");
}

#[test]
fn selects_parent_productivity_over_nested_git_repos_and_subagents() {
    let base = temp_dir("prod_nested");
    let home = base.join("home");
    let prod = seed_productivity_layout(&home);
    let brain = home.join(".gemini/antigravity-cli/brain");
    let prod_log = write_brain_log(&brain, "prod-main", &build_large_productivity_transcript(), 10);
    seed_competing_brains(&brain, &prod);

    let (found, kind) = discover_transcript(&home, &prod.to_string_lossy(), 0, "AGY").unwrap();
    assert_eq!(found, prod_log);
    assert_polled_131k(&found, kind);

    fs::remove_file(&prod_log).unwrap();
    assert!(discover_transcript(&home, &prod.to_string_lossy(), 0, "AGY").is_none());
    let _ = fs::remove_dir_all(&base);
}

fn assert_polled_131k(found: &Path, kind: TelemetrySource) {
    let mut tailer = TranscriptTailer::default();
    tailer.poll_file(found, kind).unwrap();
    assert_eq!(tailer.snapshot.context_tokens, 130_700);
    assert_eq!(tailer.snapshot.compact_context_badge().as_deref(), Some("ctx 131k"));
}

fn sample_agy_context_screen() -> String {
    concat!(
        "Context Usage\n",
        "Gemini 4 Carbon (High) · 130.7k/1.0M tokens\n(12.5%)\n",
        "System prompt: 12.4k tokens (1.2%)\nSystem tools: 18.2k tokens (1.7%)\n",
        "Skills: 2.1k tokens (0.2%)\n  SKILL.md: 24 tokens\n",
        "User messages: 1.4k tokens (0.1%)\nAgent responses: 42.1k tokens (4.0%)\n",
        "Tool calls: 54.5k tokens (5.2%)\nSystem messages: 0 tokens (0.0%)\n",
        "Subagents: 0 tokens (0.0%)\nFree space: 700.1k tokens (66.7%)\n",
        "Checkpoint buffer: 169.2k tokens (16.1%)\n↑/↓ Scroll · Esc Close",
    )
    .to_owned()
}

fn make_local_agy_job(ws: &Path, telem: SessionTelemetry) -> SessionPollJob {
    make_poll_job(1, ptr::null_mut(), &ws.to_string_lossy(), false, "AGY", telem)
}

#[test]
fn syncs_multiline_context_panel_and_persists_until_jsonl_advances() {
    let base = temp_dir("ctx_panel");
    let home = base.join("home");
    let ws = home.join("Productivity");
    fs::create_dir_all(&ws).unwrap();
    let init = "{\"type\":\"USER_INPUT\",\"content\":\"~/Productivity/ДОСКА.md\"}\n{\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":8000,\"cache_read_tokens\":118000,\"output_tokens\":500}\n";
    let log = write_brain_log(&home.join(".gemini/antigravity-cli/brain"), "s1", init, 10);

    let (_, mut telem) = run_poll_job(make_local_agy_job(&ws, SessionTelemetry::default()), &home, Instant::now());
    assert_eq!(telem.snapshot().context_tokens, 126_500);
    assert_panel_capture(&mut telem);
    assert_panel_close_and_advance(&home, &ws, &log, telem);
    let _ = fs::remove_dir_all(&base);
}

fn assert_panel_capture(telem: &mut SessionTelemetry) {
    telem.observe_vte_text(&sample_agy_context_screen());
    assert_eq!(telem.snapshot().context_tokens, 130_700);
    assert_eq!(telem.snapshot().context_pct, Some(13));
    assert_eq!(telem.snapshot().compact_context_badge().as_deref(), Some("ctx 131k"));
    assert_eq!(telem.snapshot().input_tokens, 8_000);
    assert_eq!(telem.snapshot().source, TelemetrySource::Agy);
}

fn assert_panel_close_and_advance(home: &Path, ws: &Path, log: &Path, mut telem: SessionTelemetry) {
    telem.observe_vte_text("agy > ready for next prompt\n");
    let (_, closed) = run_poll_job(make_local_agy_job(ws, telem), home, Instant::now());
    assert_eq!(closed.snapshot().context_tokens, 130_700);
    assert_eq!(closed.snapshot().compact_context_badge().as_deref(), Some("ctx 131k"));

    let mut f = fs::OpenOptions::new().append(true).open(log).unwrap();
    writeln!(f, "{{\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":9000,\"cache_read_tokens\":130000,\"output_tokens\":800}}").unwrap();
    let (_, advanced) = run_poll_job(make_local_agy_job(ws, closed), home, Instant::now());
    assert!(advanced.vte_panel_override.is_none());
    assert_eq!(advanced.snapshot().context_tokens, 139_800);
}

#[test]
fn guards_status_fallback_and_panel_against_breakdown_lines() {
    let screen = sample_agy_context_screen();
    assert_eq!(parse_vte_context_panel(&screen), Some((130_700, Some(13))));
    assert!(parse_vte_status_fallback(&screen).is_none());

    let tail_only = "  SKILL.md: 24 tokens\nFree space: 700.1k tokens\nCheckpoint buffer: 169.2k tokens\n↑/↓ Scroll\n";
    assert!(parse_vte_context_panel(tail_only).is_none());
    assert!(parse_vte_status_fallback(tail_only).is_none());

    let no_pct = "\x1b[1mModel\x1b[0m · \x1b[36m85k/200k\x1b[0m токенов\nSystem prompt: 8.2k tokens (0.8%)\n";
    assert_eq!(parse_vte_context_panel(no_pct), Some((85_000, Some(43))));
}

fn make_remote_agy_tailer(offset_body: &str) -> crate::remote_telemetry::RemoteTailer {
    let mut rt = crate::remote_telemetry::RemoteTailer::default();
    let bytes = offset_body.as_bytes().to_vec();
    rt.apply_frame(&crate::remote_telemetry::RemoteProbeFrame {
        sid: 5,
        mode: crate::remote_telemetry::FrameMode::Full,
        kind: TelemetrySource::Agy,
        size: bytes.len() as u64,
        mtime: 1,
        path: "/remote/.gemini/antigravity-cli/brain/r1/.system_generated/logs/transcript.jsonl".into(),
        head: Vec::new(),
        body: bytes,
    });
    rt
}

fn make_remote_agy_job(telem: SessionTelemetry) -> SessionPollJob {
    SessionPollJob {
        sid: 5, root_pid: 0, vte: ptr::null_mut(), local_cwd: None,
        has_ssh: true, has_tmux: true, tmux_window: None, agent: "AGY".to_owned(), is_daemon: false, telemetry: telem,
    }
}

#[test]
fn honors_vte_context_panel_on_remote_tailer_until_remote_offset_advances() {
    let b1 = "{\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":8000,\"cache_read_tokens\":118000,\"output_tokens\":500}\n";
    let rt1 = make_remote_agy_tailer(b1);
    let (_, mut telem) = run_poll_job_with_remote(make_remote_agy_job(SessionTelemetry::default()), Some(rt1.clone()), Path::new("/tmp"), Instant::now());
    assert_eq!(telem.snapshot().context_tokens, 126_500);
    telem.observe_vte_text(&sample_agy_context_screen());
    assert_eq!((telem.snapshot().context_tokens, telem.detected_agent_label()), (130_700, Some("AGY")));

    let (_, kept) = run_poll_job_with_remote(make_remote_agy_job(telem), Some(rt1), Path::new("/tmp"), Instant::now());
    assert_eq!(kept.snapshot().context_tokens, 130_700);
    let b2 = format!("{b1}{{\"type\":\"PLANNER_RESPONSE\",\"input_tokens\":9000,\"cache_read_tokens\":130000,\"output_tokens\":800}}\n");
    let (_, next) = run_poll_job_with_remote(make_remote_agy_job(kept), Some(make_remote_agy_tailer(&b2)), Path::new("/tmp"), Instant::now());
    assert_eq!((next.vte_panel_override, next.snapshot().context_tokens), (None, 139_800));
}

fn write_timed_codex_rollout(path: &Path, body: &str, plus_secs: u64) {
    fs::write(path, body).unwrap();
    let stamp = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_762_100_000 + plus_secs);
    fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(stamp).unwrap();
}

fn codex_large_header_main(repo: &Path) -> String {
    let pad = "z".repeat(64_000);
    format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\",\"source\":\"cli\",\"base_instructions\":\"subagent {pad}\"}}}}\n",
        repo.display()
    )
}

fn seed_codex_large_header_fixture(day: &Path, repo: &Path) -> PathBuf {
    let main_hdr = codex_large_header_main(repo);
    let tok = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":154000,\"cached_input_tokens\":120000,\"output_tokens\":800,\"total_tokens\":154800},\"last_token_usage\":{\"input_tokens\":154000,\"cached_input_tokens\":120000,\"output_tokens\":800,\"total_tokens\":154800},\"model_context_window\":258400}}}\n";
    let main_path = day.join("rollout-main.jsonl");
    write_timed_codex_rollout(&main_path, &format!("{main_hdr}{tok}"), 10);
    let sub_body = format!("{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\",\"source\":{{\"subagent\":{{\"other\":\"guardian\"}}}}}}}}\n{tok}", repo.display());
    write_timed_codex_rollout(&day.join("rollout-sub.jsonl"), &sub_body, 50);
    write_timed_codex_rollout(&day.join("rollout-empty.jsonl"), &main_hdr, 80);
    main_path
}

#[test]
fn discovers_local_codex_large_header_ignoring_subagents_and_empty_rollouts() {
    let base = temp_dir("codex_large_hdr");
    let (home, repo) = (base.join("home"), base.join("home/Development/Akhcheck"));
    let day = home.join(".codex/sessions/2026/11/03");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&day).unwrap();
    let main_path = seed_codex_large_header_fixture(&day, &repo);
    let (found, kind) = discover_transcript(&home, &repo.to_string_lossy(), 0, "Codex").unwrap();
    assert_eq!((found, kind), (main_path, TelemetrySource::Codex));
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn maintainability_index_stays_above_targets() {
    let targets = [
        ("telemetry.rs", include_str!("telemetry.rs"), 60.5),
        ("jsonl_parse.rs", include_str!("jsonl_parse.rs"), 60.5),
        ("tokens_ui.rs", include_str!("tokens_ui.rs"), 60.5),
        ("telemetry_tests.rs", include_str!("telemetry_tests.rs"), 60.5),
        ("telemetry_agy.rs", include_str!("telemetry_agy.rs"), 62.0),
        ("main.rs", include_str!("../main.rs"), 55.0),
    ];
    for (name, src, min_mi) in targets {
        let (_, _, mi) = crate::mi::compute_file_mi(src);
        assert!(mi >= min_mi, "{name} MI {mi:.2} must be >= {min_mi:.1}");
    }
}
