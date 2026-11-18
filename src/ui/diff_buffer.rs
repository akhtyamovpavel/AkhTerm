//! Colored GTK TextView buffer for unified diffs with Dark/Light Zinc tag palettes.

use super::parser::{RenderedDiff, RenderedLineKind};
use crate::ffi::{add_class, g_object_set, GtkWidget};
use crate::theme_mode::ThemeMode;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr;

#[repr(C)]
#[derive(Default)]
struct TextIter {
    pointers: [usize; 2],
    integers: [c_int; 6],
    more_pointers: [usize; 2],
    more_integers: [c_int; 3],
    padding: usize,
}

extern "C" {
    fn gtk_text_view_new() -> *mut GtkWidget;
    fn gtk_text_view_get_buffer(view: *mut GtkWidget) -> *mut c_void;
    fn gtk_text_view_set_editable(view: *mut GtkWidget, editable: c_int);
    fn gtk_text_view_set_wrap_mode(view: *mut GtkWidget, mode: c_int);
    fn gtk_text_view_set_monospace(view: *mut GtkWidget, monospace: c_int);
    fn gtk_text_buffer_set_text(buffer: *mut c_void, text: *const c_char, len: c_int);
    fn gtk_text_buffer_create_tag(buffer: *mut c_void, name: *const c_char, prop: *const c_char, ...) -> *mut c_void;
    fn gtk_text_buffer_apply_tag(buffer: *mut c_void, tag: *mut c_void, start: &TextIter, end: &TextIter);
    fn gtk_text_buffer_get_iter_at_offset(buffer: *mut c_void, iter: &mut TextIter, offset: c_int);
    fn gtk_text_buffer_get_selection_bounds(buffer: *mut c_void, start: &mut TextIter, end: &mut TextIter) -> c_int;
    fn gtk_text_buffer_select_range(buffer: *mut c_void, cursor: &TextIter, bound: &TextIter);
    fn gtk_text_iter_get_offset(iter: &TextIter) -> c_int;
    fn gtk_text_view_scroll_to_iter(view: *mut GtkWidget, iter: &mut TextIter, margin: f64, align: c_int, x: f64, y: f64) -> c_int;
}

struct TagColors {
    add_fg: &'static CStr,
    add_bg: &'static CStr,
    del_fg: &'static CStr,
    del_bg: &'static CStr,
    hunk_fg: &'static CStr,
    hunk_bg: &'static CStr,
    meta_fg: &'static CStr,
    gutter_fg: &'static CStr,
}

fn palette_for(mode: ThemeMode) -> TagColors {
    match mode {
        ThemeMode::Dark => TagColors {
            add_fg: c"#86efac",
            add_bg: c"#11281d",
            del_fg: c"#fda4af",
            del_bg: c"#2d1219",
            hunk_fg: c"#c4b5fd",
            hunk_bg: c"#201a35",
            meta_fg: c"#a1a1aa",
            gutter_fg: c"#71717a",
        },
        ThemeMode::Light => TagColors {
            add_fg: c"#065f46",
            add_bg: c"#dcfce7",
            del_fg: c"#9f1239",
            del_bg: c"#ffe4e6",
            hunk_fg: c"#5b21b6",
            hunk_bg: c"#ede9fe",
            meta_fg: c"#52525b",
            gutter_fg: c"#71717a",
        },
    }
}

pub struct DiffBuffer {
    pub view: *mut GtkWidget,
    buffer: *mut c_void,
    tag_add: *mut c_void,
    tag_del: *mut c_void,
    tag_hunk: *mut c_void,
    tag_meta: *mut c_void,
    tag_gutter: *mut c_void,
}

fn make_tag(buffer: *mut c_void, name: &CStr) -> *mut c_void {
    unsafe { gtk_text_buffer_create_tag(buffer, name.as_ptr(), ptr::null::<c_char>()) }
}

fn style_tag(tag: *mut c_void, fg: &CStr, bg: Option<&CStr>, bold: bool) {
    let end = ptr::null::<c_char>();
    unsafe {
        g_object_set(tag, c"foreground".as_ptr(), fg.as_ptr(), end);
        if let Some(bg_val) = bg {
            g_object_set(tag, c"background".as_ptr(), bg_val.as_ptr(), c"paragraph-background".as_ptr(), bg_val.as_ptr(), end);
        }
        if bold {
            g_object_set(tag, c"weight".as_ptr(), 700 as c_int, end);
        }
    }
}

impl DiffBuffer {
    pub fn new() -> Self {
        unsafe {
            let view = gtk_text_view_new();
            gtk_text_view_set_editable(view, 0);
            gtk_text_view_set_wrap_mode(view, 0);
            gtk_text_view_set_monospace(view, 1);
            add_class(view, "diff-review-view");
            let buffer = gtk_text_view_get_buffer(view);
            let me = Self {
                view,
                buffer,
                tag_add: make_tag(buffer, c"diff_add"),
                tag_del: make_tag(buffer, c"diff_del"),
                tag_hunk: make_tag(buffer, c"diff_hunk"),
                tag_meta: make_tag(buffer, c"diff_meta"),
                tag_gutter: make_tag(buffer, c"diff_gutter"),
            };
            me.apply_theme(ThemeMode::Dark);
            me
        }
    }

    pub fn apply_theme(&self, mode: ThemeMode) {
        let p = palette_for(mode);
        style_tag(self.tag_add, p.add_fg, Some(p.add_bg), false);
        style_tag(self.tag_del, p.del_fg, Some(p.del_bg), false);
        style_tag(self.tag_hunk, p.hunk_fg, Some(p.hunk_bg), true);
        style_tag(self.tag_meta, p.meta_fg, None, true);
        style_tag(self.tag_gutter, p.gutter_fg, None, false);
    }

    fn iter_at(&self, offset: i32) -> TextIter {
        let mut iter = TextIter::default();
        unsafe { gtk_text_buffer_get_iter_at_offset(self.buffer, &mut iter, offset) };
        iter
    }

    fn tag_range(&self, tag: *mut c_void, start: i32, end: i32) {
        if start >= end {
            return;
        }
        let s = self.iter_at(start);
        let e = self.iter_at(end);
        unsafe { gtk_text_buffer_apply_tag(self.buffer, tag, &s, &e) };
    }

    pub fn render(&self, rendered: &RenderedDiff, theme: ThemeMode) {
        self.apply_theme(theme);
        unsafe {
            gtk_text_buffer_set_text(self.buffer, rendered.text.as_ptr().cast(), rendered.text.len() as c_int);
        }
        for line in &rendered.lines {
            match line.kind {
                RenderedLineKind::Added => self.tag_range(self.tag_add, line.char_start, line.char_end),
                RenderedLineKind::Removed => self.tag_range(self.tag_del, line.char_start, line.char_end),
                RenderedLineKind::HunkHeader => self.tag_range(self.tag_hunk, line.char_start, line.char_end),
                RenderedLineKind::FileHeader | RenderedLineKind::NoNewline => {
                    self.tag_range(self.tag_meta, line.char_start, line.char_end);
                }
                RenderedLineKind::Context => {}
            }
            if matches!(line.kind, RenderedLineKind::Context | RenderedLineKind::Added | RenderedLineKind::Removed) {
                let gutter_end = (line.char_start + 12).min(line.char_end);
                self.tag_range(self.tag_gutter, line.char_start, gutter_end);
            }
        }
    }

    pub fn set_message(&self, msg: &str, theme: ThemeMode) {
        self.apply_theme(theme);
        unsafe {
            gtk_text_buffer_set_text(self.buffer, msg.as_ptr().cast(), msg.len() as c_int);
        }
        self.tag_range(self.tag_meta, 0, msg.chars().count() as i32);
    }

    pub fn bounds(&self) -> (i32, i32) {
        let (mut start, mut end) = (TextIter::default(), TextIter::default());
        unsafe {
            gtk_text_buffer_get_selection_bounds(self.buffer, &mut start, &mut end);
            (gtk_text_iter_get_offset(&start), gtk_text_iter_get_offset(&end))
        }
    }

    pub fn select(&self, start: i32, end: i32, scroll: bool) {
        let mut s = self.iter_at(start);
        let e = self.iter_at(end);
        unsafe {
            gtk_text_buffer_select_range(self.buffer, &s, &e);
            if scroll {
                gtk_text_view_scroll_to_iter(self.view, &mut s, 0.12, 0, 0.0, 0.0);
            }
        }
    }
}
