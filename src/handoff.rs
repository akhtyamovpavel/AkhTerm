//! On-demand Markdown handoff discovery, terminal output capture, and tab forwarding.

use std::ffi::{c_char, c_void, CStr, CString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::connect_click;
use crate::ffi::*;
use crate::paths::{shorten_home, state_dir, write_state_file};
use crate::tabs::is_ssh_host;
use crate::ui_text as text;

pub type ClickHandler = extern "C" fn(*mut GtkWidget, *mut c_void);

pub const CAPTURE_MAX_LINES: usize = 80;
const MAX_DISCOVERED_FILES: usize = 10;
const VTE_CAPTURE_ROWS: isize = 240;

pub struct CaptureMeta<'a> {
    pub title: &'a str,
    pub cwd: &'a str,
    pub ssh_host: &'a str,
    pub timestamp_secs: u64,
}

pub struct HandoffTarget {
    pub sid: usize,
    pub title: String,
    pub ssh_host: String,
    pub is_active: bool,
}

#[derive(Clone, Copy)]
pub struct HandoffCallbacks {
    pub on_capture: ClickHandler,
    pub on_forward: ClickHandler,
    pub on_inject: ClickHandler,
}

#[derive(Clone, Copy)]
pub struct HandoffWidgets {
    pub popover: *mut GtkWidget,
    pub outer_box: *mut GtkWidget,
    pub old_inner: *mut GtkWidget,
}

pub struct HandoffMenu {
    popover: *mut GtkWidget,
    outer_box: *mut GtkWidget,
    inner_box: *mut GtkWidget,
    files: Vec<PathBuf>,
    targets: Vec<usize>,
}

pub fn captures_dir() -> PathBuf {
    state_dir().join("captures")
}

pub fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_markdown_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

fn collect_markdown_in_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_markdown_file(&path) && !out.contains(&path) {
            out.push(path);
        }
    }
}

fn file_sort_key(path: &Path) -> (Option<SystemTime>, String) {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (modified, name)
}

pub fn discover_handoff_files(briefs_dir: Option<&Path>, extra_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(dir) = briefs_dir {
        collect_markdown_in_dir(dir, &mut files);
    }
    if let Some(dir) = extra_dir.filter(|d| briefs_dir != Some(*d)) {
        collect_markdown_in_dir(dir, &mut files);
    }
    files.sort_by(|left, right| file_sort_key(right).cmp(&file_sort_key(left)));
    files.truncate(MAX_DISCOVERED_FILES);
    files
}

pub fn list_handoff_files(briefs_dir: Option<&Path>) -> Vec<PathBuf> {
    let cap_dir = captures_dir();
    discover_handoff_files(briefs_dir, Some(&cap_dir))
}

pub fn tail_non_empty_lines(raw: &str, max_lines: usize) -> Vec<&str> {
    let non_empty: Vec<&str> = raw
        .lines()
        .map(|line| line.trim_end_matches([' ', '\t', '\r']))
        .filter(|line| !line.trim().is_empty())
        .collect();
    let skip = non_empty.len().saturating_sub(max_lines);
    non_empty[skip..].to_vec()
}

fn code_fence_for(lines: &[&str]) -> String {
    let max_ticks = lines
        .iter()
        .flat_map(|line| line.split(|c| c != '`'))
        .map(str::len)
        .max()
        .unwrap_or(0);
    "`".repeat(max_ticks.max(2) + 1)
}

fn format_utc_timestamp(secs: u64) -> String {
    let day_secs = secs % 86_400;
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (yoe as i64) + era * 400 + i64::from(month <= 2);
    let (h, m, s) = (day_secs / 3600, (day_secs % 3600) / 60, day_secs % 60);
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02} UTC")
}

pub fn format_capture_markdown(meta: &CaptureMeta<'_>, raw_output: &str) -> String {
    let title = if meta.title.trim().is_empty() { "Терминал" } else { meta.title.trim() };
    let host = if meta.ssh_host.is_empty() { "local" } else { meta.ssh_host };
    let dir = if meta.cwd.is_empty() { "~".to_owned() } else { shorten_home(meta.cwd) };
    let ts_label = format_utc_timestamp(meta.timestamp_secs);
    let lines = tail_non_empty_lines(raw_output, CAPTURE_MAX_LINES);
    let fence = code_fence_for(&lines);
    let body = if lines.is_empty() {
        "(вывод терминала пуст)".to_owned()
    } else {
        lines.join("\n")
    };
    format!(
        "# Handoff: {title}\n\n- Время: {ts_label}\n- Сессия: {title}\n- Хост: {host}\n- Каталог: {dir}\n\n## Вывод терминала\n\n{fence}text\n{body}\n{fence}\n"
    )
}

fn unique_capture_path(dir: &Path, ts: u64) -> PathBuf {
    let primary = dir.join(format!("capture_{ts}.md"));
    if !primary.exists() {
        return primary;
    }
    (2..1000u32)
        .map(|seq| dir.join(format!("capture_{ts}_{seq}.md")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(primary)
}

pub fn write_capture_file(
    briefs_dir: Option<&Path>,
    fallback_dir: &Path,
    meta: &CaptureMeta<'_>,
    raw_output: &str,
) -> io::Result<PathBuf> {
    let target_dir = briefs_dir.unwrap_or(fallback_dir);
    let path = unique_capture_path(target_dir, meta.timestamp_secs);
    let markdown = format_capture_markdown(meta, raw_output);
    write_state_file(&path, &markdown)?;
    Ok(path)
}

fn take_vte_cstr(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    unsafe { g_free(ptr as *mut c_void) };
    Some(text)
}

pub fn extract_vte_text(vte: *mut VteTerminal) -> String {
    if vte.is_null() {
        return String::new();
    }
    unsafe {
        let (mut col, mut cursor_row) = (0isize, 0isize);
        vte_terminal_get_cursor_position(vte, &mut col, &mut cursor_row);
        let rows = vte_terminal_get_row_count(vte);
        let cols = vte_terminal_get_column_count(vte).max(256);
        let end_row = cursor_row.saturating_add(rows).max(rows.saturating_sub(1)).max(0);
        let start_row = (end_row - VTE_CAPTURE_ROWS).max(0);
        let mut len = 0usize;
        let ranged = take_vte_cstr(vte_terminal_get_text_range_format(
            vte, VTE_FORMAT_TEXT, start_row, 0, end_row, cols, &mut len,
        ));
        if let Some(txt) = ranged.filter(|s| !s.trim().is_empty()) {
            return txt;
        }
        take_vte_cstr(vte_terminal_get_text_format(vte, VTE_FORMAT_TEXT)).unwrap_or_default()
    }
}

pub fn save_vte_capture(
    vte: *mut VteTerminal,
    meta: &CaptureMeta<'_>,
    briefs_dir: Option<&Path>,
) -> io::Result<PathBuf> {
    let raw = extract_vte_text(vte);
    let fallback = captures_dir();
    write_capture_file(briefs_dir, &fallback, meta, &raw)
}

pub fn format_handoff_prompt(target_path: &str) -> String {
    format!("Прочитай файл {target_path} и действуй по нему.")
}

pub fn remote_tmp_path(local_path: &Path) -> String {
    let raw_name = local_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("handoff.md");
    let clean: String = raw_name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let name = if clean.trim_matches('_').is_empty() { "handoff.md" } else { clean.as_str() };
    format!("/tmp/{name}")
}

pub fn scp_upload_args(local_path: &Path, ssh_host: &str, remote_path: &str) -> Vec<String> {
    let dest = if ssh_host.contains(':') && !ssh_host.starts_with('[') {
        format!("[{ssh_host}]:{remote_path}")
    } else {
        format!("{ssh_host}:{remote_path}")
    };
    [
        "-q",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=4",
        "-o",
        "ClearAllForwardings=yes",
        "--",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([local_path.to_string_lossy().into_owned(), dest])
    .collect()
}

pub fn upload_handoff_scp(local_path: &Path, ssh_host: &str) -> Option<String> {
    if !is_ssh_host(ssh_host) || !local_path.is_file() {
        return None;
    }
    let remote_path = remote_tmp_path(local_path);
    let args = scp_upload_args(local_path, ssh_host, &remote_path);
    let status = Command::new("scp")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    status.success().then_some(remote_path)
}

pub fn resolve_handoff_delivery(local_path: &Path, ssh_host: &str) -> Result<(String, String), String> {
    if ssh_host.is_empty() {
        let shown = shorten_home(&local_path.display().to_string());
        let prompt = format_handoff_prompt(&local_path.display().to_string());
        return Ok((prompt, format!("📋 Handoff → {shown}")));
    }
    let Some(remote_path) = upload_handoff_scp(local_path, ssh_host) else {
        return Err(format!("⚠ Ошибка SCP: не удалось загрузить бриф на {ssh_host}:/tmp/"));
    };
    let prompt = format_handoff_prompt(&remote_path);
    let status = format!("📋 Handoff загружен по SCP на {ssh_host}:{remote_path}");
    Ok((prompt, status))
}

fn make_header(title: &CStr) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(title.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-header");
        lbl
    }
}

fn make_muted_label(title: &CStr) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(title.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "ssh-count");
        lbl
    }
}

fn make_row_button(label: &str, tip: &str, cb: ClickHandler, idx: usize) -> *mut GtkWidget {
    let label_c = CString::new(label).unwrap_or_else(|_| CString::new("handoff.md").unwrap());
    unsafe {
        let btn = gtk_button_new_with_label(label_c.as_ptr());
        set_tooltip(btn, tip);
        connect_click(btn, cb, idx as *mut c_void);
        btn
    }
}

fn format_target_label(target: &HandoffTarget) -> String {
    let suffix = if target.is_active { " (активная)" } else { "" };
    if !target.ssh_host.is_empty() && !target.title.contains(&target.ssh_host) {
        format!("↳ {} [{}]{suffix}", target.title, target.ssh_host)
    } else {
        format!("↳ {}{suffix}", target.title)
    }
}

fn pack_capture_button(list: *mut GtkWidget, cb: ClickHandler) {
    unsafe {
        let cap_btn = gtk_button_new_with_label(text::HANDOFF_CAPTURE_BTN.as_ptr());
        gtk_widget_set_tooltip_text(cap_btn, text::HANDOFF_CAPTURE_TIP.as_ptr());
        connect_click(cap_btn, cb, ptr::null_mut());
        gtk_box_pack_start(list, cap_btn, 0, 1, 0);
    }
}

fn wrap_popover_scroll(list: *mut GtkWidget, item_count: usize) -> *mut GtkWidget {
    if item_count <= 12 {
        return list;
    }
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, 380);
        gtk_scrolled_window_set_min_content_height(scroll, 340);
        gtk_scrolled_window_set_max_content_height(scroll, 420);
        gtk_container_add(scroll, list);
        scroll
    }
}

fn populate_targets(list: *mut GtkWidget, targets: &[HandoffTarget], cb: ClickHandler) {
    if targets.is_empty() {
        return;
    }
    unsafe { gtk_box_pack_start(list, make_header(text::HANDOFF_FORWARD_HEADER), 0, 0, 0) };
    for (idx, target) in targets.iter().enumerate() {
        let lbl = format_target_label(target);
        let btn = make_row_button(&lbl, &target.title, cb, idx);
        unsafe { gtk_box_pack_start(list, btn, 0, 1, 0) };
    }
}

fn populate_files(list: *mut GtkWidget, files: &[PathBuf], cb: ClickHandler) {
    unsafe { gtk_box_pack_start(list, make_header(text::HANDOFF_BRIEFS_HEADER), 0, 0, 0) };
    if files.is_empty() {
        unsafe { gtk_box_pack_start(list, make_muted_label(text::HANDOFF_EMPTY), 0, 1, 2) };
        return;
    }
    for (idx, path) in files.iter().enumerate() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("handoff.md");
        let tip = shorten_home(&path.display().to_string());
        let btn = make_row_button(name, &tip, cb, idx);
        unsafe { gtk_box_pack_start(list, btn, 0, 1, 0) };
    }
}

pub fn rebuild_handoff_widgets(
    widgets: HandoffWidgets,
    files: &[PathBuf],
    targets: &[HandoffTarget],
    cbs: HandoffCallbacks,
) -> *mut GtkWidget {
    unsafe {
        if !widgets.old_inner.is_null() {
            gtk_widget_destroy(widgets.old_inner);
        }
        let list = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        pack_capture_button(list, cbs.on_capture);
        populate_targets(list, targets, cbs.on_forward);
        populate_files(list, files, cbs.on_inject);
        let root_child = wrap_popover_scroll(list, targets.len() + files.len());
        gtk_box_pack_start(widgets.outer_box, root_child, 1, 1, 0);
        gtk_widget_show_all(widgets.outer_box);
        root_child
    }
}

impl HandoffMenu {
    pub fn new(button: *mut GtkWidget, on_open: ClickHandler) -> Self {
        unsafe {
            let popover = gtk_popover_new(button);
            let outer_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
            add_class(outer_box, "popover-box");
            gtk_container_add(popover, outer_box);
            connect_click(button, on_open, ptr::null_mut());
            Self {
                popover,
                outer_box,
                inner_box: ptr::null_mut(),
                files: Vec::new(),
                targets: Vec::new(),
            }
        }
    }

    pub fn popover(&self) -> *mut GtkWidget {
        self.popover
    }

    pub fn take_widgets(&mut self) -> HandoffWidgets {
        HandoffWidgets {
            popover: self.popover,
            outer_box: self.outer_box,
            old_inner: std::mem::replace(&mut self.inner_box, ptr::null_mut()),
        }
    }

    pub fn save_snapshot(&mut self, inner_box: *mut GtkWidget, files: Vec<PathBuf>, targets: Vec<usize>) {
        self.inner_box = inner_box;
        self.files = files;
        self.targets = targets;
    }

    pub fn file_at(&self, index: usize) -> Option<PathBuf> {
        self.files.get(index).cloned()
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub fn target_at(&self, index: usize) -> Option<usize> {
        self.targets.get(index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;
    use std::time::Duration;

    fn make_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("akhterm_handoff_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn discovers_markdown_across_briefs_and_captures_newest_first() {
        let briefs = make_temp_dir("briefs");
        let caps = make_temp_dir("caps");
        fs::write(briefs.join("notes.txt"), "ignore").unwrap();
        fs::create_dir_all(briefs.join("folder.md")).unwrap();
        let older = briefs.join("z_older_brief.md");
        fs::write(&older, "# old").unwrap();
        thread::sleep(Duration::from_millis(25));
        let middle = caps.join("capture_100.md");
        fs::write(&middle, "# mid").unwrap();
        thread::sleep(Duration::from_millis(25));
        let newest = briefs.join("a_newest_brief.MD");
        fs::write(&newest, "# new").unwrap();

        let found = discover_handoff_files(Some(&briefs), Some(&caps));
        assert_eq!(found, vec![newest.clone(), middle, older]);
        let dedup = discover_handoff_files(Some(&briefs), Some(&briefs));
        assert_eq!(dedup.len(), 2);

        let _ = fs::remove_dir_all(&briefs);
        let _ = fs::remove_dir_all(&caps);
    }

    #[test]
    fn truncates_terminal_capture_to_eighty_non_empty_lines_and_escapes_fences() {
        let mut raw = String::new();
        for idx in 1..=105 {
            raw.push_str(&format!("  line {idx:03}   \r\n\n"));
        }
        raw.push_str("```rust\nfn demo() {}\n```\n");
        let meta = CaptureMeta {
            title: "akhterm · AGY",
            cwd: "/tmp/work",
            ssh_host: "",
            timestamp_secs: 1_762_809_263,
        };
        let md = format_capture_markdown(&meta, &raw);
        assert!(md.contains("# Handoff: akhterm · AGY"));
        assert!(md.contains("- Хост: local"));
        assert!(md.contains("````text"));
        assert!(!md.contains("  line 028"));
        assert!(md.contains("  line 029"));
        assert!(md.contains("  line 105"));
        assert_eq!(tail_non_empty_lines(&raw, CAPTURE_MAX_LINES).len(), 80);
    }

    #[test]
    fn writes_timestamped_capture_atomically_with_0600_permissions() {
        let briefs = make_temp_dir("perm_briefs");
        let fallback = make_temp_dir("perm_caps");
        let meta = CaptureMeta {
            title: "devbox · Claude",
            cwd: "/srv/app",
            ssh_host: "devbox",
            timestamp_secs: 1_700_000_500,
        };
        let p1 = write_capture_file(Some(&briefs), &fallback, &meta, "err 1\nerr 2\n").unwrap();
        let p2 = write_capture_file(Some(&briefs), &fallback, &meta, "err 3\n").unwrap();
        let p3 = write_capture_file(None, &fallback, &meta, "fallback capture\n").unwrap();

        assert_eq!(p1.file_name().unwrap(), "capture_1700000500.md");
        assert_eq!(p2.file_name().unwrap(), "capture_1700000500_2.md");
        assert_eq!(p3.parent().unwrap(), fallback.as_path());
        assert_eq!(fs::metadata(&p1).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&p3).unwrap().permissions().mode() & 0o777, 0o600);

        let _ = fs::remove_dir_all(&briefs);
        let _ = fs::remove_dir_all(&fallback);
    }

    #[test]
    fn builds_handoff_prompts_and_scp_arguments_for_local_and_remote_tabs() {
        let local_file = Path::new("/tmp/бриф задачи 1.md");
        assert_eq!(remote_tmp_path(local_file), "/tmp/бриф_задачи_1.md");
        assert_eq!(
            format_handoff_prompt("/tmp/capture_42.md"),
            "Прочитай файл /tmp/capture_42.md и действуй по нему."
        );
        let args = scp_upload_args(Path::new("/tmp/c.md"), "devbox", "/tmp/c.md");
        assert!(args.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(args.windows(2).any(|w| w == ["-o", "ConnectTimeout=4"]));
        assert!(args.windows(2).any(|w| w == ["-o", "ClearAllForwardings=yes"]));
        assert_eq!(args.last().unwrap(), "devbox:/tmp/c.md");
        let ipv6 = scp_upload_args(Path::new("/tmp/c.md"), "fe80::1", "/tmp/c.md");
        assert_eq!(ipv6.last().unwrap(), "[fe80::1]:/tmp/c.md");

        let (prompt, status) = resolve_handoff_delivery(Path::new("/tmp/a.md"), "").unwrap();
        assert_eq!(prompt, "Прочитай файл /tmp/a.md и действуй по нему.");
        assert!(status.contains("Handoff"));
    }

    #[test]
    fn extract_vte_text_preserves_screen_below_cursor_after_scrollback() {
        if std::env::var_os("AKHTERM_HANDOFF_VTE_CHILD").is_none() {
            let exe = std::env::current_exe().unwrap();
            let status = Command::new(exe)
                .args(["--exact", "handoff::tests::extract_vte_text_preserves_screen_below_cursor_after_scrollback"])
                .env("AKHTERM_HANDOFF_VTE_CHILD", "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        run_isolated_vte_cursor_test();
    }

    fn spawn_vte_broadway(dir: &Path) -> Option<std::process::Child> {
        let child = Command::new("broadwayd")
            .arg("-u")
            .arg(dir.join("http.sock"))
            .arg(":13")
            .env("XDG_RUNTIME_DIR", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let gdk_sock = dir.join("broadway14.socket");
        for _ in 0..80 {
            if gdk_sock.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        Some(child)
    }

    fn pump_vte_events() {
        thread::sleep(Duration::from_millis(50));
        for _ in 0..80 {
            if unsafe { gtk_events_pending() } == 0 {
                break;
            }
            unsafe { gtk_main_iteration_do(0) };
        }
    }

    fn run_isolated_vte_cursor_test() {
        let dir = make_temp_dir("bway13");
        let Some(mut bway) = spawn_vte_broadway(&dir) else {
            let _ = fs::remove_dir_all(&dir);
            return;
        };
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        std::env::set_var("GDK_BACKEND", "broadway");
        std::env::set_var("BROADWAY_DISPLAY", ":13");
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } != 0 {
            verify_vte_scrollback_and_cursor_home();
        }
        let _ = bway.kill();
        let _ = bway.wait();
        let _ = fs::remove_dir_all(&dir);
    }

    fn verify_vte_scrollback_and_cursor_home() {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            gtk_window_set_default_size(win, 800, 600);
            let vte = vte_terminal_new();
            vte_terminal_set_scrollback_lines(vte, 15_000);
            gtk_container_add(win, vte);
            gtk_widget_show_all(win);
            pump_vte_events();
            for idx in 1..=120 {
                let line = format!("row_{idx:03}\r\n");
                vte_terminal_feed(vte, line.as_ptr() as *const c_char, line.len() as isize);
            }
            pump_vte_events();
            let home_seq = "\x1b[HTOP_CURSOR";
            vte_terminal_feed(vte, home_seq.as_ptr() as *const c_char, home_seq.len() as isize);
            pump_vte_events();
            let captured = extract_vte_text(vte);
            assert!(captured.contains("TOP_CURSOR"), "missing top cursor line: {captured:?}");
            assert!(captured.contains("row_120"), "missing bottom visible row: {captured:?}");
            assert!(captured.contains("row_050"), "missing scrollback row: {captured:?}");
            gtk_widget_destroy(win);
        }
    }

    #[test]
    fn module_maintainability_index_stays_high() {
        let (_, _, handoff_mi) = crate::mi::compute_file_mi(include_str!("handoff.rs"));
        let (_, _, main_mi) = crate::mi::compute_file_mi(include_str!("main.rs"));
        assert!(handoff_mi >= 58.0, "handoff.rs MI too low: {handoff_mi:.2}");
        assert!(main_mi >= 54.78, "main.rs MI regressed: {main_mi:.2}");
    }
}
