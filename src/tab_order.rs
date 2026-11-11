//! Tab reordering (keyboard + mouse drag) and pinned tab-title popover.

use std::cell::Cell;
use std::ffi::{c_int, c_uint, c_void, CStr, CString};
use std::ptr;
use std::rc::Rc;

use crate::ffi::*;
use crate::session::sanitize_pinned_title;
use crate::ui_text as text;

pub const TAB_STRIP_SPACING: i32 = 4;
pub const DRAG_THRESHOLD_PX: f64 = 6.0;

#[derive(Clone, Copy, Debug)]
struct TabDragState {
    sid: usize,
    start_strip_x: f64,
    dragging: bool,
}

thread_local! {
    static TAB_DRAG: Cell<Option<TabDragState>> = const { Cell::new(None) };
}

pub fn tab_reorder_delta(keyval: c_uint, state: c_uint, keycode: u16) -> Option<isize> {
    let mods = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
    if (state & mods) != mods || (state & GDK_MOD1_MASK) != 0 {
        return None;
    }
    if matches!(keyval, 0xff55 | 0xff9a) || keycode == 112 {
        Some(-1)
    } else if matches!(keyval, 0xff56 | 0xff9b) || keycode == 117 {
        Some(1)
    } else {
        None
    }
}

pub fn alt_tab_index(keyval: c_uint, state: c_uint) -> Option<usize> {
    if (state & GDK_MOD1_MASK) != 0 && (b'1' as u32..=b'9' as u32).contains(&keyval) {
        Some((keyval - b'1' as u32) as usize)
    } else {
        None
    }
}

pub fn step_tab_index(cur: usize, len: usize, delta: isize) -> Option<usize> {
    if len <= 1 || cur >= len || delta == 0 {
        return None;
    }
    let max_idx = (len - 1) as isize;
    let next = (cur as isize + delta).clamp(0, max_idx) as usize;
    (next != cur).then_some(next)
}

pub fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from >= items.len() || to >= items.len() || from == to {
        return false;
    }
    let moved = items.remove(from);
    items.insert(to, moved);
    true
}

pub fn drag_target_index(widths: &[i32], spacing: i32, cur_idx: usize, strip_x: f64) -> usize {
    if widths.len() <= 1 || cur_idx >= widths.len() {
        return 0;
    }
    let mut x = 0.0f64;
    let mut target = cur_idx;
    for (idx, &raw_w) in widths.iter().enumerate() {
        let w = f64::from(raw_w.max(24));
        let mid = x + w * 0.5;
        if idx < cur_idx && strip_x < mid && idx < target {
            target = idx;
        } else if idx > cur_idx && strip_x > mid {
            target = idx;
        }
        x += w + f64::from(spacing);
    }
    target
}

pub fn allocated_widths(widgets: &[*mut GtkWidget]) -> Vec<i32> {
    widgets
        .iter()
        .map(|&w| if w.is_null() { 0 } else { unsafe { gtk_widget_get_allocated_width(w) } })
        .collect()
}

pub fn begin_tab_drag(sid: usize, start_strip_x: f64) {
    TAB_DRAG.with(|slot| {
        slot.set(Some(TabDragState {
            sid,
            start_strip_x,
            dragging: false,
        }));
    });
}

pub fn is_tab_drag_pending() -> bool {
    TAB_DRAG.with(|slot| slot.get().is_some())
}

pub fn update_tab_drag(cur_strip_x: f64) -> Option<(usize, f64, bool)> {
    TAB_DRAG.with(|slot| {
        let mut st = slot.get()?;
        let dx = cur_strip_x - st.start_strip_x;
        let started_now = !st.dragging && dx.abs() >= DRAG_THRESHOLD_PX;
        if started_now {
            st.dragging = true;
            slot.set(Some(st));
        }
        st.dragging.then_some((st.sid, cur_strip_x, started_now))
    })
}

pub fn end_tab_drag() -> bool {
    TAB_DRAG.with(|slot| slot.take().is_some_and(|st| st.dragging))
}

#[derive(Clone, Copy)]
pub struct BoxReorder {
    pub container: *mut GtkWidget,
    pub child: *mut GtkWidget,
    pub position: c_int,
}

impl BoxReorder {
    pub fn apply(self) {
        if !self.container.is_null() && !self.child.is_null() {
            unsafe { gtk_box_reorder_child(self.container, self.child, self.position) };
        }
    }
}

pub struct TabReorderOp {
    pub strip: BoxReorder,
    pub sidebar: Vec<BoxReorder>,
}

impl TabReorderOp {
    pub fn apply(self) {
        self.strip.apply();
        for op in self.sidebar {
            op.apply();
        }
    }
}

struct PinPopoverCtx {
    default_anchor: *mut GtkWidget,
    popover: *mut GtkWidget,
    entry: *mut GtkWidget,
    target_sid: Cell<usize>,
    open: Cell<bool>,
    suppress_cancel: Cell<bool>,
    on_apply: Box<dyn Fn(usize, Option<String>)>,
    on_cancel: Box<dyn Fn()>,
}

#[derive(Clone)]
pub struct TabPinPopover {
    ctx: Rc<PinPopoverCtx>,
}

fn reanchor_popover(ctx: &PinPopoverCtx) {
    if !ctx.popover.is_null() && !ctx.default_anchor.is_null() {
        unsafe { gtk_popover_set_relative_to(ctx.popover, ctx.default_anchor) };
    }
}

fn read_entry_string(entry: *mut GtkWidget) -> String {
    unsafe {
        let ptr = gtk_entry_get_text(entry);
        if ptr.is_null() {
            String::new()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}

fn commit_pin_choice(ctx: &PinPopoverCtx, value: Option<String>) {
    let sid = ctx.target_sid.get();
    ctx.open.set(false);
    ctx.suppress_cancel.set(true);
    unsafe { gtk_popover_popdown(ctx.popover) };
    reanchor_popover(ctx);
    ctx.suppress_cancel.set(false);
    (ctx.on_apply)(sid, value);
}

extern "C" fn on_pin_save_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const PinPopoverCtx) };
    let raw = read_entry_string(ctx.entry);
    commit_pin_choice(ctx, sanitize_pinned_title(Some(&raw)));
}

extern "C" fn on_pin_reset_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const PinPopoverCtx) };
    commit_pin_choice(ctx, None);
}

extern "C" fn on_pin_entry_key(
    _entry: *mut GtkWidget,
    event: *const GdkEvent,
    data: *mut c_void,
) -> c_int {
    let mut keyval: c_uint = 0;
    if unsafe { gdk_event_get_keyval(event, &mut keyval) } == 0 {
        return 0;
    }
    let ctx = unsafe { &*(data as *const PinPopoverCtx) };
    match keyval {
        0xff0d | 0xff8d | 0xfe34 => {
            let raw = read_entry_string(ctx.entry);
            commit_pin_choice(ctx, sanitize_pinned_title(Some(&raw)));
            1
        }
        0xff1b => {
            ctx.open.set(false);
            ctx.suppress_cancel.set(true);
            unsafe { gtk_popover_popdown(ctx.popover) };
            reanchor_popover(ctx);
            ctx.suppress_cancel.set(false);
            (ctx.on_cancel)();
            1
        }
        _ => 0,
    }
}

extern "C" fn on_pin_popover_closed(_pop: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const PinPopoverCtx) };
    let restore = ctx.open.replace(false) && !ctx.suppress_cancel.get();
    ctx.suppress_cancel.set(false);
    reanchor_popover(ctx);
    if restore {
        (ctx.on_cancel)();
    }
}

fn create_pin_action_row() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let actions = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let save_btn = gtk_button_new_with_label(text::PIN_TAB_SAVE.as_ptr());
        let reset_btn = gtk_button_new_with_label(text::PIN_TAB_RESET.as_ptr());
        gtk_box_pack_start(actions, save_btn, 1, 1, 0);
        gtk_box_pack_start(actions, reset_btn, 1, 1, 0);
        (actions, save_btn, reset_btn)
    }
}

fn build_pin_popover_widgets(
    anchor: *mut GtkWidget,
) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let popover = gtk_popover_new(anchor);
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        add_class(vbox, "popover-box");
        let hdr = gtk_label_new(text::PIN_TAB_TITLE.as_ptr());
        gtk_label_set_xalign(hdr, 0.0);
        add_class(hdr, "section-header");
        let entry = gtk_entry_new();
        gtk_entry_set_placeholder_text(entry, text::PIN_TAB_PLACEHOLDER.as_ptr());
        add_class(entry, "search-entry");
        let (actions, save_btn, reset_btn) = create_pin_action_row();
        gtk_box_pack_start(vbox, hdr, 0, 1, 0);
        gtk_box_pack_start(vbox, entry, 0, 1, 0);
        gtk_box_pack_start(vbox, actions, 0, 1, 0);
        gtk_container_add(popover, vbox);
        gtk_widget_show_all(vbox);
        (popover, entry, save_btn, reset_btn)
    }
}

fn wire_pin_popover_signals(ctx: &Rc<PinPopoverCtx>, save_btn: *mut GtkWidget, reset_btn: *mut GtkWidget) {
    let ptr = Rc::as_ptr(ctx) as *mut c_void;
    unsafe {
        g_signal_connect_data(save_btn, c"clicked".as_ptr(), on_pin_save_clicked as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(reset_btn, c"clicked".as_ptr(), on_pin_reset_clicked as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(ctx.entry, c"key-press-event".as_ptr(), on_pin_entry_key as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(ctx.popover, c"closed".as_ptr(), on_pin_popover_closed as *const c_void, ptr, ptr::null(), 0);
    }
}

impl TabPinPopover {
    pub fn new(
        anchor: *mut GtkWidget,
        on_apply: impl Fn(usize, Option<String>) + 'static,
        on_cancel: impl Fn() + 'static,
    ) -> Self {
        let (popover, entry, save_btn, reset_btn) = build_pin_popover_widgets(anchor);
        let ctx = Rc::new(PinPopoverCtx {
            default_anchor: anchor,
            popover,
            entry,
            target_sid: Cell::new(0),
            open: Cell::new(false),
            suppress_cancel: Cell::new(false),
            on_apply: Box::new(on_apply),
            on_cancel: Box::new(on_cancel),
        });
        wire_pin_popover_signals(&ctx, save_btn, reset_btn);
        Self { ctx }
    }

    pub fn is_open(&self) -> bool {
        self.ctx.open.get()
    }

    pub fn focus_entry(&self) {
        if self.ctx.open.get() && !self.ctx.entry.is_null() {
            unsafe { gtk_widget_grab_focus(self.ctx.entry) };
        }
    }

    pub fn detach(&self) {
        let was_open = self.ctx.open.replace(false);
        self.ctx.suppress_cancel.set(true);
        if was_open && !self.ctx.popover.is_null() {
            unsafe { gtk_popover_popdown(self.ctx.popover) };
        }
        reanchor_popover(&self.ctx);
        self.ctx.suppress_cancel.set(false);
    }

    pub fn open(&self, anchor: *mut GtkWidget, sid: usize, current_text: &str) {
        if anchor.is_null() {
            return;
        }
        self.ctx.target_sid.set(sid);
        self.ctx.open.set(true);
        self.ctx.suppress_cancel.set(false);
        let text_c = CString::new(current_text).unwrap_or_default();
        unsafe {
            gtk_popover_set_relative_to(self.ctx.popover, anchor);
            gtk_entry_set_text(self.ctx.entry, text_c.as_ptr());
            gtk_popover_popup(self.ctx.popover);
            gtk_widget_grab_focus(self.ctx.entry);
        }
    }

    #[cfg(test)]
    pub fn submit_text(&self, text: &str) {
        commit_pin_choice(&self.ctx, sanitize_pinned_title(Some(text)));
    }

    #[cfg(test)]
    pub fn reset(&self) {
        commit_pin_choice(&self.ctx, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Instant;

    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::proc_radar::ProcInfo;
    use crate::session::{refresh_smart_title, SessionCategory, TermSession, WorkState};
    use crate::sidebar::Sidebar;
    use crate::split_pane::{wrap_vte_scroll, TabPanes};
    use crate::tab_strip::{TabHandlers, TabStrip};
    use crate::tabs::Tab;

    #[repr(C)]
    struct RawGList {
        data: *mut GtkWidget,
        next: *mut RawGList,
        prev: *mut RawGList,
    }

    extern "C" {
        fn gtk_container_get_children(container: *mut GtkContainer) -> *mut RawGList;
        fn g_list_free(list: *mut RawGList);
    }

    fn container_children(container: *mut GtkWidget) -> Vec<*mut GtkWidget> {
        let mut out = Vec::new();
        unsafe {
            let head = gtk_container_get_children(container);
            let mut cur = head;
            while !cur.is_null() {
                out.push((*cur).data);
                cur = (*cur).next;
            }
            if !head.is_null() {
                g_list_free(head);
            }
        }
        out
    }

    extern "C" fn noop_click(_w: *mut GtkWidget, _d: *mut c_void) {}
    extern "C" fn noop_event(_w: *mut GtkWidget, _e: *const GdkEvent, _d: *mut c_void) -> c_int {
        0
    }

    fn dummy_handlers() -> TabHandlers {
        TabHandlers {
            on_select: noop_click,
            on_close: noop_click,
            on_press: noop_event,
            on_motion: noop_event,
            on_release: noop_event,
        }
    }

    fn make_test_session(
        sid: usize,
        title: &str,
        cwd: &str,
        cat: SessionCategory,
        tab: Tab,
        strip: &TabStrip,
        sidebar: &Sidebar,
    ) -> TermSession {
        let vte = unsafe { vte_terminal_new() };
        let scroll_wrap = wrap_vte_scroll(vte);
        let panes = TabPanes::new(scroll_wrap, vte);
        let sidebar_row = sidebar.append_tab(cat, sid, noop_click, noop_event);
        let top_tab = strip.append(sid, cat, dummy_handlers());
        let now = Instant::now();
        let (base_title, profile_cwd, title, cwd) =
            (title.to_owned(), cwd.to_owned(), title.to_owned(), cwd.to_owned());
        TermSession {
            id: sid, base_title, profile_cwd, title, osc_title: String::new(), pinned_title: None,
            cwd, category: cat, vte, scroll_wrap, panes, sidebar_row, top_tab, child_pid: 0,
            state: WorkState::Idle, unread: false, last_output: now, work_started: now,
            work_started_at: None, last_turn_secs: None, last_notified: None,
            proc_info: ProcInfo::default(), tab, remote_tmux: None, tmux_title_from_vte: false,
            telemetry: crate::telemetry::SessionTelemetry::default(),
        }
    }

    fn sync_order_widgets(sessions: &[TermSession], strip: &TabStrip, sidebar: &Sidebar, to: usize) {
        TabReorderOp {
            strip: strip.reorder_op(&sessions[to].top_tab, to),
            sidebar: sidebar.build_reorder_ops(sessions),
        }
        .apply();
        for (idx, s) in sessions.iter().enumerate() {
            s.sidebar_row.update(idx, s, None, "", idx == to);
        }
        pump_gtk_events();
    }

    fn verify_pin_popover_flow(root: *mut GtkWidget, sessions: &mut Vec<TermSession>) {
        let applied: Rc<RefCell<Option<(usize, Option<String>)>>> = Rc::new(RefCell::new(None));
        let applied_cb = Rc::clone(&applied);
        let pin_pop = TabPinPopover::new(
            root,
            move |sid, val| *applied_cb.borrow_mut() = Some((sid, val)),
            || {},
        );
        pin_pop.open(sessions[2].top_tab.select_button(), sessions[2].id, "Локал 1");
        pin_pop.focus_entry();
        pump_gtk_events();
        pin_pop.submit_text("Закреплённый API");
        assert_eq!(*applied.borrow(), Some((sessions[2].id, Some("Закреплённый API".to_owned()))));

        sessions[2].set_pinned_title(Some("Закреплённый API"));
        sessions[2].proc_info.cwd = "/home/u/ДругаяПапка".to_owned();
        refresh_smart_title(&mut sessions[2]);
        assert_eq!(sessions[2].title, "Закреплённый API");

        pin_pop.reset();
        sessions[2].set_pinned_title(None);
        assert_eq!(sessions[2].title, "ДругаяПапка");

        let closed_after_popdown = sessions.pop().unwrap();
        closed_after_popdown.top_tab.destroy();
        pump_gtk_events();

        pin_pop.open(sessions[1].top_tab.select_button(), sessions[1].id, &sessions[1].title);
        pump_gtk_events();
        assert!(pin_pop.is_open());
        let closed_while_open = sessions.pop().unwrap();
        pin_pop.detach();
        assert!(!pin_pop.is_open());
        closed_while_open.top_tab.destroy();
        pump_gtk_events();

        pin_pop.open(sessions[0].top_tab.select_button(), sessions[0].id, &sessions[0].title);
        pump_gtk_events();
        assert_eq!(read_entry_string(pin_pop.ctx.entry), sessions[0].title);
        pin_pop.submit_text("После закрытия вкладки");
        assert_eq!(*applied.borrow(), Some((sessions[0].id, Some("После закрытия вкладки".to_owned()))));
    }

    #[test]
    fn matches_ctrl_shift_page_up_down_and_alt_digit_shortcuts() {
        let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
        assert_eq!(tab_reorder_delta(0xff55, cs, 0), Some(-1));
        assert_eq!(tab_reorder_delta(0xff9a, cs, 0), Some(-1));
        assert_eq!(tab_reorder_delta(0, cs, 112), Some(-1));
        assert_eq!(tab_reorder_delta(0xff56, cs, 0), Some(1));
        assert_eq!(tab_reorder_delta(0xff9b, cs, 0), Some(1));
        assert_eq!(tab_reorder_delta(0, cs, 117), Some(1));
        assert_eq!(tab_reorder_delta(0xff55, GDK_CONTROL_MASK, 112), None);
        assert_eq!(tab_reorder_delta(0xff55, cs | GDK_MOD1_MASK, 112), None);
        assert_eq!(alt_tab_index(b'1' as u32, GDK_MOD1_MASK), Some(0));
        assert_eq!(alt_tab_index(b'3' as u32, GDK_MOD1_MASK), Some(2));
        assert_eq!(alt_tab_index(b'0' as u32, GDK_MOD1_MASK), None);
    }

    #[test]
    fn computes_step_and_drag_indices_without_width_oscillation() {
        assert_eq!(step_tab_index(0, 3, -1), None);
        assert_eq!(step_tab_index(0, 3, 1), Some(1));
        assert_eq!(step_tab_index(2, 3, 1), None);
        assert_eq!(step_tab_index(2, 3, -1), Some(1));

        assert_eq!(drag_target_index(&[160, 80], 4, 0, 190.0), 0);
        assert_eq!(drag_target_index(&[160, 80], 4, 0, 210.0), 1);
        assert_eq!(drag_target_index(&[80, 160], 4, 1, 210.0), 1);
        assert_eq!(drag_target_index(&[80, 160], 4, 1, 35.0), 0);
    }

    #[test]
    fn tracks_rootless_tab_drag_threshold_and_latch() {
        begin_tab_drag(7, 40.0);
        assert!(is_tab_drag_pending() && update_tab_drag(43.0).is_none());
        assert_eq!(update_tab_drag(170.0), Some((7, 170.0, true)));
        assert_eq!(update_tab_drag(190.0), Some((7, 190.0, false)));
        assert_eq!(update_tab_drag(40.0), Some((7, 40.0, false)));
        assert!(end_tab_drag() && !is_tab_drag_pending() && !end_tab_drag());
    }

    #[test]
    fn headless_gtk_reorders_tabs_syncs_sidebar_and_keeps_pinned_title_across_cd() {
        with_headless_gtk(|| {
            let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
            let root = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 0) };
            unsafe { gtk_container_add(win, root) };
            let strip = TabStrip::new(root, noop_click);
            let sidebar = Sidebar::new(root, noop_click);
            let mut sessions = vec![
                make_test_session(1, "Локал 1", "/home/u/a", SessionCategory::Project, Tab::local("shell"), &strip, &sidebar),
                make_test_session(2, "ssh gpu", "/home/u", SessionCategory::Remote, Tab::remote("gpu", None), &strip, &sidebar),
                make_test_session(3, "Локал 2", "/home/u/b", SessionCategory::Project, Tab::local("shell"), &strip, &sidebar),
            ];
            for s in &sessions {
                unsafe { gtk_container_add(root, s.panes.root) };
            }
            unsafe { gtk_widget_show_all(win) };
            pump_gtk_events();

            assert!(move_item(&mut sessions, 0, 2));
            sync_order_widgets(&sessions, &strip, &sidebar, 2);
            let expected: Vec<*mut GtkWidget> = sessions.iter().map(|s| s.top_tab.tab_widget()).collect();
            assert_eq!(container_children(strip.tabs_box()), expected);
            let local_rows = container_children(sidebar.group_list_widget(SessionCategory::Project));
            assert_eq!(local_rows, vec![sessions[1].sidebar_row.row_widget(), sessions[2].sidebar_row.row_widget()]);
            assert_eq!(sessions[alt_tab_index(b'1' as u32, GDK_MOD1_MASK).unwrap()].id, 2);
            assert_eq!(sessions[alt_tab_index(b'3' as u32, GDK_MOD1_MASK).unwrap()].id, 1);

            verify_pin_popover_flow(root, &mut sessions);
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
        });
    }

    #[test]
    fn tab_order_module_maintains_grade_b_mi_score() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("tab_order.rs"));
        assert!(mi >= 58.0, "tab_order.rs MI {mi:.2} < 58.0");
    }
}
