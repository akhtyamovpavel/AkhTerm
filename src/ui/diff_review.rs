//! Side-by-side Local & SSH Diff Review panel with inline comments-to-agent and hunk actions.

#[path = "diff_buffer.rs"]
mod buffer;
#[path = "diff_parser.rs"]
pub mod parser;
#[path = "diff_source.rs"]
pub mod source;
#[cfg(test)]
#[path = "diff_review_tests.rs"]
mod tests;

use crate::app_state;
use crate::ffi::*;
use crate::markdown::{self, Origin, Target};
use crate::theme_mode::ThemeMode;
use crate::ui_text as text;
use parser::{DiffFile, RenderedDiff, ReviewComment};
use source::{DiffLoadResult, DiffMode, DiffTarget};
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::{c_int, c_uint, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

const KEYCODE_B: u16 = 56;
static REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);

extern "C" {
    fn gtk_check_button_new() -> *mut GtkWidget;
    fn gtk_combo_box_text_new() -> *mut GtkWidget;
    fn gtk_combo_box_text_append_text(combo: *mut GtkWidget, text: *const i8);
    fn gtk_combo_box_text_remove_all(combo: *mut GtkWidget);
    fn gtk_combo_box_get_active(combo: *mut GtkWidget) -> c_int;
    fn gtk_combo_box_set_active(combo: *mut GtkWidget, index: c_int);
    fn gtk_widget_is_ancestor(widget: *mut GtkWidget, ancestor: *mut GtkWidget) -> c_int;
}

struct AgentPicker {
    combo: *mut GtkWidget,
    targets: Vec<Target>,
}

impl AgentPicker {
    fn new() -> Self {
        let combo = unsafe { gtk_combo_box_text_new() };
        Self { combo, targets: Vec::new() }
    }

    fn selected(&self) -> Option<Target> {
        let index = unsafe { gtk_combo_box_get_active(self.combo) };
        self.targets.get(usize::try_from(index - 1).ok()?).cloned()
    }

    fn reset(&mut self, origin: Option<&Origin>) {
        let targets = markdown::targets();
        let matched = targets.iter().position(|t| Some(&t.origin) == origin);
        let index = matched.or_else(|| (targets.len() == 1).then_some(0));
        self.populate(targets, index);
    }

    fn sync(&mut self) {
        let targets = markdown::targets();
        if targets != self.targets {
            let selected = self.selected().and_then(|old| targets.iter().position(|t| t == &old));
            let index = selected.or_else(|| (targets.len() == 1).then_some(0));
            self.populate(targets, index);
        }
    }

    fn populate(&mut self, targets: Vec<Target>, index: Option<usize>) {
        unsafe {
            gtk_combo_box_text_remove_all(self.combo);
            gtk_combo_box_text_append_text(self.combo, text::MD_TARGET.as_ptr());
            for t in &targets {
                let label = CString::new(t.label.replace('\0', "")).unwrap_or_default();
                gtk_combo_box_text_append_text(self.combo, label.as_ptr());
            }
            gtk_combo_box_set_active(self.combo, index.map_or(0, |i| (i + 1) as c_int));
        }
        self.targets = targets;
    }
}

struct DiffReviewPanel {
    pane: *mut GtkWidget,
    root: *mut GtkWidget,
    title_lbl: *mut GtkWidget,
    meta_lbl: *mut GtkWidget,
    mode_wt_btn: *mut GtkWidget,
    mode_staged_btn: *mut GtkWidget,
    mode_branch_btn: *mut GtkWidget,
    files_wrap: *mut GtkWidget,
    files_inner: *mut GtkWidget,
    hunk_lbl: *mut GtkWidget,
    prev_hunk_btn: *mut GtkWidget,
    next_hunk_btn: *mut GtkWidget,
    stage_btn: *mut GtkWidget,
    revert_btn: *mut GtkWidget,
    diff_buf: buffer::DiffBuffer,
    comment_entry: *mut GtkWidget,
    queue_badge: *mut GtkWidget,
    queue_wrap: *mut GtkWidget,
    queue_inner: *mut GtkWidget,
    clear_btn: *mut GtkWidget,
    send_btn: *mut GtkWidget,
    picker: AgentPicker,
    target: Option<DiffTarget>,
    mode: DiffMode,
    request_id: Option<u64>,
    files: Vec<DiffFile>,
    selected_file: usize,
    active_hunk: usize,
    rendered: RenderedDiff,
    reviewed_files: HashSet<String>,
    comments: Vec<ReviewComment>,
    scope_label: String,
    truncated: bool,
    last_theme: ThemeMode,
}

thread_local! {
    static PANEL: RefCell<Option<DiffReviewPanel>> = const { RefCell::new(None) };
}

fn on_gtk_idle<F: FnOnce() + Send + 'static>(callback: F) {
    extern "C" fn invoke<F: FnOnce()>(data: *mut c_void) -> c_int {
        let callback = unsafe { Box::from_raw(data.cast::<F>()) };
        callback();
        0
    }
    let data = Box::into_raw(Box::new(callback));
    unsafe { g_idle_add(invoke::<F>, data.cast()) };
}

fn current_theme() -> ThemeMode {
    app_state::with_app(|app| app.theme_mode).unwrap_or_default()
}

fn make_btn(
    parent: *mut GtkWidget,
    label: text::UiText,
    tip: Option<text::UiText>,
    cb: extern "C" fn(*mut GtkWidget, *mut c_void),
) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new_with_label(label.as_ptr());
        if let Some(t) = tip {
            set_tooltip(btn, t.as_str());
        }
        crate::connect_click(btn, cb, ptr::null_mut());
        gtk_box_pack_start(parent, btn, 0, 0, 0);
        btn
    }
}

fn connect_signal(widget: *mut GtkWidget, sig: &CStr, cb: *const c_void, data: *mut c_void) {
    unsafe { g_signal_connect_data(widget, sig.as_ptr(), cb, data, ptr::null(), 0) };
}

fn replace_inner_box(wrap: *mut GtkWidget, old_inner: &mut *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        if !old_inner.is_null() {
            gtk_widget_destroy(*old_inner);
        }
        let fresh = gtk_box_new(GTK_ORIENTATION_VERTICAL, 3);
        gtk_box_pack_start(wrap, fresh, 1, 1, 0);
        *old_inner = fresh;
        fresh
    }
}

fn build_header_bar(root: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        add_class(bar, "diff-review-header");
        let title_lbl = gtk_label_new(text::DIFF_OPEN_BTN.as_ptr());
        gtk_label_set_ellipsize(title_lbl, 3);
        gtk_label_set_xalign(title_lbl, 0.0);
        add_class(title_lbl, "diff-review-title");
        gtk_box_pack_start(bar, title_lbl, 1, 1, 0);
        make_btn(bar, text::DIFF_REFRESH, None, on_refresh_clicked);
        make_btn(bar, text::DIFF_CLOSE, None, on_close_clicked);
        gtk_box_pack_start(root, bar, 0, 1, 0);
        let meta_lbl = gtk_label_new(ptr::null());
        gtk_label_set_xalign(meta_lbl, 0.0);
        add_class(meta_lbl, "diff-review-meta");
        gtk_box_pack_start(root, meta_lbl, 0, 1, 0);
        (title_lbl, meta_lbl)
    }
}

fn build_mode_row(root: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        let wt = make_btn(row, text::DIFF_MODE_WORKTREE, None, on_mode_worktree);
        let staged = make_btn(row, text::DIFF_MODE_STAGED, None, on_mode_staged);
        let branch = make_btn(row, text::DIFF_MODE_BRANCH, None, on_mode_branch);
        gtk_box_pack_start(root, row, 0, 1, 0);
        (wt, staged, branch)
    }
}

fn build_scroll_box(root: *mut GtkWidget, css: &str, min_h: c_int, max_h: c_int) -> *mut GtkWidget {
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        add_class(scroll, css);
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_height(scroll, min_h);
        gtk_scrolled_window_set_max_content_height(scroll, max_h);
        let wrap = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_container_add(scroll, wrap);
        gtk_box_pack_start(root, scroll, 0, 1, 0);
        wrap
    }
}

fn build_hunk_toolbar(root: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        let hunk_lbl = gtk_label_new(ptr::null());
        gtk_label_set_xalign(hunk_lbl, 0.0);
        add_class(hunk_lbl, "diff-review-meta");
        gtk_box_pack_start(bar, hunk_lbl, 1, 1, 0);
        let prev = make_btn(bar, text::DIFF_HUNK_PREV, Some(text::DIFF_HUNK_PREV_TIP), on_prev_hunk);
        let next = make_btn(bar, text::DIFF_HUNK_NEXT, Some(text::DIFF_HUNK_NEXT_TIP), on_next_hunk);
        let stage = make_btn(bar, text::DIFF_STAGE_HUNK, Some(text::DIFF_HUNK_LOCAL_ONLY), on_stage_hunk);
        let revert = make_btn(bar, text::DIFF_REVERT_HUNK, Some(text::DIFF_HUNK_LOCAL_ONLY), on_revert_hunk);
        add_class(revert, "diff-review-danger");
        gtk_box_pack_start(root, bar, 0, 1, 0);
        (hunk_lbl, prev, next, stage, revert)
    }
}

struct ComposerWidgets {
    comment_entry: *mut GtkWidget,
    queue_badge: *mut GtkWidget,
    queue_wrap: *mut GtkWidget,
    clear_btn: *mut GtkWidget,
    send_btn: *mut GtkWidget,
    picker: AgentPicker,
}

fn build_comment_row(root: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        let entry = gtk_entry_new();
        gtk_entry_set_placeholder_text(entry, text::DIFF_COMMENT_PLACEHOLDER.as_ptr());
        connect_signal(entry, c"activate", on_add_comment as *const c_void, ptr::null_mut());
        gtk_box_pack_start(row, entry, 1, 1, 0);
        make_btn(row, text::DIFF_ADD_COMMENT, None, on_add_comment);
        gtk_box_pack_start(root, row, 0, 1, 0);
        entry
    }
}

fn build_queue_header(root: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        let badge = gtk_label_new(c"0".as_ptr());
        add_class(badge, "diff-review-badge");
        gtk_box_pack_start(row, badge, 0, 0, 0);
        gtk_box_pack_start(row, gtk_label_new(ptr::null()), 1, 1, 0);
        let clear_btn = make_btn(row, text::DIFF_CLEAR_BATCH, None, on_clear_batch);
        gtk_box_pack_start(root, row, 0, 1, 0);
        (badge, clear_btn)
    }
}

fn build_composer(root: *mut GtkWidget) -> ComposerWidgets {
    let comment_entry = build_comment_row(root);
    let (queue_badge, clear_btn) = build_queue_header(root);
    let queue_wrap = build_scroll_box(root, "diff-review-queue-scroll", 54, 110);
    unsafe {
        let send_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        let picker = AgentPicker::new();
        gtk_box_pack_start(send_row, picker.combo, 1, 1, 0);
        let send_btn = make_btn(send_row, text::DIFF_SEND_BATCH, None, on_send_batch);
        add_class(send_btn, "diff-review-primary");
        gtk_box_pack_start(root, send_row, 0, 1, 0);
        ComposerWidgets { comment_entry, queue_badge, queue_wrap, clear_btn, send_btn, picker }
    }
}

fn build_diff_scroll(root: *mut GtkWidget) -> buffer::DiffBuffer {
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_AUTOMATIC, GTK_POLICY_AUTOMATIC);
        let diff_buf = buffer::DiffBuffer::new();
        gtk_container_add(scroll, diff_buf.view);
        gtk_box_pack_start(root, scroll, 1, 1, 0);
        diff_buf
    }
}

fn mount_paned_shell(parent: *mut GtkWidget, workspace: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let pane = gtk_paned_new(GTK_ORIENTATION_HORIZONTAL);
        let root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(root, "diff-review-root");
        gtk_paned_pack1(pane, workspace, 1, 1);
        gtk_paned_pack2(pane, root, 0, 1);
        gtk_box_pack_start(parent, pane, 1, 1, 0);
        (pane, root)
    }
}

fn hide_root_until_opened(root: *mut GtkWidget) {
    unsafe {
        gtk_widget_show_all(root);
        gtk_widget_set_no_show_all(root, 1);
        gtk_widget_set_visible(root, 0);
    }
    connect_signal(root, c"destroy", on_destroyed as *const c_void, ptr::null_mut());
}

pub fn mount(parent: *mut GtkWidget, workspace: *mut GtkWidget) {
    let (pane, root) = mount_paned_shell(parent, workspace);
    let (title_lbl, meta_lbl) = build_header_bar(root);
    let (mode_wt_btn, mode_staged_btn, mode_branch_btn) = build_mode_row(root);
    let files_wrap = build_scroll_box(root, "diff-review-files-scroll", 90, 150);
    let (hunk_lbl, prev_hunk_btn, next_hunk_btn, stage_btn, revert_btn) = build_hunk_toolbar(root);
    let diff_buf = build_diff_scroll(root);
    let comp = build_composer(root);
    hide_root_until_opened(root);
    PANEL.replace(Some(DiffReviewPanel {
        pane, root, title_lbl, meta_lbl, mode_wt_btn, mode_staged_btn, mode_branch_btn,
        files_wrap, files_inner: ptr::null_mut(), hunk_lbl, prev_hunk_btn, next_hunk_btn,
        stage_btn, revert_btn, diff_buf, comment_entry: comp.comment_entry,
        queue_badge: comp.queue_badge, queue_wrap: comp.queue_wrap, queue_inner: ptr::null_mut(),
        clear_btn: comp.clear_btn, send_btn: comp.send_btn, picker: comp.picker,
        target: None, mode: DiffMode::WorkingTree, request_id: None, files: Vec::new(),
        selected_file: 0, active_hunk: 0, rendered: RenderedDiff::default(),
        reviewed_files: HashSet::new(), comments: Vec::new(),
        scope_label: String::new(), truncated: false, last_theme: current_theme(),
    }));
}

fn build_file_row(idx: usize, file: &DiffFile, selected: bool, reviewed: bool) -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        add_class(row, "diff-review-file-row");
        set_class(row, "active", selected);
        let chk = gtk_check_button_new();
        set_tooltip(chk, text::DIFF_REVIEWED.as_str());
        gtk_toggle_button_set_active(chk, i32::from(reviewed));
        connect_signal(chk, c"toggled", on_reviewed_toggled as *const c_void, idx as *mut c_void);
        gtk_box_pack_start(row, chk, 0, 0, 0);
        let btn = gtk_button_new_with_label(c"".as_ptr());
        add_class(btn, "diff-review-file-btn");
        if let Ok(lbl) = CString::new(file.display_path.as_str()) {
            gtk_button_set_label(btn, lbl.as_ptr());
        }
        set_tooltip(btn, &file.display_path);
        crate::connect_click(btn, on_file_clicked, idx as *mut c_void);
        gtk_box_pack_start(row, btn, 1, 1, 0);
        let ins = gtk_label_new(c"".as_ptr());
        set_label_text(ins, &format!("+{}", file.insertions));
        add_class(ins, "diff-review-ins");
        gtk_box_pack_start(row, ins, 0, 0, 0);
        let del = gtk_label_new(c"".as_ptr());
        set_label_text(del, &format!("-{}", file.deletions));
        add_class(del, "diff-review-del");
        gtk_box_pack_start(row, del, 0, 0, 0);
        row
    }
}

fn build_queue_row(idx: usize, item: &ReviewComment) -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        add_class(row, "diff-review-queue-row");
        let loc = parser::format_comment_location(item);
        let preview = format!("{loc} · {}", item.comment.trim());
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_ellipsize(lbl, 3);
        gtk_label_set_xalign(lbl, 0.0);
        set_label_text(lbl, &preview);
        set_tooltip(lbl, &preview);
        gtk_box_pack_start(row, lbl, 1, 1, 0);
        let rm = gtk_button_new_with_label(c"×".as_ptr());
        crate::connect_click(rm, on_remove_comment, idx as *mut c_void);
        gtk_box_pack_start(row, rm, 0, 0, 0);
        row
    }
}

impl DiffReviewPanel {
    fn show_panel(&self) {
        if let Some(target) = &self.target {
            let label = target.display_label();
            set_label_text(self.title_lbl, &label);
            set_tooltip(self.title_lbl, &label);
        }
        unsafe {
            if gtk_widget_get_visible(self.root) == 0 {
                let width = gtk_widget_get_allocated_width(self.pane).max(900);
                gtk_paned_set_position(self.pane, width * 42 / 100);
                gtk_widget_set_visible(self.root, 1);
            }
            gtk_widget_grab_focus(self.diff_buf.view);
        }
    }

    fn refresh_mode_buttons(&self) {
        set_class(self.mode_wt_btn, "diff-review-mode-active", self.mode == DiffMode::WorkingTree);
        set_class(self.mode_staged_btn, "diff-review-mode-active", self.mode == DiffMode::Staged);
        set_class(self.mode_branch_btn, "diff-review-mode-active", self.mode == DiffMode::Branch);
    }

    fn refresh_meta(&self) {
        let total = self.files.len();
        let reviewed = self.files.iter().filter(|f| self.reviewed_files.contains(&f.display_path)).count();
        let ins: usize = self.files.iter().map(|f| f.insertions).sum();
        let del: usize = self.files.iter().map(|f| f.deletions).sum();
        let suffix = if self.truncated { format!(" · {}", text::DIFF_TRUNCATED.as_str()) } else { String::new() };
        let summary = format!(
            "{}: {total} · {}: {reviewed}/{total} · +{ins} -{del}{suffix}",
            text::DIFF_FILES_HDR.as_str(),
            text::DIFF_REVIEWED.as_str(),
        );
        set_label_text(self.meta_lbl, &summary);
    }

    fn rebuild_file_list(&mut self) {
        let inner = replace_inner_box(self.files_wrap, &mut self.files_inner);
        for (idx, file) in self.files.iter().enumerate() {
            let reviewed = self.reviewed_files.contains(&file.display_path);
            let row = build_file_row(idx, file, idx == self.selected_file, reviewed);
            unsafe { gtk_box_pack_start(inner, row, 0, 1, 0) };
        }
        unsafe { gtk_widget_show_all(self.files_wrap) };
    }

    fn rebuild_queue_list(&mut self) {
        let count = self.comments.len();
        set_label_text(self.queue_badge, &format!("{count}"));
        unsafe {
            gtk_widget_set_sensitive(self.clear_btn, i32::from(count > 0));
            gtk_widget_set_sensitive(self.send_btn, i32::from(count > 0));
        }
        let inner = replace_inner_box(self.queue_wrap, &mut self.queue_inner);
        if self.comments.is_empty() {
            let empty = unsafe { gtk_label_new(text::DIFF_QUEUE_EMPTY.as_ptr()) };
            add_class(empty, "diff-review-meta");
            unsafe { gtk_box_pack_start(inner, empty, 0, 1, 2) };
        } else {
            for (idx, item) in self.comments.iter().enumerate() {
                let row = build_queue_row(idx, item);
                unsafe { gtk_box_pack_start(inner, row, 0, 1, 0) };
            }
        }
        unsafe { gtk_widget_show_all(self.queue_wrap) };
    }

    fn refresh_hunk_controls(&self) {
        let hunk_count = self.files.get(self.selected_file).map_or(0, |f| f.hunks.len());
        let is_binary = self.files.get(self.selected_file).is_some_and(|f| f.is_binary);
        let label = if hunk_count == 0 {
            "0/0".to_owned()
        } else {
            format!("{}/{}", self.active_hunk.min(hunk_count - 1) + 1, hunk_count)
        };
        set_label_text(self.hunk_lbl, &label);
        let allow_nav = i32::from(hunk_count > 1);
        let allow_mut = i32::from(
            self.target.as_ref().is_some_and(|t| t.allows_hunk_actions(self.mode))
                && hunk_count > 0
                && !is_binary,
        );
        unsafe {
            gtk_widget_set_sensitive(self.prev_hunk_btn, allow_nav);
            gtk_widget_set_sensitive(self.next_hunk_btn, allow_nav);
            gtk_widget_set_sensitive(self.stage_btn, allow_mut);
            gtk_widget_set_sensitive(self.revert_btn, allow_mut);
        }
    }
}

impl DiffReviewPanel {
    fn render_selected_file(&mut self) {
        let theme = current_theme();
        if let Some(file) = self.files.get(self.selected_file) {
            self.rendered = parser::render_file_diff(file);
            self.diff_buf.render(&self.rendered, theme);
            if self.active_hunk >= file.hunks.len() {
                self.active_hunk = 0;
            }
        } else {
            self.rendered = RenderedDiff::default();
            self.active_hunk = 0;
            self.diff_buf.set_message(text::DIFF_NO_CHANGES.as_str(), theme);
        }
        self.refresh_hunk_controls();
    }

    fn select_file(&mut self, idx: usize) {
        if idx >= self.files.len() {
            return;
        }
        self.selected_file = idx;
        self.active_hunk = 0;
        self.rebuild_file_list();
        self.render_selected_file();
    }

    fn step_hunk(&mut self, delta: isize) {
        let count = self.files.get(self.selected_file).map_or(0, |f| f.hunks.len());
        if count == 0 {
            return;
        }
        let next = (self.active_hunk as isize + delta).rem_euclid(count as isize) as usize;
        self.active_hunk = next;
        if let Some(offset) = parser::hunk_start_char(&self.rendered, next) {
            self.diff_buf.select(offset, offset, true);
        }
        self.refresh_hunk_controls();
    }

    fn apply_loaded_result(&mut self, result: DiffLoadResult) {
        let prev_path = self.files.get(self.selected_file).map(|f| f.display_path.clone());
        self.files = parser::parse_unified_diff(&result.raw_diff);
        self.scope_label = result.scope_label;
        self.truncated = result.truncated;
        self.selected_file = prev_path
            .and_then(|p| self.files.iter().position(|f| f.display_path == p))
            .unwrap_or(0);
        self.active_hunk = 0;
        self.refresh_mode_buttons();
        self.refresh_meta();
        self.rebuild_file_list();
        self.render_selected_file();
        self.rebuild_queue_list();
    }

    fn queue_current_comment(&mut self) -> Result<(), text::UiText> {
        let raw = unsafe { CStr::from_ptr(gtk_entry_get_text(self.comment_entry)) }
            .to_string_lossy()
            .trim()
            .to_owned();
        if raw.is_empty() {
            return Err(text::DIFF_NEED_COMMENT);
        }
        let file = self.files.get(self.selected_file).ok_or(text::DIFF_NO_CHANGES)?;
        let (start, end) = self.diff_buf.bounds();
        let selected = parser::extract_selection_excerpt(&self.rendered, start, end, self.active_hunk)
            .ok_or(text::DIFF_NO_CHANGES)?;
        let remote_host = self.target.as_ref().and_then(|t| t.remote_host()).map(str::to_owned);
        self.comments.push(ReviewComment {
            file_path: file.display_path.clone(),
            start_line: selected.start_line,
            end_line: selected.end_line,
            excerpt: selected.excerpt,
            comment: raw,
            remote_host,
        });
        unsafe { gtk_entry_set_text(self.comment_entry, c"".as_ptr()) };
        self.rebuild_queue_list();
        Ok(())
    }

    fn prepare_hunk_patch(&self) -> Result<(std::path::PathBuf, String), String> {
        let target = self.target.as_ref().ok_or_else(|| text::DIFF_HUNK_LOCAL_ONLY.as_str().to_owned())?;
        if !target.allows_hunk_actions(self.mode) {
            return Err(text::DIFF_HUNK_LOCAL_ONLY.as_str().to_owned());
        }
        let root = target.local_root().ok_or_else(|| text::DIFF_HUNK_LOCAL_ONLY.as_str().to_owned())?;
        let file = self.files.get(self.selected_file).ok_or_else(|| text::DIFF_NO_CHANGES.as_str().to_owned())?;
        let cursor = self.diff_buf.bounds().0;
        let hunk_idx = parser::active_hunk_at_offset(&self.rendered, cursor).unwrap_or(self.active_hunk);
        let patch = parser::reconstruct_hunk_patch(file, hunk_idx)
            .ok_or_else(|| text::DIFF_NO_CHANGES.as_str().to_owned())?;
        Ok((root.to_path_buf(), patch))
    }

    fn prepare_batch_send(&self) -> Result<(Target, String), text::UiText> {
        let prompt = parser::format_review_batch(&self.comments, Some(&self.scope_label))
            .ok_or(text::DIFF_QUEUE_EMPTY)?;
        let target = self.picker.selected().ok_or(text::MD_TARGET)?;
        Ok((target, prompt))
    }
}

fn start_load_current() {
    let job = PANEL.with_borrow_mut(|slot| {
        let p = slot.as_mut()?;
        let target = p.target.clone()?;
        let id = REQUEST_SEQ.fetch_add(1, Ordering::Relaxed);
        p.request_id = Some(id);
        p.refresh_mode_buttons();
        p.diff_buf.set_message(text::DIFF_LOADING.as_str(), current_theme());
        Some((id, target, p.mode))
    });
    let Some((id, target, mode)) = job else { return };
    std::thread::spawn(move || {
        let res = source::load_diff_for_target(&target, mode);
        on_gtk_idle(move || finish_load(id, res));
    });
}

fn finish_load(id: u64, result: std::io::Result<DiffLoadResult>) {
    PANEL.with_borrow_mut(|slot| {
        let Some(p) = slot.as_mut().filter(|p| p.request_id == Some(id)) else {
            return;
        };
        p.request_id = None;
        if unsafe { gtk_widget_get_visible(p.root) } == 0 {
            return;
        }
        match result {
            Ok(loaded) => p.apply_loaded_result(loaded),
            Err(err) => {
                p.files.clear();
                p.rendered = RenderedDiff::default();
                p.refresh_meta();
                p.rebuild_file_list();
                p.refresh_hunk_controls();
                p.diff_buf.set_message(&err.to_string(), current_theme());
                app_state::set_footer_message(&err.to_string());
            }
        }
    });
}

pub fn open_target(target: DiffTarget, mode: DiffMode) {
    PANEL.with_borrow_mut(|slot| {
        let Some(p) = slot.as_mut() else { return };
        if p.target.as_ref() != Some(&target) {
            p.reviewed_files.clear();
        }
        let origin = target.origin().cloned();
        p.target = Some(target);
        p.mode = mode;
        p.picker.reset(origin.as_ref());
        p.rebuild_queue_list();
        p.show_panel();
    });
    start_load_current();
}

pub fn open_for_active() {
    match source::resolve_active_target() {
        Ok(target) => open_target(target, DiffMode::WorkingTree),
        Err(msg) => app_state::set_footer_message(&msg),
    }
}

pub fn sync_targets() {
    PANEL.with_borrow_mut(|slot| {
        let Some(p) = slot.as_mut() else { return };
        if unsafe { gtk_widget_get_visible(p.root) } != 0 {
            p.picker.sync();
            let theme = current_theme();
            if p.last_theme != theme {
                p.last_theme = theme;
                p.diff_buf.apply_theme(theme);
            }
        }
    });
}

pub fn close_panel() {
    PANEL.with_borrow_mut(|slot| {
        if let Some(p) = slot.as_mut() {
            p.request_id = None;
            unsafe { gtk_widget_set_visible(p.root, 0) };
        }
    });
    app_state::focus_active_vte();
}

pub fn is_diff_review_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    let mods = state & (GDK_CONTROL_MASK | GDK_SHIFT_MASK | GDK_MOD1_MASK);
    if mods != (GDK_CONTROL_MASK | GDK_SHIFT_MASK) {
        return false;
    }
    matches!(char::from_u32(keyval), Some('B' | 'b' | 'И' | 'и')) || keycode == KEYCODE_B
}

pub fn route_key(win: *mut GtkWidget, key: u32, mods: u32) -> Option<c_int> {
    let focus = unsafe { gtk_window_get_focus(win) };
    let inside = PANEL.with_borrow(|slot| {
        let p = slot.as_ref()?;
        Some(!focus.is_null() && unsafe { gtk_widget_is_ancestor(focus, p.root) != 0 })
    });
    if inside != Some(true) {
        return None;
    }
    if key == 0xff1b {
        close_panel();
        return Some(1);
    }
    let mask = mods & (GDK_CONTROL_MASK | GDK_SHIFT_MASK | GDK_MOD1_MASK);
    if mask & GDK_MOD1_MASK != 0 || mask == GDK_CONTROL_MASK | GDK_SHIFT_MASK {
        return None;
    }
    if mask == GDK_CONTROL_MASK && matches!(char::from_u32(key), Some('k' | ',')) {
        return None;
    }
    Some(0)
}

pub fn pack_git_popover_button(vbox: *mut GtkWidget, popover: *mut GtkWidget) {
    unsafe {
        let btn = gtk_button_new_with_label(text::DIFF_OPEN_BTN.as_ptr());
        add_class(btn, "git-action-btn");
        set_tooltip(btn, text::DIFF_OPEN_TIP.as_str());
        crate::connect_click(btn, on_git_popover_open, popover.cast());
        gtk_box_pack_start(vbox, btn, 0, 1, 2);
    }
}

fn switch_mode(mode: DiffMode) {
    let changed = PANEL.with_borrow_mut(|slot| {
        let p = slot.as_mut()?;
        if p.mode == mode {
            return Some(false);
        }
        p.mode = mode;
        Some(true)
    });
    if changed == Some(true) {
        start_load_current();
    }
}

fn run_hunk_mutation(stage: bool) {
    let prep = PANEL.with_borrow(|slot| slot.as_ref().map(DiffReviewPanel::prepare_hunk_patch));
    let Some(Ok((root, patch))) = prep else {
        if let Some(Err(msg)) = prep {
            app_state::set_footer_message(&msg);
        }
        return;
    };
    let outcome = if stage { source::stage_hunk(&root, &patch) } else { source::revert_hunk(&root, &patch) };
    match outcome {
        Ok(()) => {
            let msg = if stage { text::DIFF_HUNK_STAGED } else { text::DIFF_HUNK_REVERTED };
            app_state::set_footer_message(msg.as_str());
            if let Some(root_str) = root.to_str() {
                app_state::with_app(|app| app.git_scheduler.request_refresh(root_str));
            }
            start_load_current();
        }
        Err(err) => app_state::set_footer_message(&err.to_string()),
    }
}

extern "C" fn on_destroyed(_: *mut GtkWidget, _: *mut c_void) {
    PANEL.replace(None);
}

extern "C" fn on_close_clicked(_: *mut GtkWidget, _: *mut c_void) {
    close_panel();
}

extern "C" fn on_refresh_clicked(_: *mut GtkWidget, _: *mut c_void) {
    start_load_current();
}

extern "C" fn on_mode_worktree(_: *mut GtkWidget, _: *mut c_void) {
    switch_mode(DiffMode::WorkingTree);
}

extern "C" fn on_mode_staged(_: *mut GtkWidget, _: *mut c_void) {
    switch_mode(DiffMode::Staged);
}

extern "C" fn on_mode_branch(_: *mut GtkWidget, _: *mut c_void) {
    switch_mode(DiffMode::Branch);
}

extern "C" fn on_file_clicked(_: *mut GtkWidget, data: *mut c_void) {
    let idx = data as usize;
    PANEL.with_borrow_mut(|slot| {
        if let Some(p) = slot.as_mut() {
            p.select_file(idx);
        }
    });
}

extern "C" fn on_reviewed_toggled(btn: *mut GtkWidget, data: *mut c_void) {
    let idx = data as usize;
    let active = unsafe { gtk_toggle_button_get_active(btn) != 0 };
    PANEL.with_borrow_mut(|slot| {
        let Some(p) = slot.as_mut() else { return };
        if let Some(file) = p.files.get(idx) {
            if active {
                p.reviewed_files.insert(file.display_path.clone());
            } else {
                p.reviewed_files.remove(&file.display_path);
            }
            p.refresh_meta();
        }
    });
}

extern "C" fn on_prev_hunk(_: *mut GtkWidget, _: *mut c_void) {
    PANEL.with_borrow_mut(|slot| {
        if let Some(p) = slot.as_mut() {
            p.step_hunk(-1);
        }
    });
}

extern "C" fn on_next_hunk(_: *mut GtkWidget, _: *mut c_void) {
    PANEL.with_borrow_mut(|slot| {
        if let Some(p) = slot.as_mut() {
            p.step_hunk(1);
        }
    });
}

extern "C" fn on_stage_hunk(_: *mut GtkWidget, _: *mut c_void) {
    run_hunk_mutation(true);
}

extern "C" fn on_revert_hunk(_: *mut GtkWidget, _: *mut c_void) {
    run_hunk_mutation(false);
}

extern "C" fn on_add_comment(_: *mut GtkWidget, _: *mut c_void) {
    let res = PANEL.with_borrow_mut(|slot| slot.as_mut().map(DiffReviewPanel::queue_current_comment));
    if let Some(Err(msg)) = res {
        app_state::set_footer_message(msg.as_str());
    }
}

extern "C" fn on_remove_comment(_: *mut GtkWidget, data: *mut c_void) {
    let idx = data as usize;
    PANEL.with_borrow_mut(|slot| {
        let Some(p) = slot.as_mut() else { return };
        if idx < p.comments.len() {
            p.comments.remove(idx);
            p.rebuild_queue_list();
        }
    });
}

extern "C" fn on_clear_batch(_: *mut GtkWidget, _: *mut c_void) {
    PANEL.with_borrow_mut(|slot| {
        if let Some(p) = slot.as_mut() {
            p.comments.clear();
            p.rebuild_queue_list();
        }
    });
}

extern "C" fn on_send_batch(_: *mut GtkWidget, _: *mut c_void) {
    let draft = PANEL.with_borrow(|slot| slot.as_ref().map(DiffReviewPanel::prepare_batch_send));
    match draft {
        Some(Ok((target, prompt))) => {
            if markdown::deliver(target, prompt) {
                PANEL.with_borrow_mut(|slot| {
                    if let Some(p) = slot.as_mut() {
                        p.comments.clear();
                        p.rebuild_queue_list();
                    }
                });
                app_state::set_footer_message(text::DIFF_SENT_OK.as_str());
            }
        }
        Some(Err(msg)) => app_state::set_footer_message(msg.as_str()),
        None => {}
    }
}

extern "C" fn on_git_popover_open(_: *mut GtkWidget, data: *mut c_void) {
    let pop = data as *mut GtkWidget;
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
    open_for_active();
}
