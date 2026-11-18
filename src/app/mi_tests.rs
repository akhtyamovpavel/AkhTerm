//! Directory changes and visible MI state in real Bash and tmux terminals.

use super::*;
use std::{fs, path::PathBuf, process::Command};

#[test]
fn cd_updates_mi_button_before_and_after_the_new_directory_is_scanned() {
    verify_cd_updates_mi(false);
}

#[test]
fn cd_updates_mi_inside_a_local_tmux_pane() {
    if std::env::var_os("AKHTERM_TEST_MI_TMUX").is_some() {
        verify_cd_updates_mi(true);
        return;
    }
    if Command::new("tmux").arg("-V").output().is_err() { return; }
    run_isolated_tmux_test();
}

fn run_isolated_tmux_test() {
    let test_name = "app::app_tests::mi_tests::cd_updates_mi_inside_a_local_tmux_pane";
    let tmp = std::env::temp_dir().join(format!("akhterm-mi-tmux-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture"])
        .env_remove("TMUX").env("TMUX_TMPDIR", &tmp).env("AKHTERM_TEST_MI_TMUX", "1")
        .status().unwrap();
    let _ = Command::new("tmux").arg("kill-server").env_remove("TMUX").env("TMUX_TMPDIR", &tmp).output();
    fs::remove_dir_all(tmp).unwrap();
    assert!(status.success());
}

fn create_mi_projects() -> (PathBuf, PathBuf) {
    let project = std::env::temp_dir().join(format!("akhterm-mi-cd-{}", std::process::id()));
    let empty = project.join("отдельный проект");
    fs::create_dir_all(&empty).unwrap();
    fs::write(project.join("Cargo.toml"), "[package]\n").unwrap();
    fs::write(empty.join("Cargo.toml"), "[package]\n").unwrap();
    fs::write(project.join("main.rs"), "fn main() {\n    let message = \"this file is long enough to analyze\";\n    println!(\"{message}\");\n}\n").unwrap();
    (project, empty)
}

fn open_mi_terminal(dir: &str, in_tmux: bool) -> usize {
    let argv: &[&str] = if in_tmux {
        &["tmux", "-f", "/dev/null", "new-session", "-s", "mi-cd", "bash --noprofile --norc -i"]
    } else {
        &["bash", "--noprofile", "--norc", "-i"]
    };
    session_ops::add_tab("Shell", "", dir, SessionCategory::Project, argv, Tab::local("shell"))
}

fn mi_controls(dir: &str) -> (*mut GtkWidget, *mut GtkWidget, mi_watch::MiScheduler) {
    app_state::with_app_mut(|app| {
        let button = unsafe { gtk_button_new() };
        unsafe { gtk_container_add(gtk_widget_get_parent(app.mi_btn), button) };
        app.mi_btn = button;
        app.config.mi_root = Some(dir.to_owned());
        (app.sessions[0].vte, button, app.mi_scheduler.clone())
    }).unwrap()
}

fn verify_cd_updates_mi(in_tmux: bool) {
    with_headless_gtk(|| {
        let (project, empty) = create_mi_projects();
        let dir = project.to_str().unwrap();
        let (win, _) = install_test_window();
        let sid = open_mi_terminal(dir, in_tmux);
        let (vte, button, scheduler) = mi_controls(dir);
        unsafe { gtk_widget_show_all(win) };
        wait_for_mi_cwd(dir, in_tmux);
        topbar::refresh_ui();
        scheduler.refresh_due(std::time::Instant::now());
        topbar::refresh_ui();
        let label = || unsafe { CStr::from_ptr(gtk_button_get_label(button)).to_string_lossy().into_owned() };
        assert!(label().starts_with("MI ") && label().contains('['));
        let cd = |command: &str, target: &str| {
            file_drop::feed_vte_text(vte, command, false);
            wait_for_mi_cwd(target, in_tmux);
            (session_ops::radar_tick_cb())(ptr::null_mut());
            assert_eq!(label(), crate::ui_text::MI_CALCULATING.as_str());
            scheduler.refresh_due(std::time::Instant::now());
            (topbar::mi_ready_cb())(ptr::null_mut());
        };
        cd("cd 'отдельный проект'\n", empty.to_str().unwrap());
        assert_eq!(label(), crate::ui_text::MI_NO_FILES.as_str());
        cd("cd ..\n", dir);
        assert!(label().contains('['));
        cleanup_test_window(win, &[sid]);
        fs::remove_dir_all(project).unwrap();
    });
}

fn wait_for_mi_cwd(dir: &str, in_tmux: bool) {
    for _ in 0..100 {
        pump_gtk_events();
        let matched = app_state::with_app_mut(|app| {
            let s = &mut app.sessions[0];
            session_ops::sync_session_proc(s, None);
            (!in_tmux || s.proc_info.active_pane_pid > 0) && s.active_local_cwd().as_deref() == Some(dir)
        });
        if matched == Some(true) { return; }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("active local pane did not report {dir}");
}
