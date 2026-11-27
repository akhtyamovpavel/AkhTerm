//! Unit, headless GTK widget, and Maintainability Index tests for Chunk 2 modules.

use std::{fs, path::*, ptr, sync::Mutex, time::*};

use crate::ffi::*;
use crate::files::drop_overlay::*;
use crate::files::manager::*;
use crate::files::mime::*;
use crate::files::resolve::*;
use crate::files::transfer_banner::*;
use crate::files::*;
use crate::i18n::*;
use crate::terminal::file_drop::setup_vte_file_drop;

pub(crate) static GTK_TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn ensure_gtk() -> bool {
    let mut argc = 0;
    let mut argv = ptr::null_mut();
    let ok = unsafe { gtk_init_check(&mut argc, &mut argv) };
    ok != 0
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("akhterm-ui-{tag}-{nanos}"));
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::create_dir_all(&dir);
    dir
}

unsafe fn make_overlay_fixture() -> (*mut GtkWidget, *mut GtkWidget) {
    let scroll = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    let child = gtk_label_new(c"vte".as_ptr());
    gtk_container_add(scroll as *mut GtkContainer, child);
    let overlay = wrap_pane_scroll(scroll);
    (child, overlay)
}

unsafe fn check_overlay_ru(child: *mut GtkWidget) {
    with_locale_lock(|| {
        set_locale(UiLocale::Ru);
        assert_eq!(show_for_vte(child, 0, false, false), DropMode::InsertPath);
    });
    assert_eq!(inspect_vte_overlay(child), Some((true, false, true)));
}

unsafe fn check_overlay_en(child: *mut GtkWidget) {
    with_locale_lock(|| {
        set_locale(UiLocale::En);
        assert_eq!(show_for_vte(child, 500, true, true), DropMode::CopyToCwd);
    });
    assert_eq!(inspect_vte_overlay(child), Some((false, true, true)));
}

#[test]
fn drop_overlay_zones_localize_toggle_and_retain_last_mode() {
    assert_eq!((mode_for_x(400, 100), mode_for_x(400, 300)), (DropMode::InsertPath, DropMode::CopyToCwd));
    let _guard = GTK_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !ensure_gtk() {
        return;
    }
    unsafe {
        let (child, overlay) = make_overlay_fixture();
        check_overlay_ru(child);
        check_overlay_en(child);
        hide_for_vte(child);
        assert_eq!(take_mode_for_vte(child), DropMode::CopyToCwd);
        gtk_widget_destroy(overlay);
        assert_eq!(inspect_vte_overlay(child), None);
    }
}

fn seed_hello(dir: &Path) -> (PathBuf, PathBuf) {
    let src = dir.join("src");
    let dst = dir.join("dst");
    let _ = fs::create_dir_all(&src);
    let _ = fs::create_dir_all(&dst);
    let file = src.join("hello.txt");
    fs::write(&file, b"hello akhterm").expect("write");
    (file, dst)
}

fn wait_settled(mgr: &mut TransferManager) -> TransferSnapshot {
    for _ in 0..80 {
        if let Some(s) = mgr.latest_snapshot().filter(|s| s.status != TransferStatus::Running) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    mgr.latest_snapshot().expect("settled snapshot")
}

fn verify_banner_visibility(snap: &TransferSnapshot) {
    let _guard = GTK_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !ensure_gtk() {
        return;
    }
    let b = TransferBanner::new();
    assert!(!banner_visible(&b));
    b.apply_snapshot(Some(snap));
    assert!(banner_visible(&b));
    b.apply_snapshot(None);
    assert!(!banner_visible(&b));
    unsafe { gtk_widget_destroy(banner_card_ptr(&b)) };
}

#[test]
fn transfer_manager_copies_files_and_drives_banner_visibility() {
    let dir = temp_dir("mgr");
    let (file, dst) = seed_hello(&dir);
    let mut mgr = TransferManager::default();
    mgr.start_local_copy(vec![file], dst.clone(), ptr::null_mut(), false);
    let snap = wait_settled(&mut mgr);
    assert_eq!(snap.status, TransferStatus::Done);
    assert_eq!(snap.direction, TransferDirection::LocalCopy);
    assert_eq!(fs::read(dst.join("hello.txt")).expect("read"), b"hello akhterm");
    verify_banner_visibility(&snap);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn transfer_manager_cancel_transitions_status() {
    let mut mgr = TransferManager::default();
    let missing = PathBuf::from("/nonexistent-akhterm-src");
    let id = mgr.start_local_copy(vec![missing], std::env::temp_dir(), ptr::null_mut(), false);
    if mgr.cancel(id) {
        let snap = mgr.latest_snapshot().expect("cancelled snap");
        assert_eq!(snap.status, TransferStatus::Cancelled);
    }
}

unsafe fn check_targets_registered(w: *mut GtkWidget) {
    let list = gtk_drag_dest_get_target_list(w);
    assert!(!list.is_null());
    for mime in [MIME_AKHTERM_FILES, MIME_URI_LIST, MIME_GNOME_COPIED, MIME_IMAGE_PNG] {
        let mut info = 0;
        let atom = gdk_atom_intern_static_string(mime.as_ptr());
        assert_ne!(gtk_target_list_find(list, atom, &mut info), 0);
    }
}

#[test]
fn setup_vte_file_drop_registers_all_four_mime_targets() {
    let _guard = GTK_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !ensure_gtk() {
        return;
    }
    unsafe {
        let w = gtk_label_new(c"drop-target".as_ptr());
        setup_vte_file_drop(w);
        check_targets_registered(w);
        gtk_widget_destroy(w);
    }
}

#[test]
fn endpoint_short_label_formats_local_and_remote() {
    let local = FileEndpoint::Local { cwd: PathBuf::from("/tmp/akhterm-work") };
    assert_eq!(endpoint_short_label(&local), "/tmp/akhterm-work");
    let route = crate::telemetry::ssh_connection::SshRoute::for_host("prod-box").expect("route");
    let remote = FileEndpoint::Remote { sid: 1, host: "prod-box".into(), ssh_pid: 42, route, cwd: "/srv/app".into() };
    assert_eq!(endpoint_short_label(&remote), "prod-box:/srv/app");
}

#[test]
fn clean_clicked_path_rejects_web_links() {
    assert_eq!(clean_clicked_path(""), None);
    assert_eq!(clean_clicked_path("https://example.com/a"), None);
    assert_eq!(clean_clicked_path("mailto:user@example.com"), None);
    assert_eq!(clean_clicked_path("file:///tmp/demo.txt"), Some("/tmp/demo.txt".into()));
    let stripped = clean_clicked_path("src/main.rs:42");
    assert_eq!(stripped.as_deref(), Some("src/main.rs"));
}

#[test]
fn chunk2_files_modules_meet_maintainability_targets() {
    let targets = [
        ("resolve.rs", include_str!("resolve.rs"), 62.0),
        ("manager.rs", include_str!("manager.rs"), 62.0),
        ("drop_overlay.rs", include_str!("drop_overlay.rs"), 62.0),
        ("transfer_banner.rs", include_str!("transfer_banner.rs"), 62.0),
        ("ui_tests.rs", include_str!("ui_tests.rs"), 60.0),
        ("file_drop.rs", include_str!("../terminal/file_drop.rs"), 58.0),
    ];
    let mut bad = Vec::new();
    for (name, body, min_mi) in targets {
        let (sloc, cc, score) = crate::telemetry::mi::compute_file_mi(body);
        eprintln!("{name:20} | SLOC {sloc:4} | CC {cc:2} | MI {score:.2}");
        if score < min_mi {
            bad.push(format!("{name}: {score:.2} < {min_mi:.1}"));
        }
    }
    assert!(bad.is_empty(), "Low MI modules: {bad:?}");
}
