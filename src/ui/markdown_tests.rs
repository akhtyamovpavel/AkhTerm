use super::*;
use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

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
    let mut ran = false;
    with_headless_gtk(|| {
        ran = true;
        let path = std::env::temp_dir().join(format!("akhterm-reader-{}.md", std::process::id()));
        fs::write(&path, "# Задача\n\n```rs\nкод\n```\n").unwrap();
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let parent = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0) };
        let terminal = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 0) };
        unsafe { gtk_container_add(win, parent) };
        mount(parent, terminal);
        unsafe { gtk_widget_show_all(win) };
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
        unsafe { gtk_widget_destroy(win) };
        assert!(READER.with_borrow(Option::is_none));
        fs::remove_file(path).unwrap();
    });
    if std::env::var_os("AKHTERM_REQUIRE_GTK").is_some() {
        assert!(ran, "GTK display required");
    }
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

#[test]
fn same_relative_markdown_tracks_the_clicked_tmux_pane_and_connection() {
    let mut p = test_pane();
    p.proc_info.ssh_pid = 123;
    p.ssh_directory = Some((123, "/old".into()));
    let info = p.proc_info.clone();
    let file = remote::candidate("docs/план.md:17:2").unwrap();
    let mut previous = None;
    for (pane, directory) in [("%7", "/srv/one"), ("%8", "/srv/two")] {
        let payload = format!("v2:225:{pane}|main|0:dev|opencode|{directory}");
        crate::tmux_local::apply_pane_tmux_title(&mut p, &payload);
        let title = format!("AKHTERM_TMUX:{payload}");
        assert_eq!(
            origin::remote_directory(&p, &info, "AKHTERM_TMUX:%99|main|0:dev|opencode|/wrong"),
            None
        );
        assert_eq!(p.tmux_server, Some(225));
        let key = Origin::new(&p, &info);
        assert_ne!(previous.as_ref(), Some(&key));
        previous = Some(key);
        assert_eq!(
            remote::resolve(&file, origin::remote_directory(&p, &info, &title)),
            Some(Path::new(directory).join(&file))
        );
    }
    let next = crate::proc_radar::ProcInfo {
        ssh_pid: 456,
        ..info
    };
    assert_eq!(origin::remote_directory(&p, &next, ""), None);
    p.sync_connection(&next);
    assert!(p.ssh_directory.is_none());
    assert_eq!(remote::resolve(&file, None), None);
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
fn late_remote_result_cannot_replace_the_new_document_or_reopen_a_closed_preview() {
    with_headless_gtk(|| {
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let parent = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0) };
        unsafe { gtk_container_add(win, parent) };
        mount(parent, unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 0) });
        let p = test_pane();
        let doc = RemoteDocument {
            origin: Origin::new(&p, &p.proc_info),
            host: "dev".into(),
            connection: None,
            path: "plan.md".into(),
        };
        open_remote(doc.clone());
        let request = || READER.with_borrow(|r| r.as_ref().unwrap().request.unwrap());
        let old = request();
        READER.with_borrow(|r| {
            assert_ne!(
                unsafe { gtk_widget_get_visible(r.as_ref().unwrap().entry) },
                0
            )
        });
        open_remote(RemoteDocument {
            path: "/srv/b/plan.md".into(),
            ..doc
        });
        let new = request();
        remote_reader::complete(new, Ok("вторая панель".into()));
        remote_reader::complete(old, Ok("первая панель".into()));
        READER.with_borrow(|r| assert_eq!(r.as_ref().unwrap().source, "вторая панель"));
        close(ptr::null_mut(), ptr::null_mut());
        remote_reader::complete(new, Ok("поздно".into()));
        READER.with_borrow(|r| {
            assert_eq!(
                unsafe { gtk_widget_get_visible(r.as_ref().unwrap().root) },
                0
            )
        });
        unsafe { gtk_widget_destroy(win) }
    });
}

#[test]
fn local_and_ssh_reads_enforce_the_same_text_limit() {
    assert!(file::decode(vec![b'x'; file::MAX_BYTES as usize]).is_ok());
    assert!(file::decode(vec![b'x'; file::MAX_BYTES as usize + 1]).is_err());
    assert!(file::decode(vec![0xff]).is_err());
    assert!(file::decode(vec![b'x', 0]).is_err());
}

#[test]
fn tmux_inserts_only_into_the_selected_live_recipient() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    if Command::new("tmux").arg("-V").output().is_err() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("akhterm-quote-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("socket");
    let tmux = |args: &[&str]| {
        let out = Command::new("tmux")
            .args(["-S"])
            .arg(&sock)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .arg("-S")
                .arg(self.0.join("socket"))
                .arg("kill-server")
                .output();
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let id = tmux(&[
        "-f",
        "/dev/null",
        "new-session",
        "-dP",
        "-F",
        "#{pane_id}",
        "cat",
    ]);
    let server: u32 = tmux(&["display-message", "-p", "#{pid}"]).parse().unwrap();
    let send = |agent, text: &str| {
        let target = remote::TmuxAddress::new(server, &id, agent).unwrap();
        let mut shell = Command::new("sh")
            .args(["-c", &target.paste_command()])
            .env("TMUX", format!("{},{server},0", sock.display()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        shell
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let out = shell.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(send("cat", "first passage"), "inserted\n");
    assert_eq!(send("opencode", "wrong process"), "stale\n");
    let next = tmux(&["split-window", "-P", "-F", "#{pane_id}", "cat"]);
    assert_eq!(send("cat", "wrong pane"), "stale\n");
    assert!(!tmux(&["capture-pane", "-p", "-t", &next]).contains("wrong pane"));
    let screen = tmux(&["capture-pane", "-p", "-t", &id]);
    assert!(screen.contains("first passage") && !screen.contains("wrong"));
}
