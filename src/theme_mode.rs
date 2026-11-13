//! Dark Zinc and Light Zinc theme mode state, shortcut matching, CSS token sheet, and persistence.

use std::ffi::{c_int, c_uint, CStr};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ffi::{GDK_CONTROL_MASK, GDK_MOD1_MASK, GDK_SHIFT_MASK};
use crate::paths::{state_dir, write_state_file};
use crate::ui_text as text;

pub const KEYCODE_L: u16 = 46;
const THEME_STATE_HEADER: &str = "akhterm-theme-v1";

pub const LIGHT_ZINC_TOKENS: &str = "\
@define-color canvas #ffffff;\n\
@define-color sidebar_bg #fafafa;\n\
@define-color surface #f4f4f5;\n\
@define-color elevated #ffffff;\n\
@define-color control_bg #f4f4f5;\n\
@define-color input_bg #ffffff;\n\
@define-color palette_entry_focus #ffffff;\n\
@define-color hover #ececef;\n\
@define-color selected #e4e4e7;\n\
@define-color border #e4e4e7;\n\
@define-color border_strong #d4d4d8;\n\
@define-color selected_border #a1a1aa;\n\
@define-color drag_bg #e4e4ec;\n\
@define-color drag_border #7c3aed;\n\
@define-color text #18181b;\n\
@define-color text_strong #09090b;\n\
@define-color text_secondary #27272a;\n\
@define-color row_text #27272a;\n\
@define-color muted #52525b;\n\
@define-color disabled_text #a1a1aa;\n\
@define-color focus #71717a;\n\
@define-color caret #7c3aed;\n\
@define-color scroll_thumb #d4d4d8;\n\
@define-color scroll_thumb_hover #a1a1aa;\n\
@define-color ready #047857;\n\
@define-color unread_border #059669;\n\
@define-color unread_fg #065f46;\n\
@define-color mi_b #b45309;\n\
@define-color mi_c #be123c;\n\
@define-color status_working #1d4ed8;\n\
@define-color approval_fg #b45309;\n\
@define-color approval_bg rgba(245, 158, 11, 0.12);\n\
@define-color approval_border rgba(217, 119, 6, 0.50);\n\
@define-color amber_fg #92400e;\n\
@define-color amber_tint rgba(245, 158, 11, 0.14);\n\
@define-color amber_border rgba(217, 119, 6, 0.55);\n\
@define-color amber_border_strong #d97706;\n\
@define-color gpu_bg #f5f3ff;\n\
@define-color gpu_fg #5b21b6;\n\
@define-color agent_agy_fg #5b21b6;\n\
@define-color agent_agy_bg #ede9fe;\n\
@define-color agent_deepseek_fg #1d4ed8;\n\
@define-color agent_deepseek_bg #dbeafe;\n\
@define-color agent_codex_fg #18181b;\n\
@define-color agent_codex_bg #e4e4e7;\n\
@define-color agent_claude_fg #c2410c;\n\
@define-color agent_claude_bg #ffedd5;\n\
@define-color agent_kimi_fg #0369a1;\n\
@define-color agent_kimi_bg #e0f2fe;\n\
@define-color badge_tab_border rgba(4, 120, 87, 0.35);\n\
@define-color badge_tmux_border rgba(217, 119, 6, 0.35);\n\
@define-color badge_agent_fg #5b21b6;\n\
@define-color badge_agent_border rgba(109, 40, 217, 0.30);\n\
@define-color badge_ssh_fg #1d4ed8;\n\
@define-color badge_ssh_border rgba(29, 78, 216, 0.30);\n\
@define-color badge_handoff_fg #c2410c;\n\
@define-color badge_handoff_border rgba(194, 65, 12, 0.30);\n\
@define-color badge_git_fg #047857;\n\
@define-color badge_git_border rgba(4, 120, 87, 0.35);\n\
@define-color search_err_border #e11d48;\n\
@define-color danger #be123c;\n\
@define-color danger_bg rgba(225, 29, 72, 0.12);\n\
@define-color danger_border rgba(225, 29, 72, 0.38);\n";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn prefer_dark_int(self) -> c_int {
        match self {
            Self::Dark => 1,
            Self::Light => 0,
        }
    }

    pub fn parse_config(raw: &str) -> Option<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    pub fn toggle_icon(self) -> &'static CStr {
        text::theme_toggle_icon(self == Self::Light)
    }

    pub fn toggle_tooltip(self) -> &'static CStr {
        text::theme_toggle_tooltip(self == Self::Light)
    }

    pub fn palette_title(self) -> &'static str {
        match self {
            Self::Dark => "☀ Светлая тема (Light Zinc)",
            Self::Light => "☾ Тёмная тема (Dark Zinc)",
        }
    }

    pub fn palette_detail(self) -> &'static str {
        match self {
            Self::Dark => "Сейчас: Dark Zinc · Ctrl+Shift+L",
            Self::Light => "Сейчас: Light Zinc · Ctrl+Shift+L",
        }
    }

    pub fn mi_markup_colors(self) -> (&'static str, &'static str) {
        match self {
            Self::Dark => ("#a1a1aa", "#c4b5fd"),
            Self::Light => ("#52525b", "#5b21b6"),
        }
    }

    pub fn tokens_markup_colors(self) -> TokensMarkupColors {
        match self {
            Self::Dark => TokensMarkupColors {
                high: "#fda4af",
                warn: "#e7c389",
                timer: "#9bbcf3",
                muted: "#a1a1aa",
                summary: "#c4b5fd",
            },
            Self::Light => TokensMarkupColors {
                high: "#be123c",
                warn: "#b45309",
                timer: "#1d4ed8",
                muted: "#52525b",
                summary: "#5b21b6",
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokensMarkupColors {
    pub high: &'static str,
    pub warn: &'static str,
    pub timer: &'static str,
    pub muted: &'static str,
    pub summary: &'static str,
}

fn is_l_or_cyrillic_de(keyval: c_uint) -> bool {
    matches!(
        keyval,
        0x4c | 0x6c | 0x6e4 | 0x6c4 | 0x0414 | 0x0434 | 0x0100_0414 | 0x0100_0434
    )
}

pub fn is_theme_toggle_key(keyval: c_uint, keycode: u16) -> bool {
    is_l_or_cyrillic_de(keyval) || keycode == KEYCODE_L
}

pub fn is_theme_toggle_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    let ctrl_shift = (state & GDK_CONTROL_MASK) != 0 && (state & GDK_SHIFT_MASK) != 0;
    let no_alt = (state & GDK_MOD1_MASK) == 0;
    ctrl_shift && no_alt && is_theme_toggle_key(keyval, keycode)
}

pub fn theme_state_path() -> PathBuf {
    state_dir().join("theme-v1")
}

pub fn encode_theme_state(mode: ThemeMode) -> String {
    format!("{THEME_STATE_HEADER}\n{}\n", mode.as_str())
}

pub fn decode_theme_state(data: &str) -> Option<ThemeMode> {
    let lines: Vec<&str> = data
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    match lines.as_slice() {
        [THEME_STATE_HEADER, value] => ThemeMode::parse_config(value),
        [value] => ThemeMode::parse_config(value),
        _ => None,
    }
}

pub fn load_theme_from_path(path: &Path) -> io::Result<Option<ThemeMode>> {
    match fs::read_to_string(path) {
        Ok(data) => decode_theme_state(&data)
            .map(Some)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid theme state")),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

pub fn save_theme_to_path(path: &Path, mode: ThemeMode) -> io::Result<()> {
    write_state_file(path, &encode_theme_state(mode))
}

pub fn resolve_theme_mode(saved: Option<ThemeMode>, config_default: Option<ThemeMode>) -> ThemeMode {
    saved.or(config_default).unwrap_or(ThemeMode::Dark)
}

pub fn load_initial_theme(config_default: Option<ThemeMode>) -> ThemeMode {
    let path = theme_state_path();
    let saved = match load_theme_from_path(&path) {
        Ok(mode) => mode,
        Err(err) => {
            eprintln!("AkhTerm: cannot read {}: {err}", path.display());
            None
        }
    };
    resolve_theme_mode(saved, config_default)
}

pub fn persist_theme_mode(mode: ThemeMode) {
    let path = theme_state_path();
    if let Err(err) = save_theme_to_path(&path, mode) {
        eprintln!("AkhTerm: cannot save {}: {err}", path.display());
    }
}

pub fn strip_define_colors(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    for line in css.lines() {
        if !line.trim_start().starts_with("@define-color ") {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

pub fn build_stylesheet(mode: ThemeMode, dark_theme_css: &str, extra_css: &str) -> String {
    match mode {
        ThemeMode::Dark => format!("{dark_theme_css}\n{extra_css}"),
        ThemeMode::Light => {
            let shared_rules = strip_define_colors(dark_theme_css);
            format!("{LIGHT_ZINC_TOKENS}\n{shared_rules}\n{extra_css}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{c_char, c_void, CString};
    use std::os::unix::fs::PermissionsExt;
    use std::ptr;

    use crate::ffi::*;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::session::create_vte_widget;
    use crate::split_pane::{wrap_vte_scroll, TabPanes};
    use crate::theme::{
        apply_gtk_theme_mode, apply_vte_list_colors, current_theme_mode, sync_theme_button,
        TERM_BG_LIGHT, TERM_FG_LIGHT, TERM_PALETTE_LIGHT,
    };

    fn extract_token_names(css: &str) -> Vec<String> {
        css.lines()
            .filter_map(|line| line.trim().strip_prefix("@define-color "))
            .filter_map(|rest| rest.split_whitespace().next())
            .map(str::to_owned)
            .collect()
    }

    fn srgb_channel_lum(chan: f64) -> f64 {
        if chan <= 0.03928 {
            chan / 12.92
        } else {
            ((chan + 0.055) / 1.055).powf(2.4)
        }
    }

    fn relative_luminance(color: &GdkRGBA) -> f64 {
        0.2126 * srgb_channel_lum(color.red)
            + 0.7152 * srgb_channel_lum(color.green)
            + 0.0722 * srgb_channel_lum(color.blue)
    }

    fn contrast_ratio(fg: &GdkRGBA, bg: &GdkRGBA) -> f64 {
        let l1 = relative_luminance(fg);
        let l2 = relative_luminance(bg);
        let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn parses_config_and_persists_theme_v1_atomically_with_0600_mode() {
        assert_eq!(ThemeMode::parse_config("dark"), Some(ThemeMode::Dark));
        assert_eq!(ThemeMode::parse_config("LIGHT"), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::parse_config("neon"), None);
        assert_eq!(ThemeMode::Dark.toggled(), ThemeMode::Light);
        assert_eq!(ThemeMode::Light.toggled(), ThemeMode::Dark);

        let dir = std::env::temp_dir().join(format!("akhterm_theme_v1_{}", std::process::id()));
        let path = dir.join("akhterm").join("theme-v1");
        let _ = fs::remove_file(&path);
        assert_eq!(load_theme_from_path(&path).unwrap(), None);

        save_theme_to_path(&path, ThemeMode::Light).unwrap();
        let mode_bits = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode_bits, 0o600);
        assert_eq!(load_theme_from_path(&path).unwrap(), Some(ThemeMode::Light));

        save_theme_to_path(&path, ThemeMode::Dark).unwrap();
        assert_eq!(load_theme_from_path(&path).unwrap(), Some(ThemeMode::Dark));
        fs::write(&path, "corrupted-theme-state\n").unwrap();
        assert!(load_theme_from_path(&path).is_err());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn saved_theme_overrides_config_and_fresh_empty_xdg_defaults_to_dark() {
        assert_eq!(resolve_theme_mode(None, None), ThemeMode::Dark);
        assert_eq!(resolve_theme_mode(None, Some(ThemeMode::Light)), ThemeMode::Light);
        assert_eq!(resolve_theme_mode(None, Some(ThemeMode::Dark)), ThemeMode::Dark);
        assert_eq!(
            resolve_theme_mode(Some(ThemeMode::Light), Some(ThemeMode::Dark)),
            ThemeMode::Light
        );
        assert_eq!(
            resolve_theme_mode(Some(ThemeMode::Dark), Some(ThemeMode::Light)),
            ThemeMode::Dark
        );
        assert_eq!(decode_theme_state("light\n"), Some(ThemeMode::Light));
        assert_eq!(decode_theme_state("akhterm-theme-v1\ndark\n"), Some(ThemeMode::Dark));
        assert_eq!(decode_theme_state(""), None);
    }

    #[test]
    fn matches_ctrl_shift_l_in_latin_cyrillic_and_keycode_46() {
        let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
        assert!(is_theme_toggle_shortcut(u32::from(b'L'), cs, 0));
        assert!(is_theme_toggle_shortcut(u32::from(b'l'), cs, 0));
        assert!(is_theme_toggle_shortcut(0x6e4, cs, 0));
        assert!(is_theme_toggle_shortcut(0x6c4, cs, 0));
        assert!(is_theme_toggle_shortcut('Д' as u32, cs, 0));
        assert!(is_theme_toggle_shortcut('д' as u32, cs, 0));
        assert!(is_theme_toggle_shortcut(0, cs, KEYCODE_L));
        assert!(!is_theme_toggle_shortcut(u32::from(b'l'), GDK_CONTROL_MASK, KEYCODE_L));
        assert!(!is_theme_toggle_shortcut(u32::from(b'L'), cs | GDK_MOD1_MASK, KEYCODE_L));
        assert!(!is_theme_toggle_shortcut(u32::from(b'T'), cs, 28));
    }

    #[test]
    fn css_tokens_have_one_to_one_parity_and_no_selector_hex_leaks() {
        let dark_css = include_str!("theme.css");
        let dark_tokens = extract_token_names(dark_css);
        let light_tokens = extract_token_names(LIGHT_ZINC_TOKENS);
        assert_eq!(dark_tokens, light_tokens);

        let rules = format!(
            "{}\n{}\n{}\n{}",
            strip_define_colors(dark_css),
            include_str!("tab_strip.css"),
            include_str!("sidebar.css"),
            include_str!("palette.css"),
        );
        for line in rules.lines() {
            let trimmed = line.trim();
            if let Some((_, rhs)) = trimmed.split_once(':') {
                assert!(!rhs.contains('#'), "un-tokenized hex literal in CSS rule: {trimmed}");
                assert!(!rhs.contains("rgba("), "un-tokenized rgba literal in CSS rule: {trimmed}");
                assert!(!rhs.contains("rgb("), "un-tokenized rgb literal in CSS rule: {trimmed}");
                assert!(!rhs.contains("hsl("), "un-tokenized hsl literal in CSS rule: {trimmed}");
                assert!(!rhs.contains("hsla("), "un-tokenized hsla literal in CSS rule: {trimmed}");
            }
        }
    }

    #[test]
    fn light_vte_palette_meets_wcag_aa_contrast_on_white_background() {
        assert!(contrast_ratio(&TERM_FG_LIGHT, &TERM_BG_LIGHT) >= 14.0);
        for idx in [0usize, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14] {
            let ratio = contrast_ratio(&TERM_PALETTE_LIGHT[idx], &TERM_BG_LIGHT);
            assert!(ratio >= 4.5, "ANSI index {idx} contrast {ratio:.2} < 4.5 on Light VTE bg");
        }
    }

    fn read_prefer_dark_setting() -> c_int {
        let mut val: c_int = -1;
        unsafe {
            let settings = gtk_settings_get_default();
            g_object_get(
                settings,
                c"gtk-application-prefer-dark-theme".as_ptr(),
                &mut val as *mut c_int,
                ptr::null::<c_char>(),
            );
        }
        val
    }

    fn lookup_named_rgba(widget: *mut GtkWidget, name: &str) -> GdkRGBA {
        let name_c = CString::new(name).unwrap();
        let mut out = GdkRGBA::rgb(0, 0, 0);
        let found = unsafe {
            let ctx = gtk_widget_get_style_context(widget);
            gtk_style_context_lookup_color(ctx, name_c.as_ptr(), &mut out)
        };
        assert_ne!(found, 0, "missing GTK CSS @define-color token: {name}");
        out
    }

    fn assert_rgb_close(actual: GdkRGBA, r: u8, g: u8, b: u8) {
        let expected = GdkRGBA::rgb(r, g, b);
        assert!(
            (actual.red - expected.red).abs() < 0.015
                && (actual.green - expected.green).abs() < 0.015
                && (actual.blue - expected.blue).abs() < 0.015,
            "rgba mismatch: actual=({:.3}, {:.3}, {:.3}, {:.3}), expected=#{r:02x}{g:02x}{b:02x}",
            actual.red,
            actual.green,
            actual.blue,
            actual.alpha,
        );
    }

    fn build_theme_test_window() -> (
        *mut GtkWidget,
        *mut GtkWidget,
        *mut GtkWidget,
        TabPanes,
        *mut GtkWidget,
    ) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            add_class(win, "main-bg");
            let header = gtk_header_bar_new();
            add_class(header, "window-header");
            gtk_window_set_titlebar(win, header);
            let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
            let topbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
            add_class(topbar, "topbar");
            let theme_btn = gtk_button_new_with_label(ThemeMode::Dark.toggle_icon().as_ptr());
            gtk_box_pack_start(topbar, theme_btn, 0, 0, 0);
            let (vte1, _) = create_vte_widget("/tmp", &["/bin/true"]);
            let scroll1 = wrap_vte_scroll(vte1);
            let mut panes = TabPanes::new(scroll1, vte1);
            let (vte2, _) = create_vte_widget("/tmp", &["/bin/true"]);
            panes.split(scroll1, vte1, vte2, 0, "/tmp".to_owned(), GTK_ORIENTATION_HORIZONTAL);
            gtk_box_pack_start(vbox, topbar, 0, 1, 0);
            gtk_box_pack_start(vbox, panes.root, 1, 1, 0);
            gtk_container_add(win, vbox);
            gtk_widget_show_all(win);
            (win, vbox, theme_btn, panes, vte1)
        }
    }

    fn read_button_label(button: *mut GtkWidget) -> String {
        unsafe {
            let ptr = gtk_button_get_label(button);
            assert!(!ptr.is_null());
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }

    #[repr(C)]
    struct CssGError {
        domain: u32,
        code: c_int,
        message: *const c_char,
    }

    extern "C" {
        fn gtk_css_section_get_start_line(section: *const c_void) -> c_uint;
        fn gtk_style_context_get_color(
            context: *mut GtkStyleContext,
            state: c_uint,
            color: *mut GdkRGBA,
        );
        fn gtk_style_context_set_state(context: *mut GtkStyleContext, flags: c_uint);
        fn gtk_style_context_get(context: *mut GtkStyleContext, state: c_uint, ...);
        fn gtk_style_context_remove_provider_for_screen(
            screen: *mut GdkScreen,
            provider: *mut GtkCssProvider,
        );
        fn gtk_header_bar_pack_end(bar: *mut GtkHeaderBar, child: *mut GtkWidget);
        fn gtk_check_button_new_with_label(label: *const c_char) -> *mut GtkWidget;
    }

    const STATE_NORMAL: c_uint = 0;
    const STATE_PRELIGHT: c_uint = 1 << 1;
    const STATE_BACKDROP: c_uint = 1 << 6;
    const PRIORITY_THEME: c_uint = 200;
    const PRIORITY_USER: c_uint = 800;

    const BREEZE_SIM_CSS: &str = "\
        headerbar button.titlebutton {\
            background-image: linear-gradient(to bottom, #eff0f1, #bdc3c7);\
        }\
        headerbar button.titlebutton image,\
        headerbar button.titlebutton.close image,\
        headerbar button.titlebutton.maximize image,\
        headerbar button.titlebutton.minimize image {\
            color: transparent;\
        }\
        entry, entry.search, scrollbar trough, popover.background, menu, menu menuitem, checkbutton, checkbutton check {\
            background-image: linear-gradient(to bottom, #3daee9, #2980b9);\
            color: #ff00ff;\
        }\
        window headerbar, headerbar.window-header {\
            border-radius: 8px;\
        }";

    extern "C" fn record_css_parse_error(
        _prov: *mut GtkCssProvider,
        section: *const c_void,
        gerr: *const CssGError,
        data: *mut c_void,
    ) {
        let out = unsafe { &mut *(data as *mut Vec<String>) };
        let line = if section.is_null() {
            0
        } else {
            unsafe { gtk_css_section_get_start_line(section) + 1 }
        };
        let msg = if gerr.is_null() || unsafe { (*gerr).message.is_null() } {
            "unknown css error".to_owned()
        } else {
            unsafe { CStr::from_ptr((*gerr).message).to_string_lossy().into_owned() }
        };
        out.push(format!("line {line}: {msg}"));
    }

    fn full_stylesheet(mode: ThemeMode) -> String {
        let extra = concat!(
            include_str!("tab_strip.css"),
            include_str!("sidebar.css"),
            include_str!("palette.css"),
        );
        build_stylesheet(mode, include_str!("theme.css"), extra)
    }

    fn collect_css_parse_errors(css_text: &str) -> (c_int, Vec<String>) {
        let css = CString::new(css_text).unwrap();
        let mut errors: Vec<String> = Vec::new();
        let ok = unsafe {
            let prov = gtk_css_provider_new();
            g_signal_connect_data(
                prov,
                c"parsing-error".as_ptr(),
                record_css_parse_error as *const c_void,
                (&mut errors as *mut Vec<String>).cast::<c_void>(),
                ptr::null(),
                0,
            );
            let res = gtk_css_provider_load_from_data(prov, css.as_ptr(), -1, ptr::null_mut());
            g_object_unref(prov);
            res
        };
        (ok, errors)
    }

    fn verify_css_loads_without_gtk_error(mode: ThemeMode) {
        let raw = full_stylesheet(mode);
        let (ok_sig, sig_errors) = collect_css_parse_errors(&raw);
        assert_ne!(ok_sig, 0, "GTK CSS signal load failed for {mode:?}");
        assert!(sig_errors.is_empty(), "GTK CSS parsing-error for {mode:?}: {sig_errors:?}");

        let css = CString::new(raw).unwrap();
        unsafe {
            let prov = gtk_css_provider_new();
            let mut err: *mut c_void = ptr::null_mut();
            let ok = gtk_css_provider_load_from_data(prov, css.as_ptr(), -1, &mut err);
            assert_ne!(ok, 0, "GTK CSS failed to parse for {mode:?}");
            assert!(err.is_null(), "GTK CSS returned GError for {mode:?}");
            g_object_unref(prov);
        }
    }

    fn verify_dark_and_light_live_switch(
        win: *mut GtkWidget,
        vbox: *mut GtkWidget,
        theme_btn: *mut GtkWidget,
        panes: &TabPanes,
        vte1: *mut GtkWidget,
    ) {
        verify_css_loads_without_gtk_error(ThemeMode::Dark);
        verify_css_loads_without_gtk_error(ThemeMode::Light);

        let mut vtes = Vec::new();
        panes.collect_vtes(vte1, &mut vtes);
        assert_eq!(vtes.len(), 2);

        apply_gtk_theme_mode(ThemeMode::Dark);
        apply_vte_list_colors(&vtes, ThemeMode::Dark);
        sync_theme_button(theme_btn, ThemeMode::Dark);
        pump_gtk_events();
        assert_eq!(current_theme_mode(), ThemeMode::Dark);
        assert_eq!(read_prefer_dark_setting(), 1);
        assert_eq!(read_button_label(theme_btn), "☀");
        assert_rgb_close(lookup_named_rgba(win, "canvas"), 0x09, 0x09, 0x0b);
        assert_rgb_close(lookup_named_rgba(win, "text"), 0xfa, 0xfa, 0xfa);

        apply_gtk_theme_mode(ThemeMode::Light);
        apply_vte_list_colors(&vtes, ThemeMode::Light);
        sync_theme_button(theme_btn, ThemeMode::Light);
        pump_gtk_events();
        assert_eq!(current_theme_mode(), ThemeMode::Light);
        assert_eq!(read_prefer_dark_setting(), 0);
        assert_eq!(read_button_label(theme_btn), "☾");
        assert_rgb_close(lookup_named_rgba(win, "canvas"), 0xff, 0xff, 0xff);
        assert_rgb_close(lookup_named_rgba(win, "surface"), 0xf4, 0xf4, 0xf5);
        assert_rgb_close(lookup_named_rgba(win, "hover"), 0xec, 0xec, 0xef);
        assert_rgb_close(lookup_named_rgba(win, "selected"), 0xe4, 0xe4, 0xe7);
        assert_rgb_close(lookup_named_rgba(win, "text"), 0x18, 0x18, 0x1b);
        assert_rgb_close(lookup_named_rgba(win, "muted"), 0x52, 0x52, 0x5b);
        assert_rgb_close(lookup_named_rgba(win, "agent_agy_fg"), 0x5b, 0x21, 0xb6);
        assert_rgb_close(lookup_named_rgba(win, "ready"), 0x04, 0x78, 0x57);

        let (vte3, _) = create_vte_widget("/tmp", &["/bin/true"]);
        unsafe {
            gtk_box_pack_start(vbox, vte3, 0, 0, 0);
            gtk_widget_show_all(vte3);
        }
        vtes.push(vte3);

        apply_gtk_theme_mode(ThemeMode::Dark);
        apply_vte_list_colors(&vtes, ThemeMode::Dark);
        sync_theme_button(theme_btn, ThemeMode::Dark);
        pump_gtk_events();
        assert_eq!(read_prefer_dark_setting(), 1);
        assert_eq!(read_button_label(theme_btn), "☀");
    }

    fn style_fg_rgba(widget: *mut GtkWidget, state: c_uint) -> GdkRGBA {
        let mut out = GdkRGBA::rgb(0, 0, 0);
        unsafe {
            let parent = gtk_widget_get_parent(widget);
            if !parent.is_null() {
                gtk_style_context_set_state(gtk_widget_get_style_context(parent), state);
            }
            let ctx = gtk_widget_get_style_context(widget);
            gtk_style_context_set_state(ctx, state);
            gtk_style_context_get_color(ctx, state, &mut out);
            gtk_style_context_set_state(ctx, STATE_NORMAL);
            if !parent.is_null() {
                gtk_style_context_set_state(gtk_widget_get_style_context(parent), STATE_NORMAL);
            }
        }
        out
    }

    fn style_border_radius(widget: *mut GtkWidget) -> c_int {
        let mut radius: c_int = -1;
        unsafe {
            let ctx = gtk_widget_get_style_context(widget);
            gtk_style_context_get(
                ctx,
                STATE_NORMAL,
                c"border-radius".as_ptr(),
                &mut radius as *mut c_int,
                ptr::null::<c_char>(),
            );
        }
        radius
    }

    fn attach_css_at_priority(css_text: &str, priority: c_uint) -> *mut GtkCssProvider {
        let css = CString::new(css_text).unwrap();
        unsafe {
            let prov = gtk_css_provider_new();
            gtk_css_provider_load_from_data(prov, css.as_ptr(), -1, ptr::null_mut());
            gtk_style_context_add_provider_for_screen(gdk_screen_get_default(), prov, priority);
            prov
        }
    }

    fn detach_css_provider(prov: *mut GtkCssProvider) {
        unsafe {
            gtk_style_context_remove_provider_for_screen(gdk_screen_get_default(), prov);
            g_object_unref(prov);
        }
    }

    fn make_titlebutton(header: *mut GtkWidget, role_cls: &str, icon_name: &CStr) -> *mut GtkWidget {
        unsafe {
            let btn = gtk_button_new();
            add_class(btn, "titlebutton");
            add_class(btn, role_cls);
            let img = gtk_image_new_from_icon_name(icon_name.as_ptr(), GTK_ICON_SIZE_MENU);
            gtk_container_add(btn, img);
            gtk_header_bar_pack_end(header, btn);
            img
        }
    }

    fn assert_opaque_rgb(actual: GdkRGBA, r: u8, g: u8, b: u8) {
        assert!(actual.alpha > 0.99, "expected opaque color, got alpha={:.3}", actual.alpha);
        assert_rgb_close(actual, r, g, b);
    }

    fn verify_wayland_tiled_and_maximized_zero_radius(win: *mut GtkWidget, header: *mut GtkWidget) {
        assert_eq!(style_border_radius(header), 8);
        let states = [
            "maximized",
            "fullscreen",
            "tiled",
            "tiled-top",
            "tiled-bottom",
            "tiled-left",
            "tiled-right",
            "solid-csd",
        ];
        for cls in states {
            add_class(win, cls);
            assert_eq!(style_border_radius(win), 0, "window.{cls} border-radius != 0");
            assert_eq!(style_border_radius(header), 0, "window.{cls} headerbar border-radius != 0");
            remove_class(win, cls);
        }
        assert_eq!(style_border_radius(header), 8);
    }

    fn verify_mode_breeze_colors(
        mode: ThemeMode,
        imgs: [*mut GtkWidget; 3],
        widgets: [*mut GtkWidget; 3],
        fg: [u8; 3],
        danger: [u8; 3],
        muted: [u8; 3],
    ) {
        let [min_img, max_img, close_img] = imgs;
        apply_gtk_theme_mode(mode);
        pump_gtk_events();
        for img in [min_img, max_img, close_img] {
            assert_opaque_rgb(style_fg_rgba(img, STATE_NORMAL), fg[0], fg[1], fg[2]);
        }
        assert_opaque_rgb(style_fg_rgba(close_img, STATE_PRELIGHT), danger[0], danger[1], danger[2]);
        assert_opaque_rgb(style_fg_rgba(min_img, STATE_BACKDROP), muted[0], muted[1], muted[2]);
        for w in widgets {
            assert_opaque_rgb(style_fg_rgba(w, STATE_NORMAL), fg[0], fg[1], fg[2]);
        }
        verify_fresh_menu_item_fg(fg[0], fg[1], fg[2]);
    }

    fn verify_fresh_menu_item_fg(r: u8, g: u8, b: u8) {
        unsafe {
            let menu = gtk_menu_new();
            let item = gtk_menu_item_new_with_label(c"Item".as_ptr());
            gtk_menu_shell_append(menu, item);
            gtk_widget_show_all(menu);
            assert_opaque_rgb(style_fg_rgba(item, STATE_NORMAL), r, g, b);
            gtk_widget_destroy(menu);
        }
    }

    fn build_breeze_test_widgets() -> (
        *mut GtkWidget,
        *mut GtkWidget,
        *mut GtkWidget,
        [*mut GtkWidget; 3],
        [*mut GtkWidget; 3],
    ) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            add_class(win, "main-bg");
            let header = gtk_header_bar_new();
            add_class(header, "window-header");
            gtk_header_bar_set_show_close_button(header, 1);
            let min_img = make_titlebutton(header, "minimize", c"window-minimize-symbolic");
            let max_img = make_titlebutton(header, "maximize", c"window-maximize-symbolic");
            let close_img = make_titlebutton(header, "close", c"window-close-symbolic");
            gtk_window_set_titlebar(win, header);

            let box_root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
            let search = gtk_search_entry_new();
            let check = gtk_check_button_new_with_label(c"Check".as_ptr());
            let sw = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            gtk_box_pack_start(box_root, search, 0, 0, 0);
            gtk_box_pack_start(box_root, check, 0, 0, 0);
            gtk_box_pack_start(box_root, sw, 1, 1, 0);
            gtk_container_add(win, box_root);
            let pop = gtk_popover_new(search);
            gtk_widget_show_all(win);
            gtk_popover_popup(pop);
            pump_gtk_events();
            (win, header, pop, [min_img, max_img, close_img], [search, check, pop])
        }
    }

    #[test]
    fn gtk_css_provider_emits_zero_parsing_errors_for_dark_and_light_stylesheets() {
        with_headless_gtk(|| {
            let (_, bad_errs) = collect_css_parse_errors("button { unknown-breeze-prop: 1px; }");
            assert_eq!(bad_errs.len(), 1, "parsing-error signal hookup must catch bad properties");
            verify_css_loads_without_gtk_error(ThemeMode::Dark);
            verify_css_loads_without_gtk_error(ThemeMode::Light);
        });
    }

    #[test]
    fn breeze_gtk_titlebuttons_widgets_and_wayland_tiling_render_cleanly() {
        with_headless_gtk(|| {
            apply_gtk_theme_mode(ThemeMode::Dark);
            let p_theme = attach_css_at_priority(BREEZE_SIM_CSS, PRIORITY_THEME);
            let p_user = attach_css_at_priority(BREEZE_SIM_CSS, PRIORITY_USER);
            let (win, header, pop, imgs, widgets) = build_breeze_test_widgets();
            verify_mode_breeze_colors(
                ThemeMode::Dark,
                imgs,
                widgets,
                [0xfa, 0xfa, 0xfa],
                [0xfd, 0xa4, 0xaf],
                [0xa1, 0xa1, 0xaa],
            );
            verify_mode_breeze_colors(
                ThemeMode::Light,
                imgs,
                widgets,
                [0x18, 0x18, 0x1b],
                [0xbe, 0x12, 0x3c],
                [0x52, 0x52, 0x5b],
            );
            verify_wayland_tiled_and_maximized_zero_radius(win, header);
            unsafe {
                gtk_widget_destroy(pop);
                gtk_widget_destroy(win);
            }
            detach_css_provider(p_user);
            detach_css_provider(p_theme);
            apply_gtk_theme_mode(ThemeMode::Dark);
            pump_gtk_events();
        });
    }

    #[test]
    fn headless_broadway_live_theme_toggle_updates_header_css_and_split_vte_panes() {
        with_headless_gtk(|| {
            apply_gtk_theme_mode(ThemeMode::Dark);
            let (win, vbox, theme_btn, panes, vte1) = build_theme_test_window();
            pump_gtk_events();
            verify_dark_and_light_live_switch(win, vbox, theme_btn, &panes, vte1);
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
        });
    }

    #[test]
    fn theme_modules_maintain_target_mi_scores() {
        let (_, _, mode_mi) = crate::mi::compute_file_mi(include_str!("theme_mode.rs"));
        assert!(mode_mi >= 60.0, "theme_mode.rs MI {mode_mi:.2} < 60.0");
        let (_, _, theme_mi) = crate::mi::compute_file_mi(include_str!("theme.rs"));
        assert!(theme_mi >= 58.0, "theme.rs MI {theme_mi:.2} < 58.0");
        let (_, _, main_mi) = crate::mi::compute_file_mi(include_str!("main.rs"));
        assert!(main_mi >= 54.87, "main.rs MI {main_mi:.2} < 54.87");
    }
}
