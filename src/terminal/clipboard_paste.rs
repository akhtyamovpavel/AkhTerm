//! GTK clipboard image and file-URI paste handling (local save and SSH upload).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::ptr;

use crate::ffi::*;
use crate::proc_radar::shell_quote;

#[derive(Clone, Copy)]
struct ClipboardTargets {
    has_image: bool,
    has_text: bool,
    has_uri: bool,
}

fn spawn_ssh_upload(ssh_host: &str, remote_path: &str) -> Option<Child> {
    let remote_cmd = format!("cat > {}", shell_quote(remote_path));
    Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=4",
            "-o", "ClearAllForwardings=yes",
            "--", ssh_host, &remote_cmd,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn upload_bytes_to_ssh(ssh_host: &str, remote_path: &str, bytes: &[u8]) -> bool {
    let Some(mut child) = spawn_ssh_upload(ssh_host, remote_path) else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(bytes).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

pub(crate) fn take_glib_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    unsafe { g_free(ptr as *mut c_void) };
    Some(text)
}

unsafe fn read_target_flags(targets: *mut GdkAtom, count: c_int) -> ClipboardTargets {
    ClipboardTargets {
        has_image: gtk_targets_include_image(targets, count, 0) != 0,
        has_text: gtk_targets_include_text(targets, count) != 0,
        has_uri: gtk_targets_include_uri(targets, count) != 0,
    }
}

fn clipboard_targets(clipboard: *mut GtkClipboard) -> Option<ClipboardTargets> {
    let mut targets: *mut GdkAtom = ptr::null_mut();
    let mut count: c_int = 0;
    let ok = unsafe { gtk_clipboard_wait_for_targets(clipboard, &mut targets, &mut count) };
    if ok == 0 || targets.is_null() || count <= 0 {
        unsafe { g_free(targets as *mut c_void) };
        return None;
    }
    let info = unsafe { read_target_flags(targets, count) };
    unsafe { g_free(targets as *mut c_void) };
    Some(info)
}

pub fn clipboard_has_direct_image() -> bool {
    let clipboard = unsafe { gtk_clipboard_get(GDK_SELECTION_CLIPBOARD) };
    if clipboard.is_null() {
        return false;
    }
    clipboard_targets(clipboard).is_some_and(|t| t.has_image && !t.has_text)
}

#[derive(Clone, Copy)]
struct PasteCtx<'a> {
    clipboard: *mut GtkClipboard,
    targets: ClipboardTargets,
    uri_ok: bool,
    ssh_host: &'a str,
    force_image: bool,
    ts: u64,
}

fn pixbuf_save_png_raw(pixbuf: *mut GdkPixbuf) -> Option<(*mut c_char, usize)> {
    let (mut buf, mut size) = (ptr::null_mut(), 0usize);
    let saved = unsafe {
        gdk_pixbuf_save_to_bufferv(
            pixbuf, &mut buf, &mut size, c"png".as_ptr(),
            ptr::null_mut(), ptr::null_mut(), ptr::null_mut(),
        )
    };
    if saved == 0 || buf.is_null() || size == 0 {
        unsafe { g_free(buf as *mut c_void) };
        return None;
    }
    Some((buf, size))
}

fn pixbuf_to_png(pixbuf: *mut GdkPixbuf) -> Option<Vec<u8>> {
    let (buf, size) = pixbuf_save_png_raw(pixbuf)?;
    let bytes = unsafe { std::slice::from_raw_parts(buf as *const u8, size).to_vec() };
    unsafe { g_free(buf as *mut c_void) };
    Some(bytes)
}

fn clipboard_png_bytes(clipboard: *mut GtkClipboard) -> Option<Vec<u8>> {
    let pixbuf = unsafe { gtk_clipboard_wait_for_image(clipboard) };
    if pixbuf.is_null() {
        return None;
    }
    let bytes = pixbuf_to_png(pixbuf);
    unsafe { g_object_unref(pixbuf) };
    bytes
}

fn read_uri_list(uris: *mut *mut c_char) -> Vec<String> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    unsafe {
        while !(*uris.add(idx)).is_null() {
            let item = CStr::from_ptr(*uris.add(idx)).to_string_lossy().into_owned();
            if !item.trim().is_empty() {
                out.push(item);
            }
            idx += 1;
        }
    }
    out
}

fn clipboard_uris(clipboard: *mut GtkClipboard) -> Vec<String> {
    let uris = unsafe { gtk_clipboard_wait_for_uris(clipboard) };
    if uris.is_null() {
        return Vec::new();
    }
    let out = read_uri_list(uris);
    unsafe { g_strfreev(uris) };
    out
}

fn is_file_uri_text_only(raw: &str) -> bool {
    let mut has_line = false;
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if !line.starts_with("file://") {
            return false;
        }
        has_line = true;
    }
    has_line
}

fn clipboard_allows_uri_paste(clipboard: *mut GtkClipboard, targets: ClipboardTargets) -> bool {
    if !targets.has_uri {
        return false;
    }
    if !targets.has_text {
        return true;
    }
    let text = take_glib_string(unsafe { gtk_clipboard_wait_for_text(clipboard) });
    text.as_deref().map_or(true, is_file_uri_text_only)
}

fn format_local_uri_paste(paths: &[PathBuf]) -> Option<(String, String)> {
    let formatted = crate::file_drop::format_local_drop_paths(paths);
    if formatted.is_empty() {
        return None;
    }
    let status = if paths.len() == 1 {
        format!("📎 Вставлен путь: {}", paths[0].display())
    } else {
        format!("📎 Вставлено путей: {}", paths.len())
    };
    Some((formatted, status))
}

fn remote_paste_path(local_path: &Path, ts: u64, idx: usize) -> String {
    let raw = local_path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let safe = crate::file_drop::sanitize_remote_basename(raw);
    if idx == 0 {
        format!("/tmp/akhterm_paste_{ts}_{safe}")
    } else {
        format!("/tmp/akhterm_paste_{ts}_{idx}_{safe}")
    }
}

fn format_remote_uri_status(ssh_host: &str, remote_paths: &[String], total_bytes: usize) -> String {
    let kb = (total_bytes / 1024).max(1);
    if remote_paths.len() == 1 {
        format!("📎 Файл ({kb} КБ) загружен по SSH на {ssh_host}:{}", remote_paths[0])
    } else {
        format!("📎 Файлы ({}, {kb} КБ) загружены по SSH на {ssh_host}", remote_paths.len())
    }
}

fn upload_one_uri_file<F: FnMut(&str, &str, &[u8]) -> bool>(
    ssh_host: &str, path: &Path, ts: u64, idx: usize, uploader: &mut F,
) -> Option<(String, usize)> {
    let bytes = fs::read(path).ok()?;
    let remote = remote_paste_path(path, ts, idx);
    uploader(ssh_host, &remote, &bytes).then_some((remote, bytes.len()))
}

fn upload_clipboard_uri_files<F: FnMut(&str, &str, &[u8]) -> bool>(
    ssh_host: &str, paths: &[PathBuf], ts: u64, mut uploader: F,
) -> Option<(String, String)> {
    let (mut remote_paths, mut total_bytes) = (Vec::new(), 0usize);
    for path in paths.iter().filter(|p| p.is_file()) {
        let (remote, len) = upload_one_uri_file(ssh_host, path, ts, remote_paths.len(), &mut uploader)?;
        total_bytes += len;
        remote_paths.push(remote);
    }
    if remote_paths.is_empty() {
        return None;
    }
    let status = format_remote_uri_status(ssh_host, &remote_paths, total_bytes);
    Some((crate::file_drop::format_shell_paths(&remote_paths), status))
}

fn try_paste_clipboard_uris<F: FnMut(&str, &str, &[u8]) -> bool>(
    clipboard: *mut GtkClipboard, targets: ClipboardTargets, ssh_host: &str, ts: u64, uploader: F,
) -> Option<(String, String)> {
    if !clipboard_allows_uri_paste(clipboard, targets) {
        return None;
    }
    let paths = crate::file_drop::decode_uri_list(&clipboard_uris(clipboard));
    if paths.is_empty() {
        return None;
    }
    if ssh_host.is_empty() {
        format_local_uri_paste(&paths)
    } else {
        upload_clipboard_uri_files(ssh_host, &paths, ts, uploader)
    }
}

fn first_clipboard_line(raw: &str) -> Option<&str> {
    raw.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
}

fn clipboard_text_payload(clipboard: *mut GtkClipboard, has_text: bool) -> Option<(String, bool)> {
    let text = has_text
        .then(|| take_glib_string(unsafe { gtk_clipboard_wait_for_text(clipboard) }))
        .flatten()
        .filter(|s| !s.trim().is_empty())?;
    let is_uri = is_file_uri_text_only(&text);
    Some((text, is_uri))
}

fn clipboard_uri_or_text(ctx: PasteCtx<'_>) -> Option<(String, bool)> {
    if ctx.uri_ok {
        let uris = clipboard_uris(ctx.clipboard);
        if uris.len() == 1 || (ctx.force_image && !uris.is_empty()) {
            return uris.into_iter().next().map(|u| (u, true));
        }
        if uris.len() > 1 {
            return None;
        }
    }
    clipboard_text_payload(ctx.clipboard, ctx.targets.has_text)
}

pub(crate) fn uri_to_local_path(uri: &str) -> Option<PathBuf> {
    let c_uri = CString::new(uri).ok()?;
    let ptr = unsafe { g_filename_from_uri(c_uri.as_ptr(), ptr::null_mut(), ptr::null_mut()) };
    take_glib_string(ptr).map(PathBuf::from)
}

fn clipboard_line_to_path(line: &str) -> PathBuf {
    if !line.starts_with("file://") {
        return PathBuf::from(line);
    }
    uri_to_local_path(line).unwrap_or_else(|| PathBuf::from(line.trim_start_matches("file://")))
}

fn parse_clipboard_image_file(raw: &str) -> Option<(PathBuf, String)> {
    let path = clipboard_line_to_path(first_clipboard_line(raw)?);
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    (crate::platform::is_clipboard_image_ext(&ext) && path.is_file()).then_some((path, ext))
}

fn upload_image_to_ssh(ssh_host: &str, target_path: &str, bytes: &[u8], kind: &str, kb: usize) -> Option<(String, String)> {
    if kind == "Скриншот" {
        let _ = fs::write(target_path, bytes);
    }
    let msg = format!("🖼 {kind} ({kb} КБ) загружен по SSH на {ssh_host}:{target_path}");
    upload_bytes_to_ssh(ssh_host, target_path, bytes).then(|| (format!("{target_path} "), msg))
}

fn save_local_image(target_path: &str, bytes: &[u8], kind: &str, kb: usize) -> Option<(String, String)> {
    fs::write(target_path, bytes).ok()?;
    Some((format!("{target_path} "), format!("🖼 {kind} ({kb} КБ) сохранён в {target_path}")))
}

fn save_or_upload_image(ssh_host: &str, bytes: &[u8], ext: &str, kind: &str, ts: u64) -> Option<(String, String)> {
    let target_path = format!("/tmp/akhterm_paste_{ts}.{ext}");
    let kb = (bytes.len() / 1024).max(1);
    if !ssh_host.is_empty() {
        return upload_image_to_ssh(ssh_host, &target_path, bytes, kind, kb);
    }
    save_local_image(&target_path, bytes, kind, kb)
}

fn save_clipboard_file_payload(ctx: PasteCtx<'_>) -> Option<(String, String)> {
    let (raw, from_uri) = clipboard_uri_or_text(ctx)?;
    if ctx.ssh_host.is_empty() && !ctx.force_image && !from_uri {
        return None;
    }
    let (path, ext) = parse_clipboard_image_file(&raw)?;
    let bytes = fs::read(path).ok()?;
    save_or_upload_image(ctx.ssh_host, &bytes, &ext, "Файл", ctx.ts)
}

fn current_paste_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn try_paste_direct_png(ctx: PasteCtx<'_>) -> Option<(String, String)> {
    let allow = ctx.targets.has_image && (ctx.force_image || !ctx.targets.has_text || ctx.uri_ok);
    let bytes = allow.then(|| clipboard_png_bytes(ctx.clipboard)).flatten()?;
    save_or_upload_image(ctx.ssh_host, &bytes, "png", "Скриншот", ctx.ts)
}

fn try_paste_file_payload(ctx: PasteCtx<'_>) -> Option<(String, String)> {
    if !ctx.targets.has_uri && !ctx.targets.has_text {
        return None;
    }
    save_clipboard_file_payload(ctx)
}

fn try_paste_for_ctx(ctx: PasteCtx<'_>) -> Option<(String, String)> {
    try_paste_direct_png(ctx)
        .or_else(|| try_paste_file_payload(ctx))
        .or_else(|| {
            (!ctx.force_image && ctx.uri_ok)
                .then(|| try_paste_clipboard_uris(ctx.clipboard, ctx.targets, ctx.ssh_host, ctx.ts, upload_bytes_to_ssh))
                .flatten()
        })
}

fn build_paste_ctx(clipboard: *mut GtkClipboard, ssh_host: &str, force_image: bool) -> Option<PasteCtx<'_>> {
    let targets = clipboard_targets(clipboard)?;
    let uri_ok = clipboard_allows_uri_paste(clipboard, targets);
    Some(PasteCtx { clipboard, targets, uri_ok, ssh_host, force_image, ts: current_paste_ts() })
}

pub fn try_paste_clipboard_image(ssh_host: &str, force_image: bool) -> Option<(String, String)> {
    let clipboard = unsafe { gtk_clipboard_get(GDK_SELECTION_CLIPBOARD) };
    if clipboard.is_null() {
        return None;
    }
    try_paste_for_ctx(build_paste_ctx(clipboard, ssh_host, force_image)?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::proc_radar::tests::pump_gtk_events;

    struct MockClipPayload {
        uris: Vec<CString>,
        text: Option<CString>,
    }

    fn clean_flat_dir(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let _ = fs::remove_file(e.path());
            }
        }
        let _ = fs::remove_dir(dir);
    }

    fn seed_clipboard_png() {
        unsafe {
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            let pix = gdk_pixbuf_new(0, 1, 8, 4, 4);
            gtk_clipboard_set_image(clip, pix);
            g_object_unref(pix);
            pump_gtk_events();
        }
    }

    pub(crate) fn verify_image_clipboard_paste() {
        seed_clipboard_png();
        assert!(clipboard_has_direct_image());
        let (pasted_img, status) = try_paste_clipboard_image("", false).expect("png saved");
        let saved_path = pasted_img.trim();
        let png_bytes = fs::read(saved_path).unwrap();
        assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(status.contains("Скриншот"));
        let _ = fs::remove_file(saved_path);
    }

    extern "C" fn provide_mock_clip(
        _clip: *mut GtkClipboard,
        sel: *mut GtkSelectionData,
        info: u32,
        data: *mut c_void,
    ) {
        let payload = unsafe { &*data.cast::<MockClipPayload>() };
        if info == 1 {
            let mut ptrs: Vec<*const c_char> = payload.uris.iter().map(|s| s.as_ptr()).collect();
            ptrs.push(ptr::null());
            unsafe { gtk_selection_data_set_uris(sel, ptrs.as_ptr()) };
        } else if let Some(txt) = &payload.text {
            unsafe { gtk_selection_data_set_text(sel, txt.as_ptr(), txt.as_bytes().len() as c_int) };
        }
    }

    extern "C" fn drop_mock_clip(_clip: *mut GtkClipboard, data: *mut c_void) {
        if !data.is_null() {
            unsafe { drop(Box::from_raw(data.cast::<MockClipPayload>())) };
        }
    }

    fn build_mock_clip_entries(has_text: bool) -> Vec<GtkTargetEntry> {
        let mut entries = vec![GtkTargetEntry { target: c"text/uri-list".as_ptr(), flags: 0, info: 1 }];
        if has_text {
            entries.push(GtkTargetEntry { target: c"UTF8_STRING".as_ptr(), flags: 0, info: 2 });
        }
        entries
    }

    fn box_mock_clip_payload(uris: &[String], text: Option<&str>) -> *mut c_void {
        let payload = MockClipPayload {
            uris: uris.iter().map(|u| CString::new(u.as_str()).unwrap()).collect(),
            text: text.map(|t| CString::new(t).unwrap()),
        };
        Box::into_raw(Box::new(payload)).cast()
    }

    fn set_mock_uri_clipboard(uris: &[String], text: Option<&str>) {
        let entries = build_mock_clip_entries(text.is_some());
        let payload = box_mock_clip_payload(uris, text);
        unsafe {
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            let ok = gtk_clipboard_set_with_data(
                clip, entries.as_ptr(), entries.len() as u32,
                provide_mock_clip, drop_mock_clip, payload,
            );
            assert_ne!(ok, 0);
            pump_gtk_events();
        }
    }

    fn assert_uploaded_uri_files(uploaded: &[(String, String, Vec<u8>)], out: &str, msg: &str) {
        assert_eq!(uploaded.len(), 2);
        assert_eq!(uploaded[0].0, "gpu-box");
        assert_eq!(uploaded[0].1, "/tmp/akhterm_paste_77_отчёт_1.pdf");
        assert_eq!(uploaded[0].2, b"%PDF-1.7 sample");
        assert_eq!(uploaded[1].1, "/tmp/akhterm_paste_77_1_lib.rs");
        assert!(out.contains("'/tmp/akhterm_paste_77_отчёт_1.pdf' /tmp/akhterm_paste_77_1_lib.rs "));
        assert!(msg.contains("gpu-box"));
    }

    fn verify_mock_remote_uri_upload() {
        let clip = unsafe { gtk_clipboard_get(GDK_SELECTION_CLIPBOARD) };
        let t = clipboard_targets(clip).unwrap();
        let mut uploaded: Vec<(String, String, Vec<u8>)> = Vec::new();
        let (rem_out, rem_msg) = try_paste_clipboard_uris(clip, t, "gpu-box", 77, |h, r, b| {
            uploaded.push((h.to_owned(), r.to_owned(), b.to_vec()));
            true
        })
        .expect("mock ssh uri upload");
        assert_uploaded_uri_files(&uploaded, &rem_out, &rem_msg);
        assert!(try_paste_clipboard_image("127.0.0.1", false).is_none());
    }

    fn assert_plain_text_not_pasted() {
        assert!(try_paste_clipboard_image("", false).is_none());
        assert!(try_paste_clipboard_image("127.0.0.1", false).is_none());
    }

    fn verify_bmp_uri_and_plain_text_guards(uri_bmp: &str) {
        set_mock_uri_clipboard(&[uri_bmp.to_owned()], Some(&format!("# KDE\r\n{uri_bmp}\r\n")));
        let (bmp_saved, _) = try_paste_clipboard_image("", false).expect("bmp uri image saved");
        assert!(bmp_saved.trim().ends_with(".bmp"));
        let _ = fs::remove_file(bmp_saved.trim());
        set_mock_uri_clipboard(&[String::from("file:///etc/hosts")], Some("/etc/hosts"));
        assert_plain_text_not_pasted();
        crate::context_menu::copy_text_to_clipboard("/etc/hosts");
        pump_gtk_events();
        assert_plain_text_not_pasted();
    }

    fn seed_uri_paste_fixture(dir: &Path) -> (String, Vec<String>, String) {
        clean_flat_dir(dir);
        fs::create_dir_all(dir).unwrap();
        let (pdf, rs_file, bmp) = (dir.join("отчёт 1.pdf"), dir.join("lib.rs"), dir.join("shot.bmp"));
        fs::write(&pdf, b"%PDF-1.7 sample").unwrap();
        fs::write(&rs_file, b"pub fn run() {}\n").unwrap();
        fs::write(&bmp, b"BM\x1e\x00\x00\x00").unwrap();
        let uri_pdf = format!("file://{}/%D0%BE%D1%82%D1%87%D1%91%D1%82%201.pdf", dir.display());
        let uri_rs = format!("file://{}/lib.rs", dir.display());
        let uri_bmp = format!("file://{}/shot.bmp", dir.display());
        (format!("'{}' {} ", pdf.display(), rs_file.display()), vec![uri_pdf, uri_rs], uri_bmp)
    }

    fn assert_local_uri_paste(uris: &[String], expected_local: &str) {
        set_mock_uri_clipboard(uris, None);
        assert!(!clipboard_has_direct_image());
        assert!(try_paste_clipboard_image("", true).is_none());
        let (local_out, local_msg) = try_paste_clipboard_image("", false).expect("local uri paste");
        assert_eq!(local_out, expected_local);
        assert!(local_msg.contains("Вставлено путей: 2"));
    }

    pub(crate) fn verify_non_image_uri_paste_and_plain_text_guard() {
        let dir = std::env::temp_dir().join(format!("akhterm_uri_paste_{}", std::process::id()));
        let (expected_local, uris, uri_bmp) = seed_uri_paste_fixture(&dir);
        assert_local_uri_paste(&uris, &expected_local);
        verify_mock_remote_uri_upload();
        let dolphin_txt = format!("{}\r\n{}\r\n", uris[0], uris[1]);
        set_mock_uri_clipboard(&uris, Some(&dolphin_txt));
        assert_eq!(try_paste_clipboard_image("", false).unwrap().0, expected_local);
        verify_bmp_uri_and_plain_text_guards(&uri_bmp);
        clean_flat_dir(&dir);
    }

    #[test]
    fn parses_local_image_paths_and_encoded_file_uris() {
        let dir = std::env::temp_dir().join(format!("akhterm_clip_uri_{}", std::process::id()));
        clean_flat_dir(&dir);
        fs::create_dir_all(&dir).unwrap();
        let img = dir.join("Снимок экрана 1.png");
        fs::write(&img, b"\x89PNG\r\n\x1a\n").unwrap();
        assert_eq!(parse_clipboard_image_file(&img.to_string_lossy()), Some((img.clone(), "png".into())));
        let encoded = format!("# KDE uri-list\r\nfile://{}/%D0%A1%D0%BD%D0%B8%D0%BC%D0%BE%D0%BA%20%D1%8D%D0%BA%D1%80%D0%B0%D0%BD%D0%B0%201.png\r\n", dir.display());
        assert_eq!(parse_clipboard_image_file(&encoded), Some((img, "png".into())));
        clean_flat_dir(&dir);
    }

    #[test]
    fn parses_bmp_webp_clipboard_files_and_ignores_plain_text() {
        let dir = std::env::temp_dir().join(format!("akhterm_clip_fmt_{}", std::process::id()));
        clean_flat_dir(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (bmp, webp, txt) = (dir.join("clip.BMP"), dir.join("frame.webp"), dir.join("notes.txt"));
        fs::write(&bmp, b"BM").unwrap();
        fs::write(&webp, b"RIFF").unwrap();
        fs::write(&txt, b"plain").unwrap();
        assert_eq!(parse_clipboard_image_file(&format!("file://{}", bmp.display())).unwrap().1, "bmp");
        assert_eq!(parse_clipboard_image_file(&webp.to_string_lossy()).unwrap().1, "webp");
        assert!(parse_clipboard_image_file(&txt.to_string_lossy()).is_none());
        assert!(parse_clipboard_image_file("cargo test --offline").is_none());
        clean_flat_dir(&dir);
    }
}
