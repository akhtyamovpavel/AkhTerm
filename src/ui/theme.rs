//! shadcn/ui Dark Zinc and Light Zinc GTK3 CSS stylesheets and VTE-2.91 16-color palettes.

use std::ffi::{c_char, CString};
use std::ptr;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use crate::ffi::{
    g_object_set, gdk_screen_get_default, gtk_button_set_label, gtk_css_provider_load_from_data,
    gtk_css_provider_new, gtk_settings_get_default, gtk_style_context_add_provider_for_screen,
    gtk_style_context_reset_widgets, gtk_widget_set_name, gtk_widget_set_tooltip_text,
    vte_terminal_set_color_cursor, vte_terminal_set_colors, GdkRGBA, GtkCssProvider, GtkWidget,
    GTK_STYLE_PROVIDER_PRIORITY_APPLICATION,
};
use crate::theme_mode::{build_stylesheet, ThemeMode};

const DARK_THEME_CSS: &str = include_str!("theme.css");
const EXTRA_COMPONENTS_CSS: &str = concat!(
    include_str!("../workspace/tab_strip.css"),
    include_str!("../workspace/sidebar.css"),
    include_str!("palette.css"),
);

/// Above `GTK_STYLE_PROVIDER_PRIORITY_USER` (800) so `kde-gtk-config`'s
/// `~/.config/gtk-3.0/gtk.css` (`window_decorations.css`) cannot hide titlebuttons.
pub const APP_CSS_PROVIDER_PRIORITY: u32 = GTK_STYLE_PROVIDER_PRIORITY_APPLICATION + 201;

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(0);
static APP_CSS_PROVIDER: AtomicUsize = AtomicUsize::new(0);

pub fn current_theme_mode() -> ThemeMode {
    if ACTIVE_THEME.load(Ordering::Acquire) == 1 {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    }
}

fn record_theme_mode(mode: ThemeMode) {
    let encoded = u8::from(mode == ThemeMode::Light);
    ACTIVE_THEME.store(encoded, Ordering::Release);
}

fn ensure_css_provider() -> *mut GtkCssProvider {
    let existing = APP_CSS_PROVIDER.load(Ordering::Acquire) as *mut GtkCssProvider;
    if !existing.is_null() {
        return existing;
    }
    unsafe {
        let provider = gtk_css_provider_new();
        gtk_style_context_add_provider_for_screen(
            gdk_screen_get_default(),
            provider,
            APP_CSS_PROVIDER_PRIORITY,
        );
        APP_CSS_PROVIDER.store(provider as usize, Ordering::Release);
        provider
    }
}

/// Apply Dark Zinc or Light Zinc live across GTK settings, native header bar, and CSS provider.
pub fn apply_gtk_theme_mode(mode: ThemeMode) {
    record_theme_mode(mode);
    let css = CString::new(build_stylesheet(mode, DARK_THEME_CSS, EXTRA_COMPONENTS_CSS)).unwrap();
    unsafe {
        let settings = gtk_settings_get_default();
        if !settings.is_null() {
            g_object_set(
                settings,
                c"gtk-application-prefer-dark-theme".as_ptr(),
                mode.prefer_dark_int(),
                ptr::null::<c_char>(),
            );
        }
        let provider = ensure_css_provider();
        gtk_css_provider_load_from_data(provider, css.as_ptr(), -1, ptr::null_mut());
        let screen = gdk_screen_get_default();
        if !screen.is_null() {
            gtk_style_context_reset_widgets(screen);
        }
    }
}

/// Apply the default Dark Zinc application theme after GTK has initialized.
#[allow(dead_code)]
pub unsafe fn apply_gtk_theme() {
    apply_gtk_theme_mode(ThemeMode::Dark);
}

pub fn sync_theme_button(button: *mut GtkWidget, mode: ThemeMode) {
    if button.is_null() {
        return;
    }
    unsafe {
        gtk_button_set_label(button, mode.toggle_icon().as_ptr());
        gtk_widget_set_tooltip_text(button, mode.toggle_tooltip().as_ptr());
    }
}

pub fn set_agent_accent(icon: *mut GtkWidget, agent: &str) {
    let name = match agent {
        "AGY" => c"agent-agy",
        "DeepSeek" => c"agent-deepseek",
        "Codex" => c"agent-codex",
        "Claude" => c"agent-claude",
        "Kimi" => c"agent-kimi",
        _ => c"",
    };
    unsafe { gtk_widget_set_name(icon, name.as_ptr()) };
}

pub const TERM_BG: GdkRGBA = GdkRGBA::rgb(9, 9, 11);
pub const TERM_FG: GdkRGBA = GdkRGBA::rgb(250, 250, 250);
pub const TERM_CURSOR: GdkRGBA = GdkRGBA::rgb(139, 92, 246);

pub fn dark_vte_colors() -> (&'static GdkRGBA, &'static GdkRGBA, &'static GdkRGBA, &'static [GdkRGBA; 16]) {
    (&TERM_FG, &TERM_BG, &TERM_CURSOR, &TERM_PALETTE)
}

pub const TERM_PALETTE: [GdkRGBA; 16] = [
    GdkRGBA::rgb(24, 24, 27),    // 0: Black (Zinc-900)
    GdkRGBA::rgb(244, 63, 94),   // 1: Red (Rose-500)
    GdkRGBA::rgb(16, 185, 129),  // 2: Green (Emerald-500)
    GdkRGBA::rgb(245, 158, 11),  // 3: Yellow (Amber-500)
    GdkRGBA::rgb(59, 130, 246),  // 4: Blue (Blue-500)
    GdkRGBA::rgb(139, 92, 246),  // 5: Magenta (Violet-500)
    GdkRGBA::rgb(6, 182, 212),   // 6: Cyan (Cyan-500)
    GdkRGBA::rgb(228, 228, 231), // 7: White (Zinc-200)
    GdkRGBA::rgb(82, 82, 91),    // 8: Bright Black (Zinc-600)
    GdkRGBA::rgb(251, 113, 133), // 9: Bright Red (Rose-400)
    GdkRGBA::rgb(52, 211, 153),  // 10: Bright Green (Emerald-400)
    GdkRGBA::rgb(251, 191, 36),  // 11: Bright Yellow (Amber-400)
    GdkRGBA::rgb(96, 165, 250),  // 12: Bright Blue (Blue-400)
    GdkRGBA::rgb(167, 139, 250), // 13: Bright Magenta (Violet-400)
    GdkRGBA::rgb(34, 211, 238),  // 14: Bright Cyan (Cyan-400)
    GdkRGBA::rgb(255, 255, 255), // 15: Bright White
];

pub fn vte_colors_for(
    mode: ThemeMode,
) -> (&'static GdkRGBA, &'static GdkRGBA, &'static GdkRGBA, &'static [GdkRGBA; 16]) {
    match mode {
        ThemeMode::Dark => dark_vte_colors(),
        ThemeMode::Light => light_vte_colors(),
    }
}

pub fn apply_vte_colors(vte: *mut GtkWidget, mode: ThemeMode) {
    if vte.is_null() {
        return;
    }
    let (fg, bg, cursor, palette) = vte_colors_for(mode);
    unsafe {
        vte_terminal_set_colors(vte, fg, bg, palette.as_ptr(), 16);
        vte_terminal_set_color_cursor(vte, cursor);
    }
}

pub fn apply_vte_list_colors(vtes: &[*mut GtkWidget], mode: ThemeMode) {
    for &vte in vtes {
        apply_vte_colors(vte, mode);
    }
}

pub const TERM_BG_LIGHT: GdkRGBA = GdkRGBA::rgb(255, 255, 255);
pub const TERM_FG_LIGHT: GdkRGBA = GdkRGBA::rgb(24, 24, 27);
pub const TERM_CURSOR_LIGHT: GdkRGBA = GdkRGBA::rgb(124, 58, 237);

pub fn light_vte_colors() -> (&'static GdkRGBA, &'static GdkRGBA, &'static GdkRGBA, &'static [GdkRGBA; 16]) {
    (&TERM_FG_LIGHT, &TERM_BG_LIGHT, &TERM_CURSOR_LIGHT, &TERM_PALETTE_LIGHT)
}

pub const TERM_PALETTE_LIGHT: [GdkRGBA; 16] = [
    GdkRGBA::rgb(24, 24, 27),    // 0: Black (Zinc-900)
    GdkRGBA::rgb(190, 18, 60),   // 1: Red (Rose-700)
    GdkRGBA::rgb(6, 95, 70),     // 2: Green (Emerald-800)
    GdkRGBA::rgb(146, 64, 14),   // 3: Yellow (Amber-800)
    GdkRGBA::rgb(30, 64, 175),   // 4: Blue (Blue-800)
    GdkRGBA::rgb(109, 40, 217),  // 5: Magenta (Violet-700)
    GdkRGBA::rgb(21, 94, 117),   // 6: Cyan (Cyan-800)
    GdkRGBA::rgb(228, 228, 231), // 7: White (Zinc-200)
    GdkRGBA::rgb(82, 82, 91),    // 8: Bright Black (Zinc-600)
    GdkRGBA::rgb(225, 29, 72),   // 9: Bright Red (Rose-600)
    GdkRGBA::rgb(4, 120, 87),    // 10: Bright Green (Emerald-700)
    GdkRGBA::rgb(180, 83, 9),    // 11: Bright Yellow (Amber-700)
    GdkRGBA::rgb(37, 99, 235),   // 12: Bright Blue (Blue-600)
    GdkRGBA::rgb(124, 58, 237),  // 13: Bright Magenta (Violet-600)
    GdkRGBA::rgb(14, 116, 144),  // 14: Bright Cyan (Cyan-700)
    GdkRGBA::rgb(244, 244, 245), // 15: Bright White (Zinc-100)
];
