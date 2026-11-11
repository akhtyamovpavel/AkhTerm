//! Split terminal panes inside a single local tab using GTK3 `GtkPaned`.

use std::ffi::{c_int, CString};
use std::ptr;

use crate::ffi::*;
use crate::proc_radar::foreground_cwd;

pub struct ExtraPane {
    pub scroll: *mut GtkWidget,
    pub vte: *mut GtkWidget,
    pub child_pid: i32,
    pub cwd: String,
}

pub struct SplitOp {
    pub target: *mut GtkWidget,
    pub added: *mut GtkWidget,
    pub root: *mut GtkWidget,
    pub orient: c_int,
}

impl SplitOp {
    pub fn apply(self) {
        insert_paned(self.target, self.added, self.root, self.orient);
    }
}

pub fn finish_split(op: Option<SplitOp>, new_scroll: *mut GtkWidget, new_vte: *mut GtkWidget) {
    if let Some(split_op) = op {
        split_op.apply();
        if !new_vte.is_null() {
            unsafe { gtk_widget_grab_focus(new_vte) };
        }
    } else if !new_scroll.is_null() {
        unsafe { gtk_widget_destroy(new_scroll) };
    }
}

pub fn apply_zoom_scale(vtes: &[*mut GtkWidget], scale: f64, zoom_btn: Option<*mut GtkWidget>) {
    unsafe {
        for &vte in vtes {
            vte_terminal_set_font_scale(vte, scale);
        }
        let pct = (scale * 100.0).round() as i32;
        if let (Some(btn), Ok(c)) = (zoom_btn, CString::new(format!("{pct}%"))) {
            gtk_button_set_label(btn, c.as_ptr());
        }
    }
}

pub struct CloseOp {
    pub closing_scroll: *mut GtkWidget,
    pub root: *mut GtkWidget,
    pub kill_pid: i32,
    pub next_focus: *mut GtkWidget,
}

impl CloseOp {
    pub fn apply(self) -> *mut GtkWidget {
        collapse_paned_child(self.closing_scroll, self.root);
        stop_pane_proc(self.kill_pid);
        self.next_focus
    }

    pub fn apply_focus(self, grab_focus: bool) {
        let vte = self.apply();
        if grab_focus && !vte.is_null() {
            unsafe { gtk_widget_grab_focus(vte) };
        }
    }
}

pub struct TabPanes {
    pub root: *mut GtkWidget,
    pub extra: Vec<ExtraPane>,
    pub last_focus: *mut GtkWidget,
}

pub fn wrap_vte_scroll(vte: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_container_add(scroll, vte);
        scroll
    }
}

pub fn is_widget_focused(widget: *mut GtkWidget) -> bool {
    !widget.is_null()
        && unsafe { gtk_widget_is_focus(widget) != 0 || gtk_widget_has_focus(widget) != 0 }
}

pub fn stop_pane_proc(pid: i32) {
    if pid <= 0 {
        return;
    }
    const SIGHUP: c_int = 1;
    unsafe {
        if kill(-pid, SIGHUP) != 0 {
            kill(pid, SIGHUP);
        }
    }
}

fn pack_into_parent(parent: *mut GtkWidget, child: *mut GtkWidget, root: *mut GtkWidget, first: bool) {
    unsafe {
        if parent == root {
            gtk_box_pack_start(root, child, 1, 1, 0);
        } else if first {
            gtk_paned_pack1(parent, child, 1, 1);
        } else {
            gtk_paned_pack2(parent, child, 1, 1);
        }
    }
}

fn half_extent(widget: *mut GtkWidget, orientation: c_int) -> c_int {
    let span = unsafe {
        if orientation == GTK_ORIENTATION_HORIZONTAL {
            gtk_widget_get_allocated_width(widget)
        } else {
            gtk_widget_get_allocated_height(widget)
        }
    };
    if span > 20 { span / 2 } else { 0 }
}

fn new_styled_paned(orientation: c_int, mid: c_int) -> *mut GtkWidget {
    unsafe {
        let paned = gtk_paned_new(orientation);
        gtk_paned_set_wide_handle(paned, 1);
        if mid > 0 {
            gtk_paned_set_position(paned, mid);
        }
        paned
    }
}

fn insert_paned(target: *mut GtkWidget, added: *mut GtkWidget, root: *mut GtkWidget, orient: c_int) {
    let mid = half_extent(target, orient);
    let paned = new_styled_paned(orient, mid);
    unsafe {
        let parent = gtk_widget_get_parent(target);
        let first = parent != root && gtk_paned_get_child1(parent) == target;
        g_object_ref(target);
        gtk_container_remove(parent, target);
        pack_into_parent(parent, paned, root, first);
        gtk_paned_pack1(paned, target, 1, 1);
        g_object_unref(target);
        gtk_paned_pack2(paned, added, 1, 1);
        gtk_widget_show_all(paned);
    }
}

fn paned_survivor(paned: *mut GtkWidget, closing: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let c1 = gtk_paned_get_child1(paned);
        if c1 == closing { gtk_paned_get_child2(paned) } else { c1 }
    }
}

fn collapse_paned_child(closing_scroll: *mut GtkWidget, root: *mut GtkWidget) {
    unsafe {
        let paned = gtk_widget_get_parent(closing_scroll);
        if paned.is_null() || paned == root {
            return;
        }
        let survivor = paned_survivor(paned, closing_scroll);
        let grand = gtk_widget_get_parent(paned);
        let first = grand != root && gtk_paned_get_child1(grand) == paned;
        g_object_ref(survivor);
        gtk_container_remove(paned, survivor);
        gtk_widget_destroy(paned);
        pack_into_parent(grand, survivor, root, first);
        g_object_unref(survivor);
        gtk_widget_show_all(survivor);
    }
}

impl TabPanes {
    pub fn new(primary_scroll: *mut GtkWidget, primary_vte: *mut GtkWidget) -> Self {
        let root = unsafe {
            let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
            gtk_box_pack_start(vbox, primary_scroll, 1, 1, 0);
            vbox
        };
        Self { root, extra: Vec::new(), last_focus: primary_vte }
    }

    pub fn has_splits(&self) -> bool {
        !self.extra.is_empty()
    }

    pub fn contains_vte(&self, vte: *mut GtkWidget) -> bool {
        self.extra.iter().any(|p| p.vte == vte)
    }

    pub fn record_focus(&mut self, vte: *mut GtkWidget) {
        self.last_focus = vte;
    }

    pub fn active_vte(&self, primary_vte: *mut GtkWidget) -> *mut GtkWidget {
        if is_widget_focused(primary_vte) {
            return primary_vte;
        }
        if let Some(pane) = self.extra.iter().find(|p| is_widget_focused(p.vte)) {
            return pane.vte;
        }
        if self.last_focus == primary_vte || self.contains_vte(self.last_focus) {
            return self.last_focus;
        }
        primary_vte
    }

    pub fn active_pid(&self, primary_vte: *mut GtkWidget, primary_pid: i32) -> i32 {
        let target = self.active_vte(primary_vte);
        self.extra.iter().find(|p| p.vte == target).map_or(primary_pid, |p| p.child_pid)
    }

    pub fn live_cwd_for_vte(
        &self,
        vte: *mut GtkWidget,
        primary_vte: *mut GtkWidget,
        primary_pid: i32,
        primary_cwd: &str,
    ) -> Option<String> {
        if vte == primary_vte {
            return foreground_cwd(primary_pid)
                .or_else(|| (!primary_cwd.is_empty()).then(|| primary_cwd.to_owned()));
        }
        let pane = self.extra.iter().find(|p| p.vte == vte)?;
        foreground_cwd(pane.child_pid).or_else(|| (!pane.cwd.is_empty()).then(|| pane.cwd.clone()))
    }

    pub fn active_cwd(&self, primary_vte: *mut GtkWidget, pid: i32, cwd: &str) -> String {
        let target = self.active_vte(primary_vte);
        self.live_cwd_for_vte(target, primary_vte, pid, cwd).unwrap_or_else(|| cwd.to_owned())
    }

    pub fn collect_vtes(&self, primary_vte: *mut GtkWidget, out: &mut Vec<*mut GtkWidget>) {
        out.push(primary_vte);
        for pane in &self.extra {
            out.push(pane.vte);
        }
    }

    #[cfg(test)]
    pub fn set_font_scale(&self, primary_vte: *mut GtkWidget, scale: f64) {
        unsafe {
            vte_terminal_set_font_scale(primary_vte, scale);
            for pane in &self.extra {
                vte_terminal_set_font_scale(pane.vte, scale);
            }
        }
    }

    pub fn stop_all_extra(&self) {
        for pane in &self.extra {
            stop_pane_proc(pane.child_pid);
        }
    }

    fn target_scroll(&self, primary_scroll: *mut GtkWidget, primary_vte: *mut GtkWidget) -> *mut GtkWidget {
        let target = self.active_vte(primary_vte);
        self.extra.iter().find(|p| p.vte == target).map_or(primary_scroll, |p| p.scroll)
    }

    pub fn prepare_split(
        &mut self,
        primary_scroll: *mut GtkWidget,
        primary_vte: *mut GtkWidget,
        new_scroll: *mut GtkWidget,
        new_vte: *mut GtkWidget,
        new_pid: i32,
        cwd: String,
        orient: c_int,
    ) -> SplitOp {
        let target = self.target_scroll(primary_scroll, primary_vte);
        self.last_focus = new_vte;
        self.extra.push(ExtraPane { scroll: new_scroll, vte: new_vte, child_pid: new_pid, cwd });
        SplitOp { target, added: new_scroll, root: self.root, orient }
    }

    #[cfg(test)]
    pub fn split(
        &mut self,
        primary_scroll: *mut GtkWidget,
        primary_vte: *mut GtkWidget,
        new_vte: *mut GtkWidget,
        new_pid: i32,
        cwd: String,
        orientation: c_int,
    ) {
        let new_scroll = wrap_vte_scroll(new_vte);
        self.prepare_split(primary_scroll, primary_vte, new_scroll, new_vte, new_pid, cwd, orientation)
            .apply();
    }

    fn promote_first_extra(
        &mut self,
        scroll: &mut *mut GtkWidget,
        vte: &mut *mut GtkWidget,
        pid: &mut i32,
        cwd: &mut String,
    ) -> CloseOp {
        let (closing_scroll, kill_pid) = (*scroll, *pid);
        let next = self.extra.remove(0);
        (*scroll, *vte, *pid, *cwd) = (next.scroll, next.vte, next.child_pid, next.cwd);
        self.last_focus = *vte;
        CloseOp { closing_scroll, root: self.root, kill_pid, next_focus: *vte }
    }

    fn pick_close_index(&self) -> usize {
        self.extra
            .iter()
            .position(|p| is_widget_focused(p.vte))
            .or_else(|| self.extra.iter().position(|p| p.vte == self.last_focus))
            .unwrap_or(self.extra.len() - 1)
    }

    fn remove_extra_at(&mut self, idx: usize, primary_vte: *mut GtkWidget, kill_proc: bool) -> CloseOp {
        let removed = self.extra.remove(idx);
        let kill_pid = if kill_proc { removed.child_pid } else { 0 };
        let next_focus = self.extra.last().map_or(primary_vte, |p| p.vte);
        self.last_focus = next_focus;
        CloseOp { closing_scroll: removed.scroll, root: self.root, kill_pid, next_focus }
    }

    pub fn prepare_close_active(
        &mut self,
        scroll: &mut *mut GtkWidget,
        vte: &mut *mut GtkWidget,
        pid: &mut i32,
        cwd: &mut String,
    ) -> Option<CloseOp> {
        if self.extra.is_empty() {
            return None;
        }
        if is_widget_focused(*vte) && !self.extra.iter().any(|p| is_widget_focused(p.vte)) {
            return Some(self.promote_first_extra(scroll, vte, pid, cwd));
        }
        let idx = self.pick_close_index();
        Some(self.remove_extra_at(idx, *vte, true))
    }

    #[cfg(test)]
    pub fn close_active(
        &mut self,
        scroll: &mut *mut GtkWidget,
        vte: &mut *mut GtkWidget,
        pid: &mut i32,
        cwd: &mut String,
    ) -> Option<*mut GtkWidget> {
        self.prepare_close_active(scroll, vte, pid, cwd).map(CloseOp::apply)
    }

    pub fn prepare_close_exited(
        &mut self,
        exited: *mut GtkWidget,
        scroll: &mut *mut GtkWidget,
        vte: &mut *mut GtkWidget,
        pid: &mut i32,
        cwd: &mut String,
    ) -> Option<CloseOp> {
        if self.extra.is_empty() {
            return None;
        }
        if exited == *vte {
            *pid = 0;
            return Some(self.promote_first_extra(scroll, vte, pid, cwd));
        }
        let idx = self.extra.iter().position(|p| p.vte == exited)?;
        Some(self.remove_extra_at(idx, *vte, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

    fn make_window_with_pane() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, TabPanes) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            gtk_window_set_default_size(win, 800, 600);
            let vte1 = vte_terminal_new();
            let scroll1 = wrap_vte_scroll(vte1);
            let panes = TabPanes::new(scroll1, vte1);
            gtk_container_add(win, panes.root);
            gtk_widget_show_all(win);
            gtk_widget_grab_focus(vte1);
            pump_gtk_events();
            (win, scroll1, vte1, panes)
        }
    }

    fn check_zoom_and_focus(panes: &mut TabPanes, vte1: *mut GtkWidget, vte2: *mut GtkWidget) {
        panes.set_font_scale(vte1, 1.3);
        unsafe {
            assert!((vte_terminal_get_font_scale(vte1) - 1.3).abs() < 1e-6);
            assert!((vte_terminal_get_font_scale(vte2) - 1.3).abs() < 1e-6);
            gtk_widget_grab_focus(vte2);
        }
        pump_gtk_events();
        assert_eq!(panes.active_vte(vte1), vte2);
        unsafe { gtk_widget_grab_focus(vte1) };
        pump_gtk_events();
        assert_eq!(panes.active_vte(vte1), vte1);
    }

    fn check_primary_promotion(
        panes: &mut TabPanes,
        scroll1: &mut *mut GtkWidget,
        vte1: &mut *mut GtkWidget,
        pid1: &mut i32,
        cwd1: &mut String,
    ) {
        let vte3 = unsafe { vte_terminal_new() };
        panes.split(*scroll1, *vte1, vte3, 0, "/tmp/второй".to_owned(), GTK_ORIENTATION_VERTICAL);
        unsafe { gtk_widget_grab_focus(*vte1) };
        pump_gtk_events();
        let promoted = panes.close_active(scroll1, vte1, pid1, cwd1);
        assert_eq!(promoted, Some(vte3));
        assert_eq!(*vte1, vte3);
        assert_eq!(cwd1, "/tmp/второй");
        assert!(!panes.has_splits());
    }

    #[test]
    fn split_pane_mi_meets_grade_b_target() {
        let (_, _, split_mi) = crate::mi::compute_file_mi(include_str!("split_pane.rs"));
        assert!(split_mi >= 58.0, "split_pane.rs MI {split_mi:.2} < 58.0");
        let (_, _, main_mi) = crate::mi::compute_file_mi(include_str!("main.rs"));
        assert!(main_mi >= 54.78, "main.rs MI {main_mi:.2} < 54.78");
        let (_, _, session_mi) = crate::mi::compute_file_mi(include_str!("session.rs"));
        assert!(session_mi >= 55.69, "session.rs MI {session_mi:.2} < 55.69");
        let (_, _, radar_mi) = crate::mi::compute_file_mi(include_str!("proc_radar.rs"));
        assert!(radar_mi >= 55.92, "proc_radar.rs MI {radar_mi:.2} < 55.92");
        let (_, _, ffi_mi) = crate::mi::compute_file_mi(include_str!("ffi.rs"));
        assert!(ffi_mi >= 64.76, "ffi.rs MI {ffi_mi:.2} < 64.76");
    }

    #[test]
    fn creates_and_closes_split_panes_in_headless_gtk() {
        with_headless_gtk(|| {
            let (win, mut scroll1, mut vte1, mut panes) = make_window_with_pane();
            let (mut pid1, mut cwd1) = (0, "/tmp/проект".to_owned());
            let vte2 = unsafe { vte_terminal_new() };
            panes.split(scroll1, vte1, vte2, 0, "/tmp/проект/подпапка".to_owned(), GTK_ORIENTATION_HORIZONTAL);
            pump_gtk_events();
            assert!(panes.has_splits());
            check_zoom_and_focus(&mut panes, vte1, vte2);
            unsafe { gtk_widget_grab_focus(vte2) };
            pump_gtk_events();
            assert_eq!(panes.close_active(&mut scroll1, &mut vte1, &mut pid1, &mut cwd1), Some(vte1));
            assert!(panes.close_active(&mut scroll1, &mut vte1, &mut pid1, &mut cwd1).is_none());
            check_primary_promotion(&mut panes, &mut scroll1, &mut vte1, &mut pid1, &mut cwd1);
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
        });
    }
}
