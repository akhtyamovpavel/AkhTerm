//! Application wiring between `sys::updater`, `ui::update_banner`, and GTK lifecycle.

use std::ffi::{c_int, c_void};
use std::rc::Rc;

use crate::app_state;
use crate::ffi::*;
use crate::sys::updater::{self, GSourceFunc, UpdateState};
use crate::ui::about;
use crate::ui::ui_text;
use crate::ui::update_banner::{UpdateBanner, UpdateBannerCallbacks, UpdateBannerModel};
use crate::{proc_radar, session_ops};

pub(crate) fn init_hooks() {
    about::register_check_updates_hook(trigger_manual_check);
}

pub(crate) fn finish_pending_restart() -> std::process::ExitCode {
    if let Some(exe) = updater::take_pending_restart_exe() {
        let err = updater::exec_replace_process(&exe);
        eprintln!("AkhTerm: restart failed: {err}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

pub(crate) fn wrap_stack_overlay(parent_vbox: *mut GtkWidget, stack: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let overlay = gtk_overlay_new();
        gtk_container_add(overlay, stack);
        gtk_box_pack_start(parent_vbox, overlay, 1, 1, 0);
        overlay
    }
}

pub(crate) fn build_update_banner() -> UpdateBanner {
    UpdateBanner::new(UpdateBannerCallbacks {
        on_update: Rc::new(on_update_clicked),
        on_restart: Rc::new(on_restart_clicked),
        on_dismiss: Rc::new(on_dismiss_clicked),
        on_whats_new: Rc::new(proc_radar::open_external_link),
    })
}

pub(crate) fn trigger_startup_check() {
    let custom_url = std::env::var_os("AKHTERM_UPDATE_MANIFEST_URL").is_some();
    let in_broadway = std::env::var("GDK_BACKEND").is_ok_and(|v| v == "broadway");
    if !custom_url && in_broadway {
        return;
    }
    start_check(false);
}

pub(crate) fn trigger_manual_check() {
    start_check(true);
}

fn start_check(manual: bool) {
    let ver = env!("CARGO_PKG_VERSION");
    let cb = updater_ready_cb();
    let _ = app_state::with_app(|app| app.update_scheduler.check_async(ver, manual, Some(cb)));
}

fn on_update_clicked() {
    let cb = updater_ready_cb();
    let _ = app_state::with_app(|app| app.update_scheduler.install_async(Some(cb)));
}

fn on_dismiss_clicked() {
    let _ = app_state::with_app(|app| {
        app.update_scheduler.dismiss_current();
        app.update_banner.apply_model(&UpdateBannerModel::Hidden);
    });
}

fn on_restart_clicked() {
    session_ops::save_tabs();
    let restart_info = app_state::with_app(|app| {
        let exe = match app.update_scheduler.state() {
            UpdateState::ReadyToRestart { exe_path, .. } => Some(exe_path),
            _ => None,
        };
        (exe, app.win)
    });
    let Some((exe_opt, win)) = restart_info else {
        return;
    };
    if let Some(exe) = exe_opt {
        updater::request_restart_exe(exe);
    }
    close_window_for_restart(win);
}

fn close_window_for_restart(win: *mut GtkWidget) {
    unsafe {
        if !win.is_null() {
            gtk_widget_destroy(win);
        } else {
            gtk_main_quit();
        }
    }
}

pub(crate) fn updater_ready_cb() -> GSourceFunc {
    on_updater_ready
}

extern "C" fn on_updater_ready(_data: *mut c_void) -> c_int {
    sync_updater_ui();
    0
}

pub(crate) fn sync_updater_ui() {
    let snap = app_state::with_app(|app| (app.update_scheduler.state(), app.update_banner.clone()));
    let Some((state, banner)) = snap else {
        return;
    };
    if let UpdateState::UpToDate { current_version } = state {
        banner.apply_model(&UpdateBannerModel::Hidden);
        app_state::set_footer_message(&format_up_to_date_message(&current_version));
        return;
    }
    banner.apply_model(&banner_model_for_state(state));
}

pub(crate) fn format_up_to_date_message(current_version: &str) -> String {
    let ver = current_version.trim().trim_start_matches(['v', 'V']);
    if ui_text::locale_is_ru() {
        format!("AkhTerm v{ver} — установлена актуальная версия")
    } else {
        format!("AkhTerm v{ver} is up to date")
    }
}

fn banner_model_for_state(state: UpdateState) -> UpdateBannerModel {
    match state {
        UpdateState::Idle | UpdateState::Checking | UpdateState::UpToDate { .. } => {
            UpdateBannerModel::Hidden
        }
        UpdateState::Available { current_version, latest_version, release_url, .. } => {
            UpdateBannerModel::Available { current_version, latest_version, release_url }
        }
        UpdateState::Downloading { current_version, latest_version, release_url } => {
            UpdateBannerModel::Downloading { current_version, latest_version, release_url }
        }
        UpdateState::Installing { current_version, latest_version, release_url } => {
            UpdateBannerModel::Installing { current_version, latest_version, release_url }
        }
        UpdateState::ReadyToRestart { current_version, latest_version, release_url, .. } => {
            UpdateBannerModel::ReadyToRestart { current_version, latest_version, release_url }
        }
        UpdateState::Failed { current_version, latest_version, release_url, error } => {
            UpdateBannerModel::Failed { current_version, latest_version, release_url, error }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use crate::app::app_tests::install_test_window;
    use crate::i18n::{set_locale, with_locale_lock, UiLocale};
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::ui::palette::PaletteAction;

    struct WireEnvGuard {
        root: PathBuf,
        prev_url: Option<std::ffi::OsString>,
        prev_exe: Option<std::ffi::OsString>,
    }

    impl WireEnvGuard {
        fn new(root: PathBuf, manifest_url: &str, target_exe: &Path) -> Self {
            let state_dir = root.join("state");
            fs::create_dir_all(&state_dir).unwrap();
            updater::set_test_state_dir(Some(state_dir));
            let prev_url = std::env::var_os("AKHTERM_UPDATE_MANIFEST_URL");
            let prev_exe = std::env::var_os("AKHTERM_UPDATE_TARGET_EXE");
            unsafe {
                std::env::set_var("AKHTERM_UPDATE_MANIFEST_URL", manifest_url);
                std::env::set_var("AKHTERM_UPDATE_TARGET_EXE", target_exe);
            }
            Self { root, prev_url, prev_exe }
        }
    }

    impl Drop for WireEnvGuard {
        fn drop(&mut self) {
            restore_opt("AKHTERM_UPDATE_MANIFEST_URL", self.prev_url.take());
            restore_opt("AKHTERM_UPDATE_TARGET_EXE", self.prev_exe.take());
            updater::set_test_state_dir(None);
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn restore_opt(key: &str, val: Option<std::ffi::OsString>) {
        match val {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    fn write_script_exe(path: &Path, ver: &str) {
        fs::write(path, format!("#!/bin/sh\necho 'AkhTerm {ver}'\n")).unwrap();
        fs::set_permissions(path, PermissionsExt::from_mode(0o755)).unwrap();
    }

    fn build_tar_fixture(root: &Path, tag: &str, version_str: &str) -> (PathBuf, String) {
        let stage = root.join("stage");
        let dir = stage.join(format!("akhterm-{version_str}-linux-{}", std::env::consts::ARCH));
        fs::create_dir_all(&dir).unwrap();
        write_script_exe(&dir.join("akhterm"), version_str);
        let name = format!("akhterm_{version_str}_linux_{}.tar.gz", std::env::consts::ARCH);
        let tar_path = root.join(&name);
        let st = Command::new("tar")
            .args(["-C", stage.to_str().unwrap(), "-czf", tar_path.to_str().unwrap()])
            .arg(dir.file_name().unwrap())
            .status()
            .unwrap();
        assert!(st.success());
        let sha = updater::compute_file_sha256(&tar_path).unwrap();
        let sums_path = root.join("SHA256SUMS");
        fs::write(&sums_path, format!("{sha}  {name}\n")).unwrap();
        let manifest_path = root.join("latest.json");
        let json = format!(
            "{{\"tag_name\":\"{tag}\",\"html_url\":\"https://example.test/{tag}\",\"assets\":[{{\"name\":\"SHA256SUMS\",\"browser_download_url\":\"file://{}\"}},{{\"name\":\"{name}\",\"browser_download_url\":\"file://{}\"}}]}}",
            sums_path.display(),
            tar_path.display()
        );
        fs::write(&manifest_path, json).unwrap();
        let exe = root.join("bin/akhterm");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        write_script_exe(&exe, "1.0.0");
        (exe, format!("file://{}", manifest_path.display()))
    }

    fn pump_until_banner(predicate: impl Fn(&UpdateBannerModel) -> bool) -> UpdateBannerModel {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            pump_gtk_events();
            let model = app_state::with_app(|app| app.update_banner.model_for_test()).unwrap();
            if predicate(&model) {
                return model;
            }
            assert!(Instant::now() < deadline, "timed out waiting for banner model: {model:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn pump_until_footer_contains(needle: &str, footer: *mut GtkWidget) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            pump_gtk_events();
            let text = unsafe { CStr::from_ptr(gtk_label_get_text(footer)).to_string_lossy() };
            if text.contains(needle) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for footer '{needle}', got '{text}'");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn verify_e2e_check_upgrade_and_uptodate() {
        init_hooks();
        let root = std::env::temp_dir().join(format!("akhterm-wire-e2e-{}", std::process::id()));
        let (target_exe, url) = build_tar_fixture(&root, "v9.9.0", "9.9.0");
        let _env = WireEnvGuard::new(root.clone(), &url, &target_exe);
        let (win, footer) = install_test_window();
        set_locale(UiLocale::En);

        crate::shortcuts::execute_palette_action(PaletteAction::CheckUpdates);
        let avail = pump_until_banner(|m| matches!(m, UpdateBannerModel::Available { .. }));
        assert!(matches!(avail, UpdateBannerModel::Available { ref latest_version, .. } if latest_version == "v9.9.0"));

        let btn = app_state::with_app(|app| app.update_banner.primary_button_for_test()).unwrap();
        unsafe { gtk_button_clicked(btn) };
        let ready = pump_until_banner(|m| matches!(m, UpdateBannerModel::ReadyToRestart { .. }));
        assert!(matches!(ready, UpdateBannerModel::ReadyToRestart { ref latest_version, .. } if latest_version == "v9.9.0"));

        let ver_out = Command::new(&target_exe).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&ver_out.stdout).trim(), "AkhTerm 9.9.0");

        let (old_exe, old_url) = build_tar_fixture(&root.join("old"), "v0.1.0", "0.1.0");
        let _old_env = WireEnvGuard::new(root.join("old-state"), &old_url, &old_exe);
        trigger_manual_check();
        pump_until_footer_contains("is up to date", footer);
        assert_eq!(
            app_state::with_app(|app| app.update_banner.model_for_test()).unwrap(),
            UpdateBannerModel::Hidden
        );

        unsafe { gtk_widget_destroy(win) };
        app_state::APP.with(|c| *c.borrow_mut() = None);
    }

    #[test]
    fn headless_broadway_updater_check_and_install_e2e() {
        with_locale_lock(|| {
            with_headless_gtk(|| {
                updater::with_test_env_lock(verify_e2e_check_upgrade_and_uptodate);
            });
        });
    }
}
