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
