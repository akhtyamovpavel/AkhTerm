use crate::ffi::GtkWidget;
use std::ffi::{c_char, c_int, c_void};
use std::ptr;

#[repr(C)]
#[derive(Default)]
struct Iter {
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
    fn gtk_text_buffer_set_text(buffer: *mut c_void, text: *const c_char, len: c_int);
    fn gtk_text_buffer_create_tag(
        buffer: *mut c_void,
        name: *const c_char,
        property: *const c_char,
        ...
    ) -> *mut c_void;
    fn gtk_text_buffer_apply_tag(buffer: *mut c_void, tag: *mut c_void, start: &Iter, end: &Iter);
    fn gtk_text_buffer_get_iter_at_offset(buffer: *mut c_void, iter: &mut Iter, offset: c_int);
    fn gtk_text_buffer_get_selection_bounds(
        buffer: *mut c_void,
        start: &mut Iter,
        end: &mut Iter,
    ) -> c_int;
    fn gtk_text_buffer_select_range(buffer: *mut c_void, cursor: &Iter, bound: &Iter);
    fn gtk_text_iter_get_offset(iter: &Iter) -> c_int;
    fn gtk_text_view_scroll_to_iter(
        view: *mut GtkWidget,
        iter: &mut Iter,
        margin: f64,
        align: c_int,
        x: f64,
        y: f64,
    ) -> c_int;
}

pub struct Buffer {
    pub view: *mut GtkWidget,
    buffer: *mut c_void,
    heading: *mut c_void,
    code: *mut c_void,
}

impl Buffer {
    pub fn new() -> Self {
        unsafe {
            let view = gtk_text_view_new();
            gtk_text_view_set_editable(view, 0);
            gtk_text_view_set_wrap_mode(view, 3);
            let buffer = gtk_text_view_get_buffer(view);
            let end = ptr::null::<c_char>();
            let heading = gtk_text_buffer_create_tag(
                buffer,
                c"heading".as_ptr(),
                c"weight".as_ptr(),
                700,
                end,
            );
            let code = gtk_text_buffer_create_tag(
                buffer,
                c"code".as_ptr(),
                c"family".as_ptr(),
                c"monospace".as_ptr(),
                end,
            );
            Self {
                view,
                buffer,
                heading,
                code,
            }
        }
    }

    fn iter(&self, offset: i32) -> Iter {
        let mut iter = Iter::default();
        unsafe { gtk_text_buffer_get_iter_at_offset(self.buffer, &mut iter, offset) };
        iter
    }

    pub fn bounds(&self) -> (i32, i32) {
        let (mut start, mut end) = (Iter::default(), Iter::default());
        unsafe {
            gtk_text_buffer_get_selection_bounds(self.buffer, &mut start, &mut end);
            (
                gtk_text_iter_get_offset(&start),
                gtk_text_iter_get_offset(&end),
            )
        }
    }

    pub fn select(&self, start: i32, end: i32, scroll: bool) {
        let mut first = self.iter(start);
        let last = self.iter(end);
        unsafe {
            gtk_text_buffer_select_range(self.buffer, &last, &first);
            if scroll {
                gtk_text_view_scroll_to_iter(self.view, &mut first, 0.1, 0, 0.0, 0.0);
            }
        }
    }

    pub fn render(&self, text: &str, blocks: &[(usize, usize)]) {
        unsafe { gtk_text_buffer_set_text(self.buffer, text.as_ptr().cast(), text.len() as c_int) };
        let mut offset = 0;
        let mut byte = 0;
        let mut index = 0;
        for line in text.split_inclusive('\n') {
            let next = offset + line.chars().count() as i32;
            while index < blocks.len() && blocks[index].1 <= byte {
                index += 1;
            }
            let is_code = blocks.get(index).is_some_and(|&(start, _)| start <= byte);
            let tag = if is_code {
                self.code
            } else if line.trim_start().starts_with('#') {
                self.heading
            } else {
                ptr::null_mut()
            };
            if !tag.is_null() {
                unsafe {
                    gtk_text_buffer_apply_tag(
                        self.buffer,
                        tag,
                        &self.iter(offset),
                        &self.iter(next),
                    )
                };
            }
            byte += line.len();
            offset = next;
        }
    }
}
