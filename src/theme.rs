//! shadcn/ui Dark Zinc GTK3 CSS stylesheet and VTE-2.91 16-color palette.

use std::ffi::{c_char, c_int, CString};
use std::ptr;

use crate::ffi::{
    g_object_set, g_object_unref, gdk_screen_get_default, gtk_css_provider_load_from_data,
    gtk_css_provider_new, gtk_settings_get_default, gtk_style_context_add_provider_for_screen,
    GdkRGBA, GTK_STYLE_PROVIDER_PRIORITY_APPLICATION,
};

const SHADCN_GTK_CSS: &str = concat!(
    include_str!("theme.css"),
    include_str!("tab_strip.css"),
    include_str!("sidebar.css"),
);

/// Apply the application styles after GTK has initialized the default screen.
pub unsafe fn apply_gtk_theme() {
    let settings = gtk_settings_get_default();
    g_object_set(
        settings,
        c"gtk-application-prefer-dark-theme".as_ptr(),
        1 as c_int,
        ptr::null::<c_char>(),
    );

    let provider = gtk_css_provider_new();
    let css = CString::new(SHADCN_GTK_CSS).unwrap();
    gtk_css_provider_load_from_data(provider, css.as_ptr(), -1, ptr::null_mut());
    gtk_style_context_add_provider_for_screen(
        gdk_screen_get_default(),
        provider,
        GTK_STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    g_object_unref(provider);
}

pub const TERM_BG: GdkRGBA = GdkRGBA::rgb(9, 9, 11);
pub const TERM_FG: GdkRGBA = GdkRGBA::rgb(250, 250, 250);
pub const TERM_CURSOR: GdkRGBA = GdkRGBA::rgb(139, 92, 246);

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
