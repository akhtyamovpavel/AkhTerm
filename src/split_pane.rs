//! Split terminal panes inside a single local or remote tab using GTK3 `GtkPaned`.

use std::cell::Cell;
use std::ffi::{c_int, c_void, CString};
use std::path::Path;
use std::ptr;

use crate::ffi::*;
use crate::pane_header::PaneHeader;
use crate::proc_radar::foreground_cwd;
use crate::tabs::{
    clamp_split_ratio, RemoteConnection, SplitOrientation, SplitTree, Tab, DEFAULT_SPLIT_RATIO,
};

extern "C" {
    fn gtk_paned_get_position(paned: *mut GtkPaned) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PaneRect {
    fn cx(self) -> i32 {
        self.x + self.w / 2
    }

    fn cy(self) -> i32 {
        self.y + self.h / 2
    }
}

pub struct ExtraPane {
    pub scroll: *mut GtkWidget,
    pub vte: *mut GtkWidget,
    pub child_pid: i32,
    pub cwd: String,
}

pub struct SpawnedPane {
    pub scroll: *mut GtkWidget,
    pub vte: *mut GtkWidget,
    pub child_pid: i32,
    pub cwd: String,
    pub remote: Option<RemoteConnection>,
}

#[derive(Clone)]
enum LayoutNode {
    Leaf(*mut GtkWidget),
    Split {
        orient: SplitOrientation,
        ratio_permille: u16,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    fn replace_leaf(&mut self, target: *mut GtkWidget, added: *mut GtkWidget, orient: SplitOrientation) -> bool {
        match self {
            Self::Leaf(cur) if *cur == target => {
                *self = Self::Split {
                    orient,
                    ratio_permille: DEFAULT_SPLIT_RATIO,
                    first: Box::new(Self::Leaf(target)),
                    second: Box::new(Self::Leaf(added)),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.replace_leaf(target, added, orient) || second.replace_leaf(target, added, orient)
            }
        }
    }

    fn remove_leaf(&self, closing: *mut GtkWidget) -> (Option<Self>, bool) {
        match self {
            Self::Leaf(cur) if *cur == closing => (None, true),
            Self::Leaf(_) => (Some(self.clone()), false),
            Self::Split { orient, ratio_permille, first, second } => {
                let (f_next, f_hit) = first.remove_leaf(closing);
                if f_hit {
                    let node = f_next.map_or_else(
                        || (**second).clone(),
                        |nf| Self::Split { orient: *orient, ratio_permille: *ratio_permille, first: Box::new(nf), second: second.clone() },
                    );
                    return (Some(node), true);
                }
                let (s_next, s_hit) = second.remove_leaf(closing);
                if s_hit {
                    let node = s_next.map_or_else(
                        || (**first).clone(),
                        |ns| Self::Split { orient: *orient, ratio_permille: *ratio_permille, first: first.clone(), second: Box::new(ns) },
                    );
                    return (Some(node), true);
                }
                (Some(self.clone()), false)
            }
        }
    }

    fn from_tree(
        tree: &SplitTree,
        primary: *mut GtkWidget,
        scrolls: &mut impl Iterator<Item = *mut GtkWidget>,
    ) -> Option<Self> {
        match tree {
            SplitTree::Primary => Some(Self::Leaf(primary)),
            SplitTree::Pane(_) | SplitTree::RemotePane(_) => Some(Self::Leaf(scrolls.next()?)),
            SplitTree::Split { orientation, ratio_permille, first, second } => Some(Self::Split {
                orient: *orientation,
                ratio_permille: clamp_split_ratio(*ratio_permille),
                first: Box::new(Self::from_tree(first, primary, scrolls)?),
                second: Box::new(Self::from_tree(second, primary, scrolls)?),
            }),
        }
    }

    fn any_leaf(&self) -> *mut GtkWidget {
        match self {
            Self::Leaf(w) => *w,
            Self::Split { first, .. } => first.any_leaf(),
        }
    }

    fn contains_leaf(&self, target: *mut GtkWidget) -> bool {
        match self {
            Self::Leaf(w) => *w == target,
            Self::Split { first, second, .. } => first.contains_leaf(target) || second.contains_leaf(target),
        }
    }

    fn edge_leaf(&self, take_second: bool) -> *mut GtkWidget {
        match self {
            Self::Leaf(w) => *w,
            Self::Split { first, second, .. } => {
                if take_second { second.edge_leaf(true) } else { first.edge_leaf(false) }
            }
        }
    }

    fn neighbor_leaf(&self, target: *mut GtkWidget, dir: FocusDir) -> Option<*mut GtkWidget> {
        let Self::Split { orient, first, second, .. } = self else {
            return None;
        };
        let in_first = first.contains_leaf(target);
        let child = if in_first { first } else { second };
        if let Some(deeper) = child.neighbor_leaf(target, dir) {
            return Some(deeper);
        }
        let matches_axis = matches!(
            (*orient, dir, in_first),
            (SplitOrientation::Horizontal, FocusDir::Right, true)
                | (SplitOrientation::Horizontal, FocusDir::Left, false)
                | (SplitOrientation::Vertical, FocusDir::Down, true)
                | (SplitOrientation::Vertical, FocusDir::Up, false)
        );
        matches_axis.then(|| if in_first { second.edge_leaf(false) } else { first.edge_leaf(true) })
    }

    fn to_split_tree(
        &self,
        primary_scroll: *mut GtkWidget,
        extra: &[ExtraPane],
        remotes: &[(*mut GtkWidget, RemoteConnection)],
        tab: &Tab,
    ) -> SplitTree {
        match self {
            Self::Leaf(w) if *w == primary_scroll => SplitTree::Primary,
            Self::Leaf(w) => leaf_to_split(*w, extra, remotes, tab),
            Self::Split { orient, ratio_permille, first, second } => SplitTree::split_with_ratio(
                *orient,
                *ratio_permille,
                first.to_split_tree(primary_scroll, extra, remotes, tab),
                second.to_split_tree(primary_scroll, extra, remotes, tab),
            ),
        }
    }
}

fn leaf_to_split(
    scroll: *mut GtkWidget,
    extra: &[ExtraPane],
    remotes: &[(*mut GtkWidget, RemoteConnection)],
    tab: &Tab,
) -> SplitTree {
    if let Tab::Remote(default_conn) = tab {
        let conn = remotes
            .iter()
            .find(|(s, _)| *s == scroll)
            .map(|(_, c)| c.clone())
            .unwrap_or_else(|| default_conn.clone());
        return SplitTree::RemotePane(conn);
    }
    let cwd = extra.iter().find(|p| p.scroll == scroll).map_or_else(String::new, |p| p.cwd.clone());
    SplitTree::Pane(cwd)
}

pub fn resolve_pane_cwd(saved: &str, fallback: &str) -> String {
    let p = Path::new(saved);
    if p.is_absolute() && p.is_dir() {
        saved.to_owned()
    } else {
        fallback.to_owned()
    }
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

#[repr(C)]
struct GtkAllocation {
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
}

struct RatioApplyState {
    orient: c_int,
    ratio_permille: u16,
    done: Cell<bool>,
}

fn paned_span(paned: *mut GtkWidget, orient: c_int) -> c_int {
    unsafe {
        if orient == GTK_ORIENTATION_HORIZONTAL {
            gtk_widget_get_allocated_width(paned)
        } else {
            gtk_widget_get_allocated_height(paned)
        }
    }
}

fn ratio_to_pos(span: c_int, ratio_permille: u16) -> c_int {
    let clamped = i64::from(clamp_split_ratio(ratio_permille));
    ((i64::from(span) * clamped + 500) / 1000).clamp(1, i64::from(span - 1)) as c_int
}

fn apply_paned_ratio_if_ready(paned: *mut GtkWidget, orient: c_int, ratio_permille: u16) -> bool {
    let span = paned_span(paned, orient);
    if span <= 20 {
        return false;
    }
    unsafe { gtk_paned_set_position(paned as *mut GtkPaned, ratio_to_pos(span, ratio_permille)) };
    true
}

extern "C" fn on_paned_size_allocate(
    paned: *mut GtkWidget,
    alloc: *const GtkAllocation,
    data: *mut c_void,
) {
    if data.is_null() || alloc.is_null() {
        return;
    }
    let state = unsafe { &*(data as *const RatioApplyState) };
    if state.done.get() {
        return;
    }
    let rect = unsafe { &*alloc };
    let span = if state.orient == GTK_ORIENTATION_HORIZONTAL { rect.width } else { rect.height };
    if span > 20 {
        unsafe {
            gtk_paned_set_position(paned as *mut GtkPaned, ratio_to_pos(span, state.ratio_permille));
        }
        state.done.set(true);
    }
}

extern "C" fn drop_ratio_state(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data as *mut RatioApplyState)) };
    }
}

fn connect_ratio_applier(paned: *mut GtkWidget, orient: c_int, ratio_permille: u16) {
    let ready = apply_paned_ratio_if_ready(paned, orient, ratio_permille);
    let raw = Box::into_raw(Box::new(RatioApplyState {
        orient,
        ratio_permille: clamp_split_ratio(ratio_permille),
        done: Cell::new(ready),
    }));
    unsafe {
        g_signal_connect_data(
            paned as *mut c_void,
            c"size-allocate".as_ptr(),
            on_paned_size_allocate as *const c_void,
            raw.cast(),
            drop_ratio_state as *const c_void,
            0,
        );
    }
}

pub struct RestoreSplitsOp {
    root: *mut GtkWidget,
    primary_widget: *mut GtkWidget,
    tree: SplitTree,
    pane_widgets: Vec<*mut GtkWidget>,
}

impl RestoreSplitsOp {
    pub fn apply(self) {
        unsafe {
            g_object_ref(self.primary_widget);
            gtk_container_remove(self.root, self.primary_widget);
            let mut iter = self.pane_widgets.into_iter();
            let top = build_gtk_split_tree(&self.tree, self.primary_widget, &mut iter);
            gtk_box_pack_start(self.root, top, 1, 1, 0);
            g_object_unref(self.primary_widget);
            gtk_widget_show_all(self.root);
        }
    }
}

pub fn finish_restore_splits(op: Option<RestoreSplitsOp>, scrolls: &[*mut GtkWidget]) {
    if let Some(restore_op) = op {
        restore_op.apply();
    } else {
        for &s in scrolls.iter().filter(|w| !w.is_null()) {
            unsafe { gtk_widget_destroy(s) };
        }
    }
}

fn build_gtk_split_tree(
    tree: &SplitTree,
    primary_widget: *mut GtkWidget,
    pane_widgets: &mut impl Iterator<Item = *mut GtkWidget>,
) -> *mut GtkWidget {
    match tree {
        SplitTree::Primary => primary_widget,
        SplitTree::Pane(_) | SplitTree::RemotePane(_) => pane_widgets.next().unwrap_or(primary_widget),
        SplitTree::Split { orientation, ratio_permille, first, second } => {
            let c1 = build_gtk_split_tree(first, primary_widget, pane_widgets);
            let c2 = build_gtk_split_tree(second, primary_widget, pane_widgets);
            let paned = new_styled_paned(orientation.to_gtk(), 0);
            unsafe {
                gtk_paned_pack1(paned, c1, 1, 1);
                gtk_paned_pack2(paned, c2, 1, 1);
            }
            connect_ratio_applier(paned, orientation.to_gtk(), *ratio_permille);
            paned
        }
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

struct ZoomState {
    zoomed_scroll: *mut GtkWidget,
    tree_root: *mut GtkWidget,
    parent_paned: *mut GtkWidget,
    was_first: bool,
}

pub struct TabPanes {
    pub root: *mut GtkWidget,
    pub extra: Vec<ExtraPane>,
    pub last_focus: *mut GtkWidget,
    layout: LayoutNode,
    primary_header: PaneHeader,
    extra_headers: Vec<PaneHeader>,
    remote_conns: Vec<(*mut GtkWidget, RemoteConnection)>,
    remote_seq: usize,
    zoom: Option<ZoomState>,
}

impl Drop for TabPanes {
    fn drop(&mut self) {
        if let Some(zoom) = self.zoom.take() {
            unsafe {
                gtk_widget_destroy(zoom.tree_root);
                g_object_unref(zoom.tree_root);
            }
        }
    }
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

fn collect_ancestors(mut widget: *mut GtkWidget) -> Vec<*mut GtkWidget> {
    let mut chain = Vec::new();
    while !widget.is_null() {
        chain.push(widget);
        widget = unsafe { gtk_widget_get_parent(widget) };
    }
    chain
}

fn find_split_paned(w1: *mut GtkWidget, w2: *mut GtkWidget) -> *mut GtkWidget {
    let c1 = collect_ancestors(w1);
    let c2 = collect_ancestors(w2);
    c1.into_iter().find(|a| c2.contains(a)).unwrap_or(ptr::null_mut())
}

fn read_paned_ratio(paned: *mut GtkWidget, orient: SplitOrientation, fallback: u16) -> u16 {
    if paned.is_null() {
        return clamp_split_ratio(fallback);
    }
    let span = paned_span(paned, orient.to_gtk());
    let pos = unsafe { gtk_paned_get_position(paned as *mut GtkPaned) };
    if span <= 20 || pos <= 0 {
        return clamp_split_ratio(fallback);
    }
    let ratio = ((i64::from(pos) * 1000 + i64::from(span) / 2) / i64::from(span)) as u16;
    clamp_split_ratio(ratio)
}

fn find_top_child(mut leaf: *mut GtkWidget, root: *mut GtkWidget) -> *mut GtkWidget {
    while !leaf.is_null() {
        let parent = unsafe { gtk_widget_get_parent(leaf) };
        if parent == root {
            return leaf;
        }
        leaf = parent;
    }
    ptr::null_mut()
}

fn range_gap(a_start: i32, a_end: i32, b_start: i32, b_end: i32) -> i32 {
    if b_end < a_start {
        a_start - b_end
    } else if a_end < b_start {
        b_start - a_end
    } else {
        0
    }
}

fn axis_metrics(src: PaneRect, dst: PaneRect, dir: FocusDir) -> Option<(i32, i32, i32)> {
    match dir {
        FocusDir::Left if dst.cx() < src.cx() && dst.x < src.x => {
            Some((src.x - (dst.x + dst.w), range_gap(src.y, src.y + src.h, dst.y, dst.y + dst.h), (dst.cy() - src.cy()).abs()))
        }
        FocusDir::Right if dst.cx() > src.cx() && dst.x + dst.w > src.x + src.w => {
            Some((dst.x - (src.x + src.w), range_gap(src.y, src.y + src.h, dst.y, dst.y + dst.h), (dst.cy() - src.cy()).abs()))
        }
        FocusDir::Up if dst.cy() < src.cy() && dst.y < src.y => {
            Some((src.y - (dst.y + dst.h), range_gap(src.x, src.x + src.w, dst.x, dst.x + dst.w), (dst.cx() - src.cx()).abs()))
        }
        FocusDir::Down if dst.cy() > src.cy() && dst.y + dst.h > src.y + src.h => {
            Some((dst.y - (src.y + src.h), range_gap(src.x, src.x + src.w, dst.x, dst.x + dst.w), (dst.cx() - src.cx()).abs()))
        }
        _ => None,
    }
}

pub fn pick_neighbor_rect(
    src: PaneRect,
    candidates: &[(usize, PaneRect)],
    dir: FocusDir,
) -> Option<usize> {
    candidates
        .iter()
        .filter_map(|&(idx, rect)| {
            let (primary, cross, center) = axis_metrics(src, rect, dir)?;
            let key = (primary.max(0), cross, center / 16, rect.y, rect.x);
            Some((key, idx))
        })
        .min_by_key(|&(key, _)| key)
        .map(|(_, idx)| idx)
}

fn pane_rect_in_root(widget: *mut GtkWidget, root: *mut GtkWidget) -> Option<PaneRect> {
    if widget.is_null() || root.is_null() {
        return None;
    }
    unsafe {
        let (mut x, mut y) = (0, 0);
        if gtk_widget_translate_coordinates(widget, root, 0, 0, &mut x, &mut y) == 0 {
            return None;
        }
        let w = gtk_widget_get_allocated_width(widget);
        let h = gtk_widget_get_allocated_height(widget);
        (w > 1 && h > 1).then_some(PaneRect { x, y, w, h })
    }
}

impl TabPanes {
    pub fn new(primary_scroll: *mut GtkWidget, primary_vte: *mut GtkWidget) -> Self {
        let primary_header = PaneHeader::wrap(primary_scroll, primary_vte);
        let root = unsafe {
            let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
            gtk_box_pack_start(vbox, primary_header.container, 1, 1, 0);
            vbox
        };
        Self {
            root,
            extra: Vec::new(),
            last_focus: primary_vte,
            layout: LayoutNode::Leaf(primary_scroll),
            primary_header,
            extra_headers: Vec::new(),
            remote_conns: Vec::new(),
            remote_seq: 0,
            zoom: None,
        }
    }

    #[cfg(test)]
    pub fn empty_for_test() -> Self {
        Self {
            root: ptr::null_mut(),
            extra: Vec::new(),
            last_focus: ptr::null_mut(),
            layout: LayoutNode::Leaf(ptr::null_mut()),
            primary_header: PaneHeader::empty_for_test(),
            extra_headers: Vec::new(),
            remote_conns: Vec::new(),
            remote_seq: 0,
            zoom: None,
        }
    }

    pub fn has_splits(&self) -> bool {
        !self.extra.is_empty()
    }

    pub fn is_zoomed(&self) -> bool {
        self.zoom.is_some()
    }

    pub fn header_visible(&self) -> bool {
        !self.extra.is_empty() || self.zoom.is_some()
    }

    pub fn zoomed_vte(&self, primary_scroll: *mut GtkWidget, primary_vte: *mut GtkWidget) -> Option<*mut GtkWidget> {
        let zoom = self.zoom.as_ref()?;
        if zoom.zoomed_scroll == primary_scroll {
            return Some(primary_vte);
        }
        self.extra.iter().find(|p| p.scroll == zoom.zoomed_scroll).map(|p| p.vte)
    }

    pub fn all_headers(&self) -> Vec<PaneHeader> {
        let mut out = Vec::with_capacity(1 + self.extra_headers.len());
        out.push(self.primary_header);
        out.extend_from_slice(&self.extra_headers);
        out
    }

    pub fn sync_header_visibility(&self) {
        let visible = self.header_visible();
        self.primary_header.set_visible(visible);
        for header in &self.extra_headers {
            header.set_visible(visible);
        }
    }

    pub fn next_remote_seq(&mut self) -> usize {
        self.remote_seq += 1;
        self.remote_seq
    }

    pub fn record_remote_pane(&mut self, scroll: *mut GtkWidget, conn: RemoteConnection) {
        self.remote_conns.retain(|(s, _)| *s != scroll);
        self.remote_conns.push((scroll, conn));
    }

    pub fn remote_for_vte(&self, vte: *mut GtkWidget, primary_scroll: *mut GtkWidget, primary_vte: *mut GtkWidget) -> Option<&RemoteConnection> {
        let scroll = if vte == primary_vte {
            primary_scroll
        } else {
            self.extra.iter().find(|p| p.vte == vte).map_or(ptr::null_mut(), |p| p.scroll)
        };
        self.remote_conns.iter().find(|(s, _)| *s == scroll).map(|(_, c)| c)
    }

    fn pane_container(&self, scroll: *mut GtkWidget, primary_scroll: *mut GtkWidget) -> *mut GtkWidget {
        let container = if scroll == primary_scroll {
            self.primary_header.container
        } else {
            self.extra
                .iter()
                .position(|p| p.scroll == scroll)
                .and_then(|idx| self.extra_headers.get(idx))
                .map_or(scroll, |h| h.container)
        };
        if container.is_null() { scroll } else { container }
    }

    fn sync_node_ratios(&self, node: &mut LayoutNode, primary_scroll: *mut GtkWidget) {
        let LayoutNode::Split { orient, ratio_permille, first, second } = node else {
            return;
        };
        let w1 = self.pane_container(first.any_leaf(), primary_scroll);
        let w2 = self.pane_container(second.any_leaf(), primary_scroll);
        let paned = find_split_paned(w1, w2);
        *ratio_permille = read_paned_ratio(paned, *orient, *ratio_permille);
        self.sync_node_ratios(first, primary_scroll);
        self.sync_node_ratios(second, primary_scroll);
    }

    fn apply_node_ratios(&self, node: &LayoutNode, primary_scroll: *mut GtkWidget) {
        let LayoutNode::Split { orient, ratio_permille, first, second } = node else {
            return;
        };
        let w1 = self.pane_container(first.any_leaf(), primary_scroll);
        let w2 = self.pane_container(second.any_leaf(), primary_scroll);
        let paned = find_split_paned(w1, w2);
        if !paned.is_null() {
            apply_paned_ratio_if_ready(paned, orient.to_gtk(), *ratio_permille);
        }
        self.apply_node_ratios(first, primary_scroll);
        self.apply_node_ratios(second, primary_scroll);
    }

    pub fn unzoom_in_place(&mut self, primary_scroll: *mut GtkWidget) -> bool {
        let Some(zoom) = self.zoom.take() else {
            return false;
        };
        let target = self.pane_container(zoom.zoomed_scroll, primary_scroll);
        unsafe {
            g_object_ref(target);
            gtk_container_remove(self.root, target);
            if zoom.was_first {
                gtk_paned_pack1(zoom.parent_paned, target, 1, 1);
            } else {
                gtk_paned_pack2(zoom.parent_paned, target, 1, 1);
            }
            g_object_unref(target);
            gtk_box_pack_start(self.root, zoom.tree_root, 1, 1, 0);
            g_object_unref(zoom.tree_root);
            gtk_widget_show_all(self.root);
        }
        self.sync_header_visibility();
        self.apply_node_ratios(&self.layout, primary_scroll);
        true
    }

    pub fn toggle_zoom(&mut self, primary_scroll: *mut GtkWidget, primary_vte: *mut GtkWidget) -> bool {
        if self.zoom.is_some() {
            let vte = self.zoomed_vte(primary_scroll, primary_vte).unwrap_or(primary_vte);
            self.unzoom_in_place(primary_scroll);
            if !vte.is_null() {
                unsafe { gtk_widget_grab_focus(vte) };
            }
            return true;
        }
        if self.extra.is_empty() {
            return false;
        }
        self.zoom_active_in(primary_scroll, primary_vte)
    }

    fn swap_zoom_widgets(root: *mut GtkWidget, tree_root: *mut GtkWidget, parent: *mut GtkWidget, target: *mut GtkWidget, vte: *mut GtkWidget) {
        unsafe {
            g_object_ref(tree_root);
            g_object_ref(target);
            gtk_container_remove(parent, target);
            gtk_container_remove(root, tree_root);
            gtk_box_pack_start(root, target, 1, 1, 0);
            g_object_unref(target);
            gtk_widget_show_all(target);
            gtk_widget_grab_focus(vte);
        }
    }

    fn zoom_active_in(&mut self, primary_scroll: *mut GtkWidget, primary_vte: *mut GtkWidget) -> bool {
        let target_vte = self.active_vte(primary_vte);
        let target_scroll = self.extra.iter().find(|p| p.vte == target_vte).map_or(primary_scroll, |p| p.scroll);
        let target = self.pane_container(target_scroll, primary_scroll);
        let tree_root = find_top_child(target, self.root);
        let parent_paned = unsafe { gtk_widget_get_parent(target) };
        if tree_root.is_null() || parent_paned.is_null() || parent_paned == self.root {
            return false;
        }
        let mut layout_copy = self.layout.clone();
        self.sync_node_ratios(&mut layout_copy, primary_scroll);
        self.layout = layout_copy;
        let was_first = unsafe { gtk_paned_get_child1(parent_paned) == target };
        Self::swap_zoom_widgets(self.root, tree_root, parent_paned, target, target_vte);
        self.zoom = Some(ZoomState { zoomed_scroll: target_scroll, tree_root, parent_paned, was_first });
        self.sync_header_visibility();
        true
    }

    pub fn focus_direction(
        &mut self,
        dir: FocusDir,
        primary_scroll: *mut GtkWidget,
        primary_vte: *mut GtkWidget,
    ) -> Option<*mut GtkWidget> {
        if self.extra.is_empty() || self.zoom.is_some() {
            return None;
        }
        let active_vte = self.active_vte(primary_vte);
        let mut panes = vec![(primary_scroll, primary_vte)];
        panes.extend(self.extra.iter().map(|p| (p.scroll, p.vte)));
        let active_idx = panes.iter().position(|&(_, v)| v == active_vte).unwrap_or(0);
        let active_scroll = panes[active_idx].0;
        let next_idx = self.pick_rect_neighbor(&panes, active_idx, dir)
            .or_else(|| {
                let s = self.layout.neighbor_leaf(active_scroll, dir)?;
                panes.iter().position(|&(scroll, _)| scroll == s)
            })?;
        let next_vte = panes[next_idx].1;
        self.last_focus = next_vte;
        unsafe { gtk_widget_grab_focus(next_vte) };
        Some(next_vte)
    }

    fn pick_rect_neighbor(
        &self,
        panes: &[(*mut GtkWidget, *mut GtkWidget)],
        active_idx: usize,
        dir: FocusDir,
    ) -> Option<usize> {
        let src_rect = pane_rect_in_root(panes[active_idx].0, self.root)?;
        let cands: Vec<(usize, PaneRect)> = panes
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != active_idx)
            .filter_map(|(i, &(scroll, _))| Some((i, pane_rect_in_root(scroll, self.root)?)))
            .collect();
        pick_neighbor_rect(src_rect, &cands, dir)
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

    pub fn pid_for_vte(
        &self,
        vte: *mut GtkWidget,
        primary_vte: *mut GtkWidget,
        primary_pid: i32,
    ) -> i32 {
        if vte == primary_vte {
            return primary_pid;
        }
        self.extra.iter().find(|p| p.vte == vte).map_or(primary_pid, |p| p.child_pid)
    }

    pub fn active_pid(&self, primary_vte: *mut GtkWidget, primary_pid: i32) -> i32 {
        self.pid_for_vte(self.active_vte(primary_vte), primary_vte, primary_pid)
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

    pub fn snapshot_splits(&mut self, primary_scroll: *mut GtkWidget) -> Option<SplitTree> {
        self.snapshot_splits_for_tab(primary_scroll, &Tab::local("shell"))
    }

    pub fn snapshot_splits_for_tab(
        &mut self,
        primary_scroll: *mut GtkWidget,
        tab: &Tab,
    ) -> Option<SplitTree> {
        if self.extra.is_empty() {
            return None;
        }
        if !self.is_zoomed() {
            let mut layout_copy = self.layout.clone();
            self.sync_node_ratios(&mut layout_copy, primary_scroll);
            self.layout = layout_copy;
        }
        if matches!(tab, Tab::Local { .. }) {
            for pane in &mut self.extra {
                if let Some(live) = foreground_cwd(pane.child_pid) {
                    pane.cwd = live;
                }
            }
        }
        let tree = self.layout.to_split_tree(primary_scroll, &self.extra, &self.remote_conns, tab);
        tree.is_valid_for(tab).then_some(tree)
    }

    pub fn prepare_restore_splits(
        &mut self,
        primary_scroll: *mut GtkWidget,
        primary_vte: *mut GtkWidget,
        tree: SplitTree,
        spawned: Vec<SpawnedPane>,
    ) -> Option<RestoreSplitsOp> {
        if !self.extra.is_empty() || !tree.is_valid() || spawned.is_empty() {
            return None;
        }
        let scrolls: Vec<*mut GtkWidget> = spawned.iter().map(|p| p.scroll).collect();
        let mut iter = scrolls.iter().copied();
        let next_layout = LayoutNode::from_tree(&tree, primary_scroll, &mut iter)?;
        if iter.next().is_some() {
            return None;
        }
        self.layout = next_layout;
        self.last_focus = primary_vte;
        let pane_widgets = self.push_restored_panes(spawned);
        self.sync_header_visibility();
        let primary_widget = self.primary_header.container;
        Some(RestoreSplitsOp { root: self.root, primary_widget, tree, pane_widgets })
    }

    fn push_restored_panes(&mut self, spawned: Vec<SpawnedPane>) -> Vec<*mut GtkWidget> {
        let mut pane_widgets = Vec::with_capacity(spawned.len());
        for p in spawned {
            let hdr = PaneHeader::wrap(p.scroll, p.vte);
            pane_widgets.push(hdr.container);
            self.extra_headers.push(hdr);
            if let Some(conn) = p.remote {
                self.remote_conns.push((p.scroll, conn));
            }
            self.extra.push(ExtraPane { scroll: p.scroll, vte: p.vte, child_pid: p.child_pid, cwd: p.cwd });
        }
        pane_widgets
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

    pub fn prepare_split_vte(
        &mut self,
        target_vte: *mut GtkWidget,
        primary_scroll: *mut GtkWidget,
        primary_vte: *mut GtkWidget,
        new_scroll: *mut GtkWidget,
        new_vte: *mut GtkWidget,
        new_pid: i32,
        cwd: String,
        orient: c_int,
    ) -> SplitOp {
        self.unzoom_in_place(primary_scroll);
        let chosen = if target_vte.is_null() { self.active_vte(primary_vte) } else { target_vte };
        let target_scroll = self.extra.iter().find(|p| p.vte == chosen).map_or(primary_scroll, |p| p.scroll);
        let target = self.pane_container(target_scroll, primary_scroll);
        let added_header = PaneHeader::wrap(new_scroll, new_vte);
        let added = added_header.container;
        self.layout.replace_leaf(target_scroll, new_scroll, SplitOrientation::from_gtk(orient));
        self.last_focus = new_vte;
        self.extra.push(ExtraPane { scroll: new_scroll, vte: new_vte, child_pid: new_pid, cwd });
        self.extra_headers.push(added_header);
        self.sync_header_visibility();
        SplitOp { target, added, root: self.root, orient }
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
        self.prepare_split_vte(
            ptr::null_mut(),
            primary_scroll,
            primary_vte,
            new_scroll,
            new_vte,
            new_pid,
            cwd,
            orient,
        )
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

    fn collapse_layout_leaf(&mut self, closing_scroll: *mut GtkWidget) {
        if let (Some(next), true) = self.layout.remove_leaf(closing_scroll) {
            self.layout = next;
        }
    }

    fn promote_first_extra(
        &mut self,
        scroll: &mut *mut GtkWidget,
        vte: &mut *mut GtkWidget,
        pid: &mut i32,
        cwd: &mut String,
    ) -> CloseOp {
        self.unzoom_in_place(*scroll);
        let (closing_scroll, kill_pid) = (*scroll, *pid);
        let closing_widget = self.primary_header.container;
        self.collapse_layout_leaf(closing_scroll);
        self.remote_conns.retain(|(s, _)| *s != closing_scroll);
        let next = self.extra.remove(0);
        if !self.extra_headers.is_empty() {
            self.primary_header = self.extra_headers.remove(0);
        }
        (*scroll, *vte, *pid, *cwd) = (next.scroll, next.vte, next.child_pid, next.cwd);
        self.last_focus = *vte;
        self.sync_header_visibility();
        CloseOp { closing_scroll: closing_widget, root: self.root, kill_pid, next_focus: *vte }
    }

    fn pick_close_index(&self) -> usize {
        self.extra
            .iter()
            .position(|p| is_widget_focused(p.vte))
            .or_else(|| self.extra.iter().position(|p| p.vte == self.last_focus))
            .unwrap_or(self.extra.len() - 1)
    }

    fn remove_extra_at(&mut self, idx: usize, primary_scroll: *mut GtkWidget, primary_vte: *mut GtkWidget, kill_proc: bool) -> CloseOp {
        self.unzoom_in_place(primary_scroll);
        let removed = self.extra.remove(idx);
        let closing_widget = if idx < self.extra_headers.len() {
            self.extra_headers.remove(idx).container
        } else {
            removed.scroll
        };
        self.collapse_layout_leaf(removed.scroll);
        self.remote_conns.retain(|(s, _)| *s != removed.scroll);
        let kill_pid = if kill_proc { removed.child_pid } else { 0 };
        let next_focus = self.extra.last().map_or(primary_vte, |p| p.vte);
        self.last_focus = next_focus;
        self.sync_header_visibility();
        CloseOp { closing_scroll: closing_widget, root: self.root, kill_pid, next_focus }
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
        Some(self.remove_extra_at(idx, *scroll, *vte, true))
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

    pub fn prepare_close_vte(
        &mut self,
        target_vte: *mut GtkWidget,
        scroll: &mut *mut GtkWidget,
        vte: &mut *mut GtkWidget,
        pid: &mut i32,
        cwd: &mut String,
    ) -> Option<CloseOp> {
        if self.extra.is_empty() {
            return None;
        }
        if target_vte == *vte {
            return Some(self.promote_first_extra(scroll, vte, pid, cwd));
        }
        let idx = self.extra.iter().position(|p| p.vte == target_vte)?;
        Some(self.remove_extra_at(idx, *scroll, *vte, true))
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
        Some(self.remove_extra_at(idx, *scroll, *vte, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::tabs::{Tab, TabMeta, TabsState};

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
        assert!(split_mi >= 58.25, "split_pane.rs MI {split_mi:.2} < 58.25");
        let (_, _, main_mi) = crate::mi::compute_file_mi(include_str!("main.rs"));
        assert!(main_mi >= 54.87, "main.rs MI {main_mi:.2} < 54.87");
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

    fn spawn_real_shells_for_tree(tree: &SplitTree, fallback: &str) -> Vec<SpawnedPane> {
        let mut cwds = Vec::new();
        tree.secondary_cwds(&mut cwds);
        cwds.into_iter()
            .map(|raw| {
                let cwd = resolve_pane_cwd(&raw, fallback);
                let (vte, child_pid) = crate::session::create_vte_widget(&cwd, &["/bin/sh"]);
                let scroll = wrap_vte_scroll(vte);
                SpawnedPane { scroll, vte, child_pid, cwd, remote: None }
            })
            .collect()
    }

    fn record_nested_real_splits(dir_right: &str, dir_bottom: &str) -> SplitTree {
        let (win, scroll1, vte1, mut panes) = make_window_with_pane();
        let (vte2, pid2) = crate::session::create_vte_widget(dir_right, &["/bin/sh"]);
        panes.split(scroll1, vte1, vte2, pid2, String::new(), GTK_ORIENTATION_HORIZONTAL);
        unsafe { gtk_widget_grab_focus(vte2) };
        pump_gtk_events();
        let (vte3, pid3) = crate::session::create_vte_widget(dir_bottom, &["/bin/sh"]);
        panes.split(scroll1, vte1, vte3, pid3, String::new(), GTK_ORIENTATION_VERTICAL);
        pump_gtk_events();
        let snap = panes.snapshot_splits(scroll1).expect("live split tree");
        panes.stop_all_extra();
        unsafe { gtk_widget_destroy(win) };
        pump_gtk_events();
        snap
    }

    fn verify_restored_real_splits(restored: SplitTree, s_main: &str, s_right: &str, s_bot: &str) {
        let (win2, mut r_scroll, mut r_vte, mut panes2) = make_window_with_pane();
        let spawned = spawn_real_shells_for_tree(&restored, s_main);
        let op = panes2.prepare_restore_splits(r_scroll, r_vte, restored.clone(), spawned);
        finish_restore_splits(op, &[]);
        pump_gtk_events();

        assert_eq!(panes2.snapshot_splits(r_scroll), Some(restored));
        assert_eq!(foreground_cwd(panes2.extra[0].child_pid).as_deref(), Some(s_right));
        assert_eq!(foreground_cwd(panes2.extra[1].child_pid).as_deref(), Some(s_bot));
        let (mut r_pid, mut r_cwd) = (0, s_main.to_owned());
        assert!(panes2.close_active(&mut r_scroll, &mut r_vte, &mut r_pid, &mut r_cwd).is_some());
        panes2.stop_all_extra();
        unsafe { gtk_widget_destroy(win2) };
        pump_gtk_events();
    }

    #[test]
    fn snapshots_and_restores_nested_split_tree_in_headless_gtk() {
        let base = std::env::temp_dir().join(format!("akhterm_split_cwd_{}", std::process::id()));
        let (d_main, d_right, d_bot) = (base.join("main"), base.join("правая"), base.join("нижняя"));
        for d in [&d_main, &d_right, &d_bot] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (s_main, s_right, s_bot) =
            (d_main.to_str().unwrap(), d_right.to_str().unwrap(), d_bot.to_str().unwrap());
        assert_eq!(resolve_pane_cwd("/nonexistent/akhterm_missing_xyz", s_main), s_main);

        with_headless_gtk(|| {
            let snap = record_nested_real_splits(s_right, s_bot);
            let meta = vec![TabMeta { pinned_title: None, splits: Some(snap) }];
            let state = TabsState::with_meta(0, vec![Tab::Local { profile: "shell".into(), cwd: s_main.into() }], meta);
            let restored = TabsState::decode(&state.encode()).unwrap().splits_at(0).unwrap().clone();
            verify_restored_real_splits(restored, s_main, s_right, s_bot);
        });

        for d in [&d_main, &d_right, &d_bot, &base] {
            let _ = std::fs::remove_dir(d);
        }
    }

    #[test]
    fn directional_rect_picker_selects_closest_neighbor() {
        let grid = [
            (0, PaneRect { x: 0, y: 0, w: 400, h: 600 }),
            (1, PaneRect { x: 406, y: 0, w: 394, h: 297 }),
            (2, PaneRect { x: 406, y: 303, w: 394, h: 297 }),
        ];
        assert_eq!(pick_neighbor_rect(grid[0].1, &grid[1..], FocusDir::Right), Some(1));
        assert_eq!(pick_neighbor_rect(grid[1].1, &grid[..1], FocusDir::Left), Some(0));
        assert_eq!(pick_neighbor_rect(grid[1].1, &grid[2..], FocusDir::Down), Some(2));
        assert_eq!(pick_neighbor_rect(grid[2].1, &grid[1..2], FocusDir::Up), Some(1));
    }

    fn verify_dir_focus_cycle(panes: &mut TabPanes, scroll1: *mut GtkWidget, vte1: *mut GtkWidget, vte2: *mut GtkWidget, vte3: *mut GtkWidget) {
        unsafe { gtk_widget_grab_focus(vte1) };
        pump_gtk_events();
        assert_eq!(panes.focus_direction(FocusDir::Right, scroll1, vte1), Some(vte2));
        pump_gtk_events();
        assert_eq!(panes.focus_direction(FocusDir::Down, scroll1, vte1), Some(vte3));
        pump_gtk_events();
        assert_eq!(panes.focus_direction(FocusDir::Up, scroll1, vte1), Some(vte2));
        pump_gtk_events();
        assert_eq!(panes.focus_direction(FocusDir::Left, scroll1, vte1), Some(vte1));
        pump_gtk_events();
    }

    fn verify_zoom_toggle_round_trip(panes: &mut TabPanes, scroll1: *mut GtkWidget, vte1: *mut GtkWidget) {
        assert!(panes.toggle_zoom(scroll1, vte1));
        pump_gtk_events();
        assert!(panes.is_zoomed());
        assert_eq!(panes.zoomed_vte(scroll1, vte1), Some(vte1));
        assert!(panes.all_headers()[0].is_visible(), "zoomed pane keeps header visible");
        assert!(panes.toggle_zoom(scroll1, vte1));
        pump_gtk_events();
        assert!(!panes.is_zoomed());
        assert_eq!(panes.extra.len(), 2);
    }

    #[test]
    fn headless_gtk_supports_directional_nav_zoom_and_header_visibility() {
        with_headless_gtk(|| {
            let (win, mut scroll1, mut vte1, mut panes) = make_window_with_pane();
            let (mut pid1, mut cwd1) = (0, "/tmp/one".to_owned());
            assert!(!panes.all_headers()[0].is_visible(), "1 pane header must be hidden");
            let vte2 = unsafe { vte_terminal_new() };
            panes.split(scroll1, vte1, vte2, 0, "/tmp/two".into(), GTK_ORIENTATION_HORIZONTAL);
            pump_gtk_events();
            assert!(panes.all_headers().iter().all(|h| h.is_visible()), "2+ panes show headers");
            let vte3 = unsafe { vte_terminal_new() };
            panes.split(scroll1, vte2, vte3, 0, "/tmp/three".into(), GTK_ORIENTATION_VERTICAL);
            pump_gtk_events();
            verify_dir_focus_cycle(&mut panes, scroll1, vte1, vte2, vte3);
            verify_zoom_toggle_round_trip(&mut panes, scroll1, vte1);
            assert!(panes.close_active(&mut scroll1, &mut vte1, &mut pid1, &mut cwd1).is_some());
            assert!(panes.close_active(&mut scroll1, &mut vte1, &mut pid1, &mut cwd1).is_some());
            pump_gtk_events();
            assert!(!panes.all_headers()[0].is_visible(), "back to 1 pane hides header");
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
        });
    }
}
