//! Unit and /bin/sh integration tests for `files` MIME codec, local_fs, ssh_ops, and MI gates.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::local_fs::{compute_local_size, copy_local_items, list_local_dir, resolve_unique_local_path};
use super::mime::{
    decode_akhterm_drag, decode_gnome_copied_files, decode_uri_list_bytes, encode_local_drag,
    encode_remote_drag, stage_png_bytes,
};
use super::ssh_ops::{
    build_remote_dir_upload_script, build_remote_file_upload_script, build_remote_list_script,
    parse_remote_listing, pump_stream,
};
use super::{format_bytes, DragPayload, FileKind, RemoteFileItem, TransferProgress};

fn make_temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("akhterm_files_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn verify_akhterm_codec_round_trips() {
    let local = vec![PathBuf::from("/tmp/Отчёт 1.pdf"), PathBuf::from("/tmp/it's \"quoted\".txt")];
    assert_eq!(decode_akhterm_drag(&encode_local_drag(&local)), Some(DragPayload::LocalPaths(local)));
    let remote = vec![RemoteFileItem {
        host: "gpu-01".into(), ssh_pid: 4242, path: "/srv/модель 'v2'/weights.bin".into(),
        kind: FileKind::File, size: 2048,
    }];
    assert_eq!(decode_akhterm_drag(&encode_remote_drag(&remote)), Some(DragPayload::RemoteItems(remote)));
    assert_eq!(decode_akhterm_drag(b"bad-header\nL\t/tmp/a\n"), None);
    assert_eq!(encode_local_drag(&[PathBuf::from("/tmp/bad\nline")]), b"akhterm-files-v1\n");
}

#[test]
fn mime_round_trips_local_remote_gnome_uri_and_png_staging() {
    verify_akhterm_codec_round_trips();
    let gnome = b"copy\nfile:///tmp/%D0%BE%D1%82%D1%87%D1%91%D1%82.txt\nfile:///tmp/b.rs\n";
    assert_eq!(decode_gnome_copied_files(gnome), vec![PathBuf::from("/tmp/отчёт.txt"), PathBuf::from("/tmp/b.rs")]);
    assert!(decode_gnome_copied_files(b"unknown\nfile:///tmp/a").is_empty());
    assert_eq!(decode_uri_list_bytes(b"# comment\r\nfile:///tmp/x%20y.md\r\n"), vec![PathBuf::from("/tmp/x y.md")]);
    let dir = make_temp_dir("mime_png");
    let png = stage_png_bytes(b"\x89PNG\r\n\x1a\npayload", &dir, 99).unwrap();
    assert!(png.is_file() && png.file_name().unwrap() == "akhterm_drop_99.png");
    assert!(stage_png_bytes(b"NOTPNG", &dir, 100).is_err());
    let _ = fs::remove_dir_all(&dir);
}

fn verify_local_collision_names(dst: &Path) {
    fs::write(dst.join("a.txt"), b"existing").unwrap();
    let u1 = resolve_unique_local_path(dst, "a.txt");
    assert_eq!(u1.file_name().unwrap(), "a (1).txt");
    fs::write(&u1, b"existing1").unwrap();
    assert_eq!(resolve_unique_local_path(dst, "a.txt").file_name().unwrap(), "a (2).txt");
    fs::create_dir(dst.join("папка")).unwrap();
    assert_eq!(resolve_unique_local_path(dst, "папка").file_name().unwrap(), "папка (1)");
}

fn has_temp_artifacts(dir: &Path) -> bool {
    fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with(".akhterm-"))
}

#[test]
fn local_fs_resolves_collisions_copies_trees_and_honors_cancel() {
    let dir = make_temp_dir("local_copy");
    let (src, dst) = (dir.join("src"), dir.join("dst"));
    fs::create_dir_all(src.join("папка/sub")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.txt"), b"hello").unwrap();
    fs::write(src.join("папка/sub/data.bin"), vec![7u8; 4096]).unwrap();
    verify_local_collision_names(&dst);
    verify_local_copy_and_cancel(&src, &dst);
    let _ = fs::remove_dir_all(&dir);
}

fn verify_local_copy_and_cancel(src: &Path, dst: &Path) {
    let sources = vec![src.join("a.txt"), src.join("папка")];
    let total = compute_local_size(&sources);
    assert_eq!(total, 4101);
    let prog = TransferProgress::new(total);
    assert_eq!(copy_local_items(&sources, dst, &prog).unwrap().len(), 2);
    assert_eq!(prog.snapshot(), (4101, 4101));
    assert_eq!(fs::read(dst.join("папка (1)/sub/data.bin")).unwrap().len(), 4096);
    assert!(copy_local_items(&[src.join("папка")], &src.join("папка/sub"), &prog).is_err());
    let cancel_prog = TransferProgress::new(10);
    cancel_prog.cancel();
    assert!(copy_local_items(&[src.join("a.txt")], dst, &cancel_prog).is_err());
    assert!(!has_temp_artifacts(dst));
}

#[test]
fn local_fs_lists_directories_first_and_formats_byte_sizes() {
    let dir = make_temp_dir("local_list");
    fs::create_dir(dir.join("Zeta")).unwrap();
    fs::create_dir(dir.join(".hidden_dir")).unwrap();
    fs::write(dir.join("alpha.txt"), b"12345").unwrap();
    fs::write(dir.join(".env"), b"SECRET=1").unwrap();
    let (_, visible) = list_local_dir(&dir, false).unwrap();
    assert_eq!(visible.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["Zeta", "alpha.txt"]);
    assert!(visible[0].kind.is_dir() && !visible[1].kind.is_dir());
    assert_eq!(list_local_dir(&dir, true).unwrap().1.len(), 4);
    assert_eq!((format_bytes(512), format_bytes(2048), format_bytes(15 * 1024 * 1024)),
        ("512 B".into(), "2.0 KB".into(), "15 MB".into()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ssh_ops_parses_listings_and_executes_posix_scripts_via_sh() {
    let raw = "/srv/проект\nf\t120\t100\tnotes.md\nd\t0\t200\tsrc\nf\t10\t300\t.git\n";
    let (cwd, entries) = parse_remote_listing(raw, false).unwrap();
    assert_eq!(cwd, "/srv/проект");
    assert_eq!((entries.len(), entries[0].name.as_str(), entries[0].full_path.as_str()),
        (2, "src", "/srv/проект/src"));
    assert_eq!(parse_remote_listing("/\nf\t1\t0\ta.txt\n", false).unwrap().1[0].full_path, "/a.txt");
    verify_posix_sh_scripts();
}

fn seed_sh_dirs(dir: &Path) -> (PathBuf, PathBuf) {
    let (work, src_dir) = (dir.join("remote $(x) 'y'"), dir.join("src_tree"));
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(src_dir.join("папка '1'")).unwrap();
    fs::write(work.join("-evil;id.txt"), b"old").unwrap();
    fs::write(src_dir.join("папка '1'/data.txt"), b"tar-body").unwrap();
    (work, src_dir)
}

fn verify_sh_listing(work: &Path) {
    let out = Command::new("sh").arg("-c").arg(build_remote_list_script(&work.to_string_lossy())).output().unwrap();
    assert!(out.status.success(), "list failed: {}", String::from_utf8_lossy(&out.stderr));
    let (cwd, listed) = parse_remote_listing(&String::from_utf8_lossy(&out.stdout), false).unwrap();
    assert_eq!((cwd, listed[0].name.as_str()), (work.to_string_lossy().into_owned(), "-evil;id.txt"));
}

fn verify_posix_sh_scripts() {
    let dir = make_temp_dir("ssh_sh");
    let (work, src_dir) = seed_sh_dirs(&dir);
    verify_sh_listing(&work);
    verify_sh_file_upload(&work);
    verify_sh_dir_upload(&work, &src_dir);
    verify_pump_stream_and_cancel();
    let _ = fs::remove_dir_all(&dir);
}

fn run_sh_stdin_script(script: &str, stdin_bytes: &[u8]) -> PathBuf {
    let mut child = Command::new("sh").arg("-c").arg(script).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin_bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "sh failed: {}", String::from_utf8_lossy(&out.stderr));
    PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
}

fn verify_sh_file_upload(work: &Path) {
    let script = build_remote_file_upload_script(&work.to_string_lossy(), "-evil;id.txt", None);
    let uploaded = run_sh_stdin_script(&script, b"new-bytes");
    assert_eq!(uploaded.file_name().unwrap(), "-evil;id (1).txt");
    assert_eq!(fs::read(&uploaded).unwrap(), b"new-bytes");
}

fn verify_sh_dir_upload(work: &Path, src_dir: &Path) {
    let dir_script = build_remote_dir_upload_script(&work.to_string_lossy(), "папка '1'", None);
    let tar_bytes = Command::new("tar").args(["-cf", "-", "-C"]).arg(src_dir).args(["--", "папка '1'"]).output().unwrap();
    assert!(tar_bytes.status.success());
    let uploaded_dir = run_sh_stdin_script(&dir_script, &tar_bytes.stdout);
    assert_eq!(fs::read(uploaded_dir.join("data.txt")).unwrap(), b"tar-body");
}

fn verify_pump_stream_and_cancel() {
    let prog = TransferProgress::new(100);
    let (mut src, mut dst) = (Cursor::new(vec![42u8; 100]), Vec::new());
    assert_eq!(pump_stream(&mut src, &mut dst, &prog).unwrap(), 100);
    assert_eq!(prog.snapshot(), (100, 100));
    let cancel = TransferProgress::new(100);
    cancel.cancel();
    assert!(pump_stream(&mut Cursor::new(vec![1u8; 100]), &mut dst, &cancel).is_err());
}

fn check_mi_pair(name: &str, body: &str, failures: &mut Vec<String>) {
    let (sloc, cc, mi) = crate::mi::compute_file_mi(body);
    println!("{name:<24} | SLOC {sloc:>4} | CC {cc:>2} | MI {mi:>6.2}");
    if mi < 62.0 {
        failures.push(format!("{name} MI {mi:.2} < 62.0"));
    }
}

#[test]
fn files_modules_meet_high_maintainability_index() {
    let mut bad = Vec::new();
    check_mi_pair("src/files/mod.rs", include_str!("mod.rs"), &mut bad);
    check_mi_pair("src/files/mime.rs", include_str!("mime.rs"), &mut bad);
    check_mi_pair("src/files/local_fs.rs", include_str!("local_fs.rs"), &mut bad);
    check_mi_pair("src/files/ssh_ops.rs", include_str!("ssh_ops.rs"), &mut bad);
    check_mi_pair("src/files/tests.rs", include_str!("tests.rs"), &mut bad);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
