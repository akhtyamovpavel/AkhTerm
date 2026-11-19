use super::*;
use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn sample_alpha_hunks() -> &'static str {
    concat!(
        "diff --git a/src/alpha.rs b/src/alpha.rs\nindex 1111111..2222222 100644\n",
        "--- a/src/alpha.rs\n+++ b/src/alpha.rs\n@@ -1,4 +1,5 @@ fn main()\n",
        " fn main() {\n-    let old = 1;\n+    let new = 2;\n+    let extra = 3;\n",
        "     println!(\"{new}\");\n }\n",
        "@@ -20,3 +21,2 @@ fn tail()\n fn tail() {\n-    drop_me();\n }\n",
    )
}

fn sample_added_and_binary_diff() -> &'static str {
    concat!(
        "diff --git a/src/added.rs b/src/added.rs\nnew file mode 100644\n",
        "index 0000000..3333333\n--- /dev/null\n+++ b/src/added.rs\n",
        "@@ -0,0 +1,2 @@\n+pub fn hello() {}\n+// end\n\\ No newline at end of file\n",
        "diff --git a/assets/icon.png b/assets/icon.png\nindex 4444444..5555555\n",
        "Binary files a/assets/icon.png and b/assets/icon.png differ\n",
    )
}

fn sample_diff() -> String {
    format!("{}{}", sample_alpha_hunks(), sample_added_and_binary_diff())
}

#[test]
fn parses_multi_file_unified_diff_counts_and_line_numbers() {
    let diff = sample_diff();
    let files = parser::parse_unified_diff(&diff);
    assert_eq!(files.len(), 3);
    assert_eq!(files[0].display_path, "src/alpha.rs");
    assert_eq!((files[0].insertions, files[0].deletions), (2, 2));
    assert_eq!(files[0].hunks.len(), 2);
    assert_eq!(files[0].hunks[0].lines[1].old_line, Some(2));
    assert_eq!(files[0].hunks[0].lines[2].new_line, Some(2));
    assert_eq!(files[0].hunks[1].old_start, 20);
    assert!(files[1].is_new && files[1].insertions == 2 && files[2].is_binary);
}

fn cyrillic_octal_diff() -> &'static str {
    concat!(
        "diff --git \"a/\\320\\274\\320\\276\\320\\264\\321\\203\\320\\273\\321\\214/\\321\\204\\320\\260\\320\\271\\320\\273.rs\" ",
        "\"b/\\320\\274\\320\\276\\320\\264\\321\\203\\320\\273\\321\\214/\\321\\204\\320\\260\\320\\271\\320\\273.rs\"\n",
        "--- \"a/\\320\\274\\320\\276\\320\\264\\321\\203\\320\\273\\321\\214/\\321\\204\\320\\260\\320\\271\\320\\273.rs\"\n",
        "+++ \"b/\\320\\274\\320\\276\\320\\264\\321\\203\\320\\273\\321\\214/\\321\\204\\320\\260\\320\\271\\320\\273.rs\"\n",
        "@@ -1 +1 @@\n-привет\n+мир\n",
        "diff --git a/доки/новый план.rs b/доки/новый план.rs\n",
        "--- a/доки/новый план.rs\n+++ b/доки/новый план.rs\n@@ -1 +1 @@\n-а\n+б\n",
    )
}

#[test]
fn parses_cyrillic_and_git_octal_quoted_paths() {
    let files = parser::parse_unified_diff(cyrillic_octal_diff());
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].display_path, "модуль/файл.rs");
    assert_eq!(files[1].display_path, "доки/новый план.rs");
}

#[test]
fn reconstructs_single_hunk_patch_and_preserves_no_newline() {
    let diff = sample_diff();
    let files = parser::parse_unified_diff(&diff);
    let second = parser::reconstruct_hunk_patch(&files[0], 1).unwrap();
    assert!(second.contains("@@ -20,3 +21,2 @@"));
    assert!(!second.contains("let new = 2;"));
    let added = parser::reconstruct_hunk_patch(&files[1], 0).unwrap();
    assert!(added.contains("new file mode"));
    assert!(added.contains("\\ No newline at end of file\n"));
    assert!(parser::reconstruct_hunk_patch(&files[2], 0).is_none());
}

fn make_comment(
    file_path: &str,
    lines: (u32, u32),
    excerpt: &str,
    comment: &str,
    remote_host: Option<&str>,
) -> ReviewComment {
    ReviewComment {
        file_path: file_path.into(),
        start_line: lines.0,
        end_line: lines.1,
        excerpt: excerpt.into(),
        comment: comment.into(),
        remote_host: remote_host.map(Into::into),
    }
}

fn sample_review_comments() -> Vec<ReviewComment> {
    vec![
        make_comment(
            "src/модуль.rs",
            (10, 12),
            "+let fence = \"```\";\n+run();",
            "Check error handling",
            Some("gpu01"),
        ),
        make_comment("src/alpha.rs", (5, 5), "-old()", "Keep fallback", None),
    ]
}

#[test]
fn formats_structured_markdown_review_batch_with_nested_fences_and_safety() {
    let comments = sample_review_comments();
    let md = parser::format_review_batch(&comments, Some("working tree")).unwrap();
    assert!(md.contains("1. `gpu01:src/модуль.rs:L10-L12`"));
    assert!(md.contains("````diff\n+let fence = \"```\";\n+run();\n````\nCheck error handling"));
    assert!(md.contains("2. `src/alpha.rs:L5`"));
    assert!(parser::format_review_batch(&[], None).is_none());
    let unsafe_item = ReviewComment { comment: "bad\x1b[31m".into(), ..comments[1].clone() };
    assert!(parser::format_review_batch(&[unsafe_item], None).is_none());
}

fn assert_diff_cap_truncation() {
    let exact = vec![b'a'; source::MAX_DIFF_BYTES];
    let (text_ok, trunc_ok) = source::read_capped_diff(exact.as_slice()).unwrap();
    assert!(!trunc_ok && text_ok.len() == source::MAX_DIFF_BYTES);
    let mut big = vec![b'x'; source::MAX_DIFF_BYTES + 64];
    big[source::MAX_DIFF_BYTES - 10] = b'\n';
    let (trimmed, trunc_big) = source::read_capped_diff(big.as_slice()).unwrap();
    assert!(trunc_big && trimmed.ends_with('\n') && trimmed.len() <= source::MAX_DIFF_BYTES);
}

#[test]
fn diff_source_enforces_2mib_safety_cap_and_builds_safe_ssh_scripts() {
    assert_diff_cap_truncation();
    let (clean, _) = source::read_capped_diff(&b"ok\0text\n"[..]).unwrap();
    assert_eq!(clean, "oktext\n");
    let script = source::build_remote_diff_script("/srv/my 'repo'", DiffMode::Branch);
    assert!(script.contains("head -c 2097153"));
    assert!(script.contains("merge-base"));
    assert!(script.contains("'/srv/my '\\''repo'\\'''"));
}

struct TempRepo(PathBuf);

impl TempRepo {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("akhterm-diff-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new("git").arg("-C").arg(&self.0).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn init_main(&self) {
        self.git(&["init", "-b", "main"]);
        self.git(&["config", "user.email", "test@example.com"]);
        self.git(&["config", "user.name", "Test"]);
    }

    fn commit_file(&self, name: &str, content: &str, msg: &str) {
        fs::write(self.0.join(name), content).unwrap();
        self.git(&["add", name]);
        self.git(&["commit", "-m", msg]);
    }

    fn load_parsed(&self, mode: DiffMode) -> Vec<DiffFile> {
        let raw = source::load_local_diff(&self.0, mode).unwrap().raw_diff;
        parser::parse_unified_diff(&raw)
    }

    fn apply_first_hunk(&self, file: &DiffFile, stage: bool) {
        let patch = parser::reconstruct_hunk_patch(file, 0).unwrap();
        if stage {
            source::stage_hunk(&self.0, &patch).unwrap();
        } else {
            source::revert_hunk(&self.0, &patch).unwrap();
        }
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn init_test_git_repo(repo: &TempRepo) {
    repo.init_main();
    let base: String = (1..=20).map(|i| format!("line_{i}\n")).collect();
    repo.commit_file("модуль.txt", &base, "init");
    repo.git(&["checkout", "-b", "feat"]);
    repo.commit_file("branch.txt", "branch commit\n", "branch change");
    let edited = base
        .replace("line_2\n", "line_2_mod\n")
        .replace("line_18\n", "line_18_mod\n");
    fs::write(repo.0.join("модуль.txt"), edited).unwrap();
}

#[test]
fn real_git_repo_loads_diffs_and_stages_reverts_single_hunk() {
    let repo = TempRepo::new("hunks");
    init_test_git_repo(&repo);
    let files = repo.load_parsed(DiffMode::WorkingTree);
    assert_eq!((files.len(), files[0].display_path.as_str(), files[0].hunks.len()), (1, "модуль.txt", 2));
    repo.apply_first_hunk(&files[0], true);
    let remaining = repo.load_parsed(DiffMode::WorkingTree);
    assert_eq!((remaining[0].hunks.len(), repo.load_parsed(DiffMode::Staged)[0].hunks.len()), (1, 1));
    repo.apply_first_hunk(&remaining[0], false);
    assert!(repo.load_parsed(DiffMode::WorkingTree).is_empty());
    let branch_diff = repo.load_parsed(DiffMode::Branch);
    assert_eq!((branch_diff.len(), branch_diff[0].display_path.as_str()), (1, "branch.txt"));
}

fn wait_for_opencode_target(vte: *mut GtkWidget) -> bool {
    for _ in 0..80 {
        pump_gtk_events();
        app_state::with_app_mut(|app| {
            let s = &mut app.sessions[0];
            crate::session_ops::sync_session_proc(s, None);
            crate::pane_radar::sync_registry_procs(&mut s.pane_registry);
        });
        if markdown::targets().into_iter().any(|t| t.vte == vte) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

fn wait_for_vte_contains(vte: *mut GtkWidget, needle: &str) -> bool {
    for _ in 0..80 {
        pump_gtk_events();
        if crate::handoff::extract_vte_text(vte).contains(needle) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

fn setup_panel_for_headless_test() -> (*mut GtkWidget, usize, *mut GtkWidget) {
    let (win, _) = crate::tests::install_test_window();
    let host_box = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0) };
    let term_box = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 0) };
    mount(host_box, term_box);
    let root_child = unsafe { gtk_bin_get_child(win) };
    unsafe { gtk_box_pack_start(root_child, host_box, 1, 1, 0) };
    let sid = crate::session_ops::add_tab(
        "Agent", "", "/tmp", crate::session::SessionCategory::Project,
        &["bash", "--noprofile", "--norc", "-i"], crate::tabs::Tab::local("shell"),
    );
    let vte = app_state::with_app(|app| app.sessions[0].vte).unwrap();
    unsafe { gtk_widget_show_all(win) };
    (win, sid, vte)
}

fn assert_hunk_buttons_sensitive(p: &DiffReviewPanel, expected: bool) {
    let want = i32::from(expected);
    assert_eq!(unsafe { gtk_widget_get_sensitive(p.stage_btn) }, want);
    assert_eq!(unsafe { gtk_widget_get_sensitive(p.revert_btn) }, want);
}

fn panel_is_visible() -> bool {
    PANEL.with_borrow(|slot| unsafe { gtk_widget_get_visible(slot.as_ref().unwrap().root) != 0 })
}

fn load_sample_into_panel(target: DiffTarget, scope: &str, request_id: Option<u64>) {
    let raw_diff = sample_diff();
    PANEL.with_borrow_mut(|slot| {
        let p = slot.as_mut().unwrap();
        p.target = Some(target);
        p.mode = DiffMode::WorkingTree;
        p.request_id = request_id;
        p.apply_loaded_result(DiffLoadResult {
            raw_diff,
            truncated: false,
            scope_label: scope.into(),
        });
        p.show_panel();
    });
}

fn verify_local_file_switch_and_review() {
    PANEL.with_borrow_mut(|slot| {
        let p = slot.as_mut().unwrap();
        assert_eq!(p.files.len(), 3);
        assert_hunk_buttons_sensitive(p, true);
        p.select_file(1);
        assert_eq!(p.selected_file, 1);
        p.reviewed_files.insert("src/added.rs".into());
        p.refresh_meta();
    });
}

fn verify_branch_mode_disables_hunks() {
    PANEL.with_borrow_mut(|slot| {
        let p = slot.as_mut().unwrap();
        p.mode = DiffMode::Branch;
        p.refresh_hunk_controls();
        assert_hunk_buttons_sensitive(p, false);
        p.mode = DiffMode::WorkingTree;
        p.select_file(0);
    });
}

fn verify_panel_switch_and_sensitivity() {
    let local = DiffTarget::Local {
        repo_root: PathBuf::from("/tmp"),
        label: "repo".into(),
        origin: None,
    };
    load_sample_into_panel(local, "working tree", None);
    assert!(panel_is_visible());
    verify_local_file_switch_and_review();
    verify_branch_mode_disables_hunks();
}

fn spawn_mock_opencode(vte: *mut GtkWidget) {
    let cmd = "bash -c 'exec -a opencode bash -c \"$1\"' stub 'printf \"\\033]0;OpenCode\\007\"; while IFS= read -r l; do printf \"%s\\n\" \"$l\"; done'\n";
    crate::file_drop::feed_vte_text(vte, cmd, false);
    assert!(wait_for_opencode_target(vte), "expected live OpenCode target");
}

fn queue_comment_and_verify_badge(comment: &CStr) {
    PANEL.with_borrow_mut(|slot| {
        let p = slot.as_mut().unwrap();
        p.picker.reset(None);
        unsafe { gtk_entry_set_text(p.comment_entry, comment.as_ptr()) };
        p.queue_current_comment().unwrap();
        assert_eq!(p.comments.len(), 1);
        let badge = unsafe { CStr::from_ptr(gtk_label_get_text(p.queue_badge)) }.to_string_lossy();
        assert_eq!(badge, "1");
    });
}

fn send_batch_and_verify_vte(vte: *mut GtkWidget, expected: &str) {
    on_send_batch(ptr::null_mut(), ptr::null_mut());
    PANEL.with_borrow(|slot| assert!(slot.as_ref().unwrap().comments.is_empty()));
    assert!(wait_for_vte_contains(vte, expected), "expected review batch in VTE");
    close_panel();
    assert!(!panel_is_visible());
}

fn assert_gtk_ran_if_required(ran: bool) {
    if std::env::var_os("AKHTERM_REQUIRE_GTK").is_some() {
        assert!(ran, "GTK display required");
    }
}

#[test]
fn headless_broadway_panel_open_switch_queue_and_quote_delivery() {
    let mut ran = false;
    with_headless_gtk(|| {
        ran = true;
        let (win, sid, vte) = setup_panel_for_headless_test();
        verify_panel_switch_and_sensitivity();
        spawn_mock_opencode(vte);
        queue_comment_and_verify_badge(c"Fix variable name");
        send_batch_and_verify_vte(vte, "Fix variable name");
        crate::tests::cleanup_test_window(win, &[sid]);
        assert!(PANEL.with_borrow(Option::is_none));
    });
    assert_gtk_ran_if_required(ran);
}

extern "C" {
    fn gtk_bin_get_child(bin: *mut GtkWidget) -> *mut GtkWidget;
}

fn mount_isolated_panel_window() -> *mut GtkWidget {
    unsafe {
        let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        let parent = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
        gtk_container_add(win, parent);
        mount(parent, gtk_box_new(GTK_ORIENTATION_VERTICAL, 0));
        win
    }
}

fn empty_diff_result(scope: &str) -> DiffLoadResult {
    DiffLoadResult {
        raw_diff: String::new(),
        truncated: false,
        scope_label: scope.into(),
    }
}

#[test]
fn stale_async_diff_cannot_overwrite_or_reopen_closed_panel() {
    with_headless_gtk(|| {
        let win = mount_isolated_panel_window();
        let route = crate::telemetry::ssh_connection::SshRoute::for_host("devbox").unwrap();
        let remote = DiffTarget::Remote { host: "devbox".into(), cwd: "/srv/app".into(), route, origin: None };
        load_sample_into_panel(remote, "devbox", Some(42));
        PANEL.with_borrow(|slot| assert_hunk_buttons_sensitive(slot.as_ref().unwrap(), false));
        finish_load(41, Ok(empty_diff_result("stale")));
        PANEL.with_borrow(|slot| assert_eq!(slot.as_ref().unwrap().files.len(), 3));
        close_panel();
        finish_load(42, Ok(empty_diff_result("late")));
        assert!(!panel_is_visible());
        unsafe { gtk_widget_destroy(win) };
    });
}

fn diff_ui_text_entries() -> [text::UiText; 30] {
    [
        text::DIFF_OPEN_BTN, text::DIFF_OPEN_TIP, text::DIFF_MODE_WORKTREE,
        text::DIFF_MODE_STAGED, text::DIFF_MODE_BRANCH, text::DIFF_REFRESH,
        text::DIFF_CLOSE, text::DIFF_FILES_HDR, text::DIFF_REVIEWED,
        text::DIFF_HUNK_PREV, text::DIFF_HUNK_NEXT, text::DIFF_HUNK_PREV_TIP,
        text::DIFF_HUNK_NEXT_TIP, text::DIFF_STAGE_HUNK, text::DIFF_REVERT_HUNK,
        text::DIFF_HUNK_LOCAL_ONLY, text::DIFF_COMMENT_PLACEHOLDER, text::DIFF_ADD_COMMENT,
        text::DIFF_CLEAR_BATCH, text::DIFF_SEND_BATCH, text::DIFF_QUEUE_EMPTY,
        text::DIFF_NEED_COMMENT, text::DIFF_NO_CHANGES, text::DIFF_LOADING,
        text::DIFF_TRUNCATED, text::DIFF_SENT_OK, text::DIFF_HUNK_STAGED,
        text::DIFF_HUNK_REVERTED, text::DIFF_PALETTE_TITLE, text::DIFF_PALETTE_DETAIL,
    ]
}

fn assert_clean_bilingual_entry(entry: text::UiText) {
    for loc in [text::UiLocale::Ru, text::UiLocale::En] {
        let s = entry.for_locale(loc).to_string_lossy();
        assert!(!s.trim().is_empty(), "empty diff ui_text");
        for arrow in ["->", "→", "=>"] {
            assert!(!s.contains(arrow), "arrow {arrow:?} forbidden in UI string: {s}");
        }
    }
}

#[test]
fn bilingual_ui_text_has_no_arrows_and_shortcut_matches_ctrl_shift_b() {
    for entry in diff_ui_text_entries() {
        assert_clean_bilingual_entry(entry);
    }
    let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
    assert!(is_diff_review_shortcut('B' as u32, cs, 56));
    assert!(is_diff_review_shortcut('И' as u32, cs, 56));
    assert!(!is_diff_review_shortcut('b' as u32, GDK_CONTROL_MASK, 56));
    assert!(!is_diff_review_shortcut('B' as u32, cs | GDK_MOD1_MASK, 56));
}

#[test]
fn palette_registers_diff_review_action_for_ru_and_en() {
    let src = crate::palette::PaletteSource {
        sessions: &[],
        tmux_items: &[],
        profiles: &[],
        ssh_hosts: &[],
        briefs_dir: None,
        extra_handoffs: &[],
        active_git: None,
        active_is_worktree: false,
    };
    for loc in [text::UiLocale::Ru, text::UiLocale::En] {
        let items = crate::palette::collect_items_for_locale(&src, loc);
        assert!(items.iter().any(|it| it.action == crate::palette::PaletteAction::OpenDiffReview));
    }
}

#[test]
fn diff_review_modules_meet_maintainability_index_gates() {
    for rel in [
        "src/ui/diff_parser.rs",
        "src/ui/diff_source.rs",
        "src/ui/diff_buffer.rs",
        "src/ui/diff_review.rs",
    ] {
        let src = fs::read_to_string(rel).expect("read diff module");
        let (sloc, cc, score) = crate::mi::compute_file_mi(&src);
        let grade = crate::mi::grade_for_mi(score);
        println!("{rel:<30} | SLOC {sloc:>4} | CC {cc:>2} | MI {score:>6.2} | Grade {grade}");
        assert!(score >= 60.0, "{rel} MI {score:.2} < 60.0");
    }
}

#[test]
fn active_hunk_at_offset_targets_exact_hunk_after_jump() {
    let diff = sample_diff();
    let files = parser::parse_unified_diff(&diff);
    let rendered = parser::render_file_diff(&files[0]);
    let off0 = parser::hunk_start_char(&rendered, 0).unwrap();
    let off1 = parser::hunk_start_char(&rendered, 1).unwrap();
    assert_eq!(parser::active_hunk_at_offset(&rendered, off0), Some(0));
    assert_eq!(parser::active_hunk_at_offset(&rendered, off1), Some(1));
    let excerpt = parser::extract_selection_excerpt(&rendered, off1, off1, 0).unwrap();
    assert_eq!(excerpt.hunk_idx, 1);
    assert!(excerpt.excerpt.contains("drop_me()"));
    assert!(!excerpt.excerpt.contains("let new = 2;"));
}

#[test]
fn real_git_repo_truncates_over_2mib_diff_without_sigpipe_error() {
    let repo = TempRepo::new("oversized");
    repo.init_main();
    repo.commit_file("big.txt", "initial\n", "init");
    let line = "x".repeat(100) + "\n";
    fs::write(repo.0.join("big.txt"), line.repeat(26_000)).unwrap();
    let res = source::load_local_diff(&repo.0, DiffMode::WorkingTree).unwrap();
    assert!(res.truncated);
    assert!(res.raw_diff.len() <= source::MAX_DIFF_BYTES);
    assert!(res.raw_diff.ends_with('\n'));
}

#[test]
fn remote_branch_script_fails_cleanly_in_non_git_directory() {
    let plain = TempRepo::new("nongit");
    let script = source::build_remote_diff_script(plain.0.to_str().unwrap(), DiffMode::Branch);
    let out = Command::new("sh").args(["-c", &script]).output().unwrap();
    assert!(!out.status.success(), "expected non-zero exit in non-git dir");
    assert!(out.stdout.is_empty(), "expected no leaked stdout: {}", String::from_utf8_lossy(&out.stdout));
}
