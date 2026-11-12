//! Compact 22px header bar shown above each split pane when >= 2 panes or zoomed.

use std::ffi::{c_int, c_void, CString};
use std::ptr;
use std::sync::{Once, OnceLock};

use crate::ffi::{
    g_signal_connect_data, gdk_screen_get_default, gtk_box_new, gtk_box_pack_end,
    gtk_box_pack_start, gtk_button_new_with_label, gtk_css_provider_load_from_data,
    gtk_css_provider_new, gtk_label_new, gtk_label_set_ellipsize, gtk_label_set_text,
    gtk_label_set_xalign, gtk_style_context_add_class, gtk_style_context_add_provider_for_screen,
    gtk_style_context_remove_class, gtk_widget_get_style_context, gtk_widget_get_visible,
    gtk_widget_set_can_focus, gtk_widget_set_no_show_all, gtk_widget_set_size_request,
    gtk_widget_set_tooltip_text, gtk_widget_set_visible, gtk_widget_show_all, GtkBox, GtkLabel,
    GtkWidget, GTK_ORIENTATION_HORIZONTAL, GTK_ORIENTATION_VERTICAL,
    GTK_STYLE_PROVIDER_PRIORITY_APPLICATION, PANGO_ELLIPSIZE_END, PANGO_ELLIPSIZE_MIDDLE,
};
use crate::i18n::UiText;

pub const PANE_HEADER_HEIGHT_PX: c_int = 22;
const MAX_CWD_CHARS: usize = 28;
const TXT_ZOOMED: UiText = UiText::new(c"Увеличена", c"Zoomed");
const TXT_CLOSE_TIP: UiText = UiText::new(c"Закрыть панель", c"Close pane");
const TXT_DEFAULT_PROC: UiText = UiText::new(c"оболочка", c"shell");

const PANE_HEADER_CSS: &str = "\
.pane-header-bar { min-height: 22px; padding: 0 6px; background-color: #18181b; border-bottom: 1px solid #27272a; color: #a1a1aa; font-size: 11px; }\n\
.pane-header-bar.pane-header-active { background-color: #202026; border-bottom-color: #3f3f46; color: #e4e4e7; }\n\
.pane-header-bar.pane-header-zoomed { border-bottom-color: #7c3aed; }\n\
.pane-header-agent { font-weight: 600; color: #e4e4e7; }\n\
.pane-header-cwd { color: #a1a1aa; }\n\
.pane-header-status { color: #a1a1aa; }\n\
.pane-header-ctx { color: #60a5fa; font-weight: 600; }\n\
.pane-header-close { min-height: 16px; min-width: 16px; padding: 0 4px; margin: 0; border: none; background: transparent; color: #71717a; border-radius: 3px; }\n\
.pane-header-close:hover { background-color: #3f3f46; color: #f4f4f5; }\n";

static CSS_ONCE: Once = Once::new();
static CLOSE_PANE_HOOK: OnceLock<fn(*mut GtkWidget)> = OnceLock::new();

pub fn register_pane_close_hook(cb: fn(*mut GtkWidget)) {
    let _ = CLOSE_PANE_HOOK.set(cb);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneHeaderState {
    pub agent_or_proc: String,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub elapsed_secs: Option<u64>,
    pub ctx_badge: String,
    pub active: bool,
    pub zoomed: bool,
}

#[derive(Clone, Copy)]
pub struct PaneHeader {
    pub container: *mut GtkWidget,
    pub bar: *mut GtkWidget,
    agent_label: *mut GtkWidget,
    cwd_label: *mut GtkWidget,
    status_label: *mut GtkWidget,
    ctx_label: *mut GtkWidget,
}

fn install_header_css() {
    unsafe {
        let screen = gdk_screen_get_default();
        if screen.is_null() {
            return;
        }
        let provider = gtk_css_provider_new();
        if let Ok(css) = CString::new(PANE_HEADER_CSS) {
            gtk_css_provider_load_from_data(provider, css.as_ptr(), -1, ptr::null_mut());
            gtk_style_context_add_provider_for_screen(
                screen, provider, GTK_STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    }
}

fn ensure_pane_header_css() {
    CSS_ONCE.call_once(install_header_css);
}

fn add_css_class(widget: *mut GtkWidget, class_name: &std::ffi::CStr) {
    unsafe {
        let ctx = gtk_widget_get_style_context(widget);
        gtk_style_context_add_class(ctx, class_name.as_ptr());
    }
}

fn set_css_class(widget: *mut GtkWidget, class_name: &std::ffi::CStr, enabled: bool) {
    unsafe {
        let ctx = gtk_widget_get_style_context(widget);
        if enabled {
            gtk_style_context_add_class(ctx, class_name.as_ptr());
        } else {
            gtk_style_context_remove_class(ctx, class_name.as_ptr());
        }
    }
}

fn make_header_label(class_name: &std::ffi::CStr, ellipsize: c_int) -> *mut GtkWidget {
    unsafe {
        let label = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(label as *mut GtkLabel, 0.0);
        gtk_label_set_ellipsize(label as *mut GtkLabel, ellipsize);
        add_css_class(label, class_name);
        label
    }
}

fn make_close_button(vte: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new_with_label(c"×".as_ptr());
        gtk_widget_set_can_focus(btn, 0);
        gtk_widget_set_tooltip_text(btn, TXT_CLOSE_TIP.cstr().as_ptr());
        add_css_class(btn, c"pane-header-close");
        let sig = c"clicked".as_ptr();
        let cb = on_close_clicked as *const c_void;
        g_signal_connect_data(btn as *mut c_void, sig, cb, vte as *mut c_void, ptr::null(), 0);
        btn
    }
}

extern "C" fn on_close_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if let Some(hook) = CLOSE_PANE_HOOK.get() {
        hook(data as *mut GtkWidget);
    }
}

fn set_label_str(label: *mut GtkWidget, text: &str) {
    let clean = text.replace('\0', "");
    let Ok(c_text) = CString::new(clean) else {
        return;
    };
    unsafe { gtk_label_set_text(label as *mut GtkLabel, c_text.as_ptr()) };
}

fn build_bar(
    agent: *mut GtkWidget,
    cwd: *mut GtkWidget,
    status: *mut GtkWidget,
    ctx: *mut GtkWidget,
    close: *mut GtkWidget,
) -> *mut GtkWidget {
    unsafe {
        let bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        gtk_widget_set_size_request(bar, -1, PANE_HEADER_HEIGHT_PX);
        add_css_class(bar, c"pane-header-bar");
        gtk_box_pack_start(bar as *mut GtkBox, agent, 0, 0, 0);
        gtk_box_pack_start(bar as *mut GtkBox, cwd, 1, 1, 0);
        gtk_box_pack_end(bar as *mut GtkBox, close, 0, 0, 0);
        gtk_box_pack_end(bar as *mut GtkBox, ctx, 0, 0, 0);
        gtk_box_pack_end(bar as *mut GtkBox, status, 0, 0, 0);
        gtk_widget_show_all(bar);
        gtk_widget_set_no_show_all(bar, 1);
        gtk_widget_set_visible(bar, 0);
        bar
    }
}

fn wrap_header_vbox(bar: *mut GtkWidget, scroll: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_box_pack_start(vbox as *mut GtkBox, bar, 0, 1, 0);
        gtk_box_pack_start(vbox as *mut GtkBox, scroll, 1, 1, 0);
        gtk_widget_set_visible(scroll, 1);
        gtk_widget_set_visible(vbox, 1);
        vbox
    }
}

impl PaneHeader {
    pub fn wrap(scroll: *mut GtkWidget, vte: *mut GtkWidget) -> Self {
        ensure_pane_header_css();
        let agent_label = make_header_label(c"pane-header-agent", PANGO_ELLIPSIZE_END);
        let cwd_label = make_header_label(c"pane-header-cwd", PANGO_ELLIPSIZE_MIDDLE);
        let status_label = make_header_label(c"pane-header-status", PANGO_ELLIPSIZE_END);
        let ctx_label = make_header_label(c"pane-header-ctx", PANGO_ELLIPSIZE_END);
        let close = make_close_button(vte);
        let bar = build_bar(agent_label, cwd_label, status_label, ctx_label, close);
        let container = wrap_header_vbox(bar, scroll);
        Self { container, bar, agent_label, cwd_label, status_label, ctx_label }
    }

    #[cfg(test)]
    pub fn empty_for_test() -> Self {
        Self {
            container: ptr::null_mut(),
            bar: ptr::null_mut(),
            agent_label: ptr::null_mut(),
            cwd_label: ptr::null_mut(),
            status_label: ptr::null_mut(),
            ctx_label: ptr::null_mut(),
        }
    }

    pub fn set_visible(&self, visible: bool) {
        if !self.bar.is_null() {
            unsafe { gtk_widget_set_visible(self.bar, c_int::from(visible)) };
        }
    }

    pub fn is_visible(&self) -> bool {
        unsafe { gtk_widget_get_visible(self.bar) != 0 }
    }

    pub fn update(&self, state: &PaneHeaderState, visible: bool) {
        set_label_str(self.agent_label, &format_agent_title(state));
        set_label_str(self.cwd_label, &format_short_cwd(&state.cwd));
        set_label_str(
            self.status_label,
            &format_status_badge(state.exit_code, state.elapsed_secs),
        );
        set_label_str(self.ctx_label, &state.ctx_badge);
        set_css_class(self.bar, c"pane-header-active", state.active);
        set_css_class(self.bar, c"pane-header-zoomed", state.zoomed);
        self.set_visible(visible);
    }

    #[cfg(test)]
    pub fn agent_text(&self) -> String {
        unsafe {
            let ptr = crate::ffi::gtk_label_get_text(self.agent_label as *mut GtkLabel);
            if ptr.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }
}

pub fn format_agent_title(state: &PaneHeaderState) -> String {
    let base = state.agent_or_proc.trim();
    let label = if base.is_empty() { TXT_DEFAULT_PROC.as_str() } else { base };
    if state.zoomed {
        format!("{label} · {}", TXT_ZOOMED.as_str())
    } else {
        label.to_owned()
    }
}

pub fn format_short_cwd(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let home = std::env::var("HOME").ok();
    let replaced = match home.as_deref().filter(|h| !h.is_empty() && trimmed.starts_with(h)) {
        Some(h) if trimmed.len() == h.len() => "~".to_owned(),
        Some(h) => format!("~{}", &trimmed[h.len()..]),
        None => trimmed.to_owned(),
    };
    truncate_left_ellipsis(&replaced, MAX_CWD_CHARS)
}

fn truncate_left_ellipsis(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let tail: String = text.chars().skip(total - keep).collect();
    format!("…{tail}")
}

pub fn format_elapsed(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub fn format_status_badge(exit_code: Option<i32>, elapsed_secs: Option<u64>) -> String {
    let mark = match exit_code {
        Some(0) => "✓".to_owned(),
        Some(code) => format!("✗{code}"),
        None => String::new(),
    };
    match (elapsed_secs.map(format_elapsed), mark.is_empty()) {
        (Some(t), false) => format!("{t} {mark}"),
        (Some(t), true) => t,
        (None, false) => mark,
        (None, true) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::{
        gtk_container_add, gtk_scrolled_window_new, gtk_widget_destroy, gtk_window_new,
        gtk_window_set_default_size, GtkContainer, GtkWindow, GTK_WINDOW_TOPLEVEL,
    };
    use std::sync::atomic::{AtomicI32, Ordering};

    extern "C" {
        fn gtk_widget_get_preferred_height(
            widget: *mut GtkWidget,
            minimum_height: *mut c_int,
            natural_height: *mut c_int,
        );
    }

    static CLOSED_TAG: AtomicI32 = AtomicI32::new(0);

    fn flush_gtk() {
        for _ in 0..5 {
            crate::proc_radar::tests::pump_gtk_events();
        }
    }

    fn hook_record(vte: *mut GtkWidget) {
        CLOSED_TAG.store(vte as usize as i32, Ordering::SeqCst);
    }

    #[test]
    fn formatting_helpers_handle_cwd_exit_badges_and_zoom_label() {
        assert_eq!(format_status_badge(Some(0), Some(65)), "1:05 ✓");
        assert_eq!(format_status_badge(Some(2), None), "✗2");
        assert_eq!(format_status_badge(None, Some(9)), "0:09");
        assert_eq!(format_status_badge(None, None), "");

        let long_cwd = "/home/user/workspace/deeply/nested/project/crates/core";
        let short = format_short_cwd(long_cwd);
        assert!(short.starts_with('…'));
        assert!(short.chars().count() <= MAX_CWD_CHARS);

        let zoomed = PaneHeaderState {
            agent_or_proc: "claude".into(),
            zoomed: true,
            ..Default::default()
        };
        assert!(format_agent_title(&zoomed).contains("claude"));
    }

    fn sample_zoomed_state() -> PaneHeaderState {
        PaneHeaderState {
            agent_or_proc: "ssh gpu".into(),
            cwd: "/tmp/work".into(),
            exit_code: Some(0),
            elapsed_secs: Some(15),
            ctx_badge: "12k".into(),
            active: true,
            zoomed: true,
        }
    }

    fn verify_header_toggles(header: &PaneHeader) {
        assert!(!header.is_visible(), "1-pane header must stay hidden after show_all");
        header.update(&sample_zoomed_state(), true);
        flush_gtk();
        assert!(header.is_visible());
        let (mut min_h, mut nat_h) = (0, 0);
        unsafe { gtk_widget_get_preferred_height(header.bar, &mut min_h, &mut nat_h) };
        assert!(min_h >= PANE_HEADER_HEIGHT_PX && nat_h >= PANE_HEADER_HEIGHT_PX);
        assert!(header.agent_text().contains("ssh gpu"));
        header.set_visible(false);
        flush_gtk();
        assert!(!header.is_visible());
    }

    #[test]
    fn gtk_pane_header_toggles_22px_visibility_and_respects_no_show_all() {
        crate::proc_radar::tests::with_headless_gtk(|| {
            register_pane_close_hook(hook_record);
            unsafe {
                let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
                gtk_window_set_default_size(win as *mut GtkWindow, 480, 320);
                let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
                let header = PaneHeader::wrap(scroll, 0x1234usize as *mut GtkWidget);
                gtk_container_add(win as *mut GtkContainer, header.container);
                gtk_widget_show_all(win);
                flush_gtk();
                verify_header_toggles(&header);
                gtk_widget_destroy(win);
            }
        });
    }

    #[test]
    fn pane_header_module_meets_mi_gate() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("pane_header.rs"));
        assert!(mi >= 60.0, "pane_header.rs MI {mi:.2} < 60.0");
    }
}
