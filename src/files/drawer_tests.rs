//! Unit, headless GTK widget, and Maintainability Index tests for the Files drawer.

use std::{ffi::CStr, fs, path::*, rc::Rc, time::*};

use crate::context_menu::{build_vte_menu, DispatchFn, VteMenuSpec};
use crate::ffi::*;
use crate::files::drawer::*;
use crate::proc_radar::tests::with_headless_gtk;
use crate::files::{FileEntry, FileKind, RemoteFileLink};
use crate::i18n::{set_locale, with_locale_lock, UiLocale};
use crate::telemetry::ssh_connection::SshRoute;

fn temp_drawer_dir() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("akhterm-drawer-{nanos}"));
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::create_dir_all(&dir);
    dir
}

#[test]
fn drawer_shortcut_matches_latin_and_cyrillic_ctrl_shift_o() {
    let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
    assert!(is_drawer_shortcut(u32::from(b'O'), cs, 0));
    assert!(is_drawer_shortcut(u32::from(b'o'), cs, 0));
    assert!(is_drawer_shortcut(0x06d9, cs, 32));
    assert!(!is_drawer_shortcut(u32::from(b'O'), GDK_CONTROL_MASK, 32));
    assert!(!is_drawer_shortcut(u32::from(b'P'), cs, 33));
}

#[test]
fn filter_entries_handles_ascii_and_cyrillic_case_insensitively() {
    let entries = vec![
        FileEntry { name: "README.md".into(), full_path: "/tmp/README.md".into(), kind: FileKind::File, size: 12, mtime: 0 },
        FileEntry { name: "Отчёт.docx".into(), full_path: "/tmp/Отчёт.docx".into(), kind: FileKind::File, size: 34, mtime: 0 },
    ];
    assert_eq!(filter_entries(&entries, "").len(), 2);
    assert_eq!(filter_entries(&entries, "read").len(), 1);
    let ru = filter_entries(&entries, "отчёт");
    assert_eq!((ru.len(), ru[0].name.as_str()), (1, "Отчёт.docx"));
}

fn read_menu_label(item: *mut GtkWidget) -> String {
    let raw = unsafe { gtk_menu_item_get_label(item) };
    if raw.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned()
}

fn verify_ssh_menu_in_locale(vte: *mut GtkWidget, dl: RemoteFileLink, locale: UiLocale, expected: &str) {
    set_locale(locale);
    let dispatch: DispatchFn = Rc::new(|_| {});
    let link = Some(crate::session::VteLink { url: "/srv/app/log.txt".into(), document: None });
    let spec = VteMenuSpec {
        sid: 1, vte, has_selection: false, selected_text: None, link_target: link,
        remote_download: Some(dl), is_local: false, targets: Vec::new(), has_cmd_marks: false, has_failed_cmd: false,
    };
    let built = build_vte_menu(&spec, &dispatch);
    assert_eq!(read_menu_label(built.items[4]), expected);
    unsafe { gtk_widget_destroy(built.menu) };
}

#[test]
fn vte_menu_adds_download_from_ssh_in_ru_and_en() {
    let route = SshRoute::for_host("prod-01").expect("route");
    let link = RemoteFileLink { sid: 1, host: "prod-01".into(), ssh_pid: 99, route, path: "/srv/app/log.txt".into() };
    with_locale_lock(|| with_headless_gtk(|| {
        let vte = unsafe { gtk_label_new(c"vte".as_ptr()) };
        verify_ssh_menu_in_locale(vte, link.clone(), UiLocale::Ru, "Скачать с SSH");
        verify_ssh_menu_in_locale(vte, link, UiLocale::En, "Download from SSH");
        unsafe { gtk_widget_destroy(vte) };
    }));
}

unsafe fn verify_mount_and_filter(dir: &Path) {
    let parent = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
    let main_child = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    mount(parent, main_child);
    assert!(!is_open());
    open();
    assert!(is_open());
    assert_eq!(test_drive_local_and_filter(dir, "отчёт"), (2, 1));
    close();
    assert!(!is_open());
    gtk_widget_destroy(parent);
}

#[test]
fn headless_drawer_mounts_lists_local_files_filters_and_toggles() {
    let dir = temp_drawer_dir();
    let _ = fs::write(dir.join("alpha.txt"), b"a");
    let _ = fs::write(dir.join("Отчёт.md"), b"# hi");
    with_headless_gtk(|| unsafe { verify_mount_and_filter(&dir) });
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn chunk3_drawer_modules_meet_maintainability_targets() {
    let targets = [
        ("drawer.rs", include_str!("drawer.rs"), 62.0),
        ("drawer_rows.rs", include_str!("drawer_rows.rs"), 62.0),
        ("drawer_transfer.rs", include_str!("drawer_transfer.rs"), 62.0),
        ("drawer_tests.rs", include_str!("drawer_tests.rs"), 60.0),
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
