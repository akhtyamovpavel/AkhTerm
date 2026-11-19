use super::*;
use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
use std::io::Write;
use std::process::{Command, Stdio};

fn mount_test_reader_window(show_all: bool) -> *mut GtkWidget {
    unsafe {
        let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        let parent = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
        let terminal = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_container_add(win, parent);
        mount(parent, terminal);
        if show_all {
            gtk_widget_show_all(win);
        }
        win
    }
}

fn with_mounted_reader(show_all: bool, body: impl FnOnce()) -> bool {
    let mut ran = false;
    with_headless_gtk(|| {
        ran = true;
        let win = mount_test_reader_window(show_all);
        body();
        unsafe { gtk_widget_destroy(win) };
        assert!(READER.with_borrow(Option::is_none));
    });
    ran
}

fn assert_gtk_ran_if_required(ran: bool) {
    if std::env::var_os("AKHTERM_REQUIRE_GTK").is_some() {
        assert!(ran, "GTK display required");
    }
}

fn reader_request_id() -> u64 {
    READER.with_borrow(|r| r.as_ref().unwrap().request.unwrap())
}

fn is_reader_widget_visible(pick: impl FnOnce(&Reader) -> *mut GtkWidget) -> bool {
    READER.with_borrow(|r| {
        let widget = pick(r.as_ref().unwrap());
        unsafe { gtk_widget_get_visible(widget) != 0 }
    })
}

#[test]
fn longer_fences_preserve_nested_fences_and_cyrillic() {
    let source = "# Пример\n\n````md\n```rust\nкод\n```\n````\n";
    let blocks = file::code_blocks(source);
    assert_eq!(blocks.len(), 1);
    let (start, end) = blocks[0];
    assert_eq!(&source[start..end], "```rust\nкод\n```\n");
}

#[test]
fn finds_recent_markdown_with_spaces_in_terminal_output() {
    let dir = std::env::temp_dir().join(format!("akhterm-md-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Новый файл.md");
    fs::write(&path, "# Готово").unwrap();
    assert_eq!(
        file::last_path("Saved [файл](Новый файл.md)", dir.to_str().unwrap()),
        Some(path.clone())
    );
    fs::remove_file(path).unwrap();
    fs::remove_dir(dir).unwrap();
}

#[test]
fn gtk_reader_searches_cyrillic_preserves_selection_and_reloads() {
    let ran = with_mounted_reader(true, || {
        let path = std::env::temp_dir().join(format!("akhterm-reader-{}.md", std::process::id()));
        fs::write(&path, "# Задача\n\n```rs\nкод\n```\n").unwrap();
        open(&path);
        let entry = READER.with_borrow(|slot| slot.as_ref().unwrap().search);
        unsafe { gtk_entry_set_text(entry, c"код".as_ptr()) };
        pump_gtk_events();
        fs::write(&path, "# Задача\n\n```rs\nкод\n```\nещё строка\n").unwrap();
        reload();
        READER.with_borrow(|slot| {
            let r = slot.as_ref().unwrap();
            assert_eq!(r.buffer.bounds(), (16, 19));
            assert!(r.source.ends_with("ещё строка\n"));
        });
        fs::remove_file(path).unwrap();
    });
    assert_gtk_ran_if_required(ran);
}

#[test]
fn quotes_source_lines_without_submitting_and_handles_nested_fences() {
    let source = "дом\nпример ```\nконец\nещё";
    let quote = quote::format_quote("box:/srv/план.md", source, 4, 21).unwrap();
    assert_eq!(
        quote,
        "box:/srv/план.md#L2-L3\n````\nпример ```\nконец\n````"
    );
    assert!(quote::format_quote("p.md", "one\x1btwo", 0, 7).is_none());
    assert!(quote::format_quote("p.md", "текст", 1, 1).is_none());
    assert_eq!(
        quote::format_quote("a.md", "a\r\nb", 0, 4).unwrap(),
        "a.md#L1-L2\n```\na\nb\n```"
    );
}

fn test_pane() -> crate::pane_registry::PaneRecord {
    crate::pane_registry::PaneRecord::new(
        crate::pane_registry::PaneId(1),
        ptr::null_mut(),
        ptr::null_mut(),
        11,
        "/home/local".into(),
    )
}

fn assert_tmux_pane_tracks_directory(
    p: &mut crate::pane_registry::PaneRecord,
    info: &crate::proc_radar::ProcInfo,
    file: &Path,
    pane: &str,
    directory: &str,
) -> Origin {
    let payload = format!("v2:225:{pane}|main|0:dev|opencode|{directory}");
    crate::tmux_local::apply_pane_tmux_title(p, &payload);
    let title = format!("AKHTERM_TMUX:{payload}");
    assert_eq!(
        origin::remote_directory(p, info, "AKHTERM_TMUX:%99|main|0:dev|opencode|/wrong"),
        None
    );
    assert_eq!(p.tmux_server, Some(225));
    assert_eq!(
        remote::resolve(file, origin::remote_directory(p, info, &title)),
        Some(Path::new(directory).join(file))
    );
    Origin::new(p, info)
}

fn assert_remote_path_fallbacks(file: &Path) {
    assert_eq!(remote::resolve(file, None), None);
    assert_eq!(
        remote::resolve(Path::new("/srv/absolute.md"), None),
        Some("/srv/absolute.md".into())
    );
    assert_eq!(remote::candidate("https://example.org/plan.md"), None);
    assert_eq!(
        remote::candidate("file://server/srv/a%20b.md"),
        Some("/srv/a b.md".into())
    );
}

#[test]
fn same_relative_markdown_tracks_the_clicked_tmux_pane_and_connection() {
    let mut p = test_pane();
    p.proc_info.ssh_pid = 123;
    p.ssh_directory = Some((123, "/old".into()));
    let info = p.proc_info.clone();
    let file = remote::candidate("docs/план.md:17:2").unwrap();
    let first = assert_tmux_pane_tracks_directory(&mut p, &info, &file, "%7", "/srv/one");
    let second = assert_tmux_pane_tracks_directory(&mut p, &info, &file, "%8", "/srv/two");
    assert_ne!(first, second);
    let next = crate::proc_radar::ProcInfo {
        ssh_pid: 456,
        ..info
    };
    assert_eq!(origin::remote_directory(&p, &next, ""), None);
    p.sync_connection(&next);
    assert!(p.ssh_directory.is_none());
    assert_remote_path_fallbacks(&file);
}

fn sample_remote_doc(pane: &crate::pane_registry::PaneRecord, path: &str) -> RemoteDocument {
    RemoteDocument {
        origin: Origin::new(pane, &pane.proc_info),
        host: "dev".into(),
        connection: None,
        path: path.into(),
    }
}

#[test]
fn late_remote_result_cannot_replace_the_new_document_or_reopen_a_closed_preview() {
    with_mounted_reader(false, || {
        let p = test_pane();
        open_remote(sample_remote_doc(&p, "plan.md"));
        let old = reader_request_id();
        assert!(is_reader_widget_visible(|r| r.entry));
        open_remote(sample_remote_doc(&p, "/srv/b/plan.md"));
        let new = reader_request_id();
        remote_reader::complete(new, Ok("вторая панель".into()));
        remote_reader::complete(old, Ok("первая панель".into()));
        READER.with_borrow(|r| assert_eq!(r.as_ref().unwrap().source, "вторая панель"));
        close(ptr::null_mut(), ptr::null_mut());
        remote_reader::complete(new, Ok("поздно".into()));
        assert!(!is_reader_widget_visible(|r| r.root));
    });
}

#[test]
fn local_and_ssh_reads_enforce_the_same_text_limit() {
    use std::io::{repeat, Read};
    let limit = file::MAX_BYTES;
    assert!(file::read_text(repeat(b'x').take(limit)).is_ok());
    let mut oversized = repeat(b'x').take(limit + 20);
    assert!(file::read_text(&mut oversized).is_err());
    assert_eq!(oversized.limit(), 19);
    assert!(file::read_text(&[0xff][..]).is_err());
    assert!(file::read_text(&b"x\0"[..]).is_err());
}

struct TmuxFixture {
    dir: PathBuf,
    sock: PathBuf,
}

impl TmuxFixture {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("akhterm-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("socket");
        Self { dir, sock }
    }

    fn run(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-S"])
            .arg(&self.sock)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn start_cat_pane(&self) -> (String, u32) {
        let id = self.run(&[
            "-f", "/dev/null", "new-session", "-dP", "-F", "#{pane_id}", "cat",
        ]);
        let server = self.run(&["display-message", "-p", "#{pid}"]).parse().unwrap();
        (id, server)
    }

    fn send_quote(&self, server: u32, id: &str, agent: &str, text: &str) -> String {
        let target = remote::TmuxAddress::new(server, id, agent).unwrap();
        let mut shell = Command::new("sh")
            .args(["-c", &target.paste_command()])
            .env("TMUX", format!("{},{server},0", self.sock.display()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        shell.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        let out = shell.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }
}

impl Drop for TmuxFixture {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.sock)
            .arg("kill-server")
            .output();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn tmux_inserts_only_into_the_selected_live_recipient() {
    if Command::new("tmux").arg("-V").output().is_err() {
        return;
    }
    let tmux = TmuxFixture::new("quote");
    let (id, server) = tmux.start_cat_pane();
    assert_eq!(tmux.send_quote(server, &id, "cat", "first passage"), "inserted\n");
    assert_eq!(tmux.send_quote(server, &id, "opencode", "wrong process"), "stale\n");
    let next = tmux.run(&["split-window", "-P", "-F", "#{pane_id}", "cat"]);
    assert_eq!(tmux.send_quote(server, &id, "cat", "wrong pane"), "stale\n");
    assert!(!tmux.run(&["capture-pane", "-p", "-t", &next]).contains("wrong pane"));
    let screen = tmux.run(&["capture-pane", "-p", "-t", &id]);
    assert!(screen.contains("first passage") && !screen.contains("wrong"));
}
