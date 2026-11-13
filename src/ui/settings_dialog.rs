//! Live Settings modal dialog (`Ctrl+,`) for AkhTerm v1.0.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::PathBuf;
use std::rc::Rc;

use crate::config::{
    MAX_NOTIFY_SECS, MAX_SCROLLBACK_LINES, MIN_SCROLLBACK_LINES, UserPrefs,
    normalize_prefs, save_user_preferences,
};
use crate::ffi::{
    GDK_CONTROL_MASK, GtkWidget, add_class, g_signal_connect_data, set_label_text,
};
use crate::i18n::{UiLocale, set_locale};
use crate::paste_route::is_plain_ctrl_shortcut;
use crate::theme_mode::ThemeMode;
use crate::ui_text;

#[repr(C)]
struct GdkEventKey {
    kind: c_int,
    window: *mut c_void,
    send_event: i8,
    time: c_uint,
    state: c_uint,
    keyval: c_uint,
}

const GTK_WINDOW_TOPLEVEL: c_int = 0;
const GTK_ORIENTATION_HORIZONTAL: c_int = 0;
const GTK_ORIENTATION_VERTICAL: c_int = 1;
const GTK_ALIGN_START: c_int = 1;
const GTK_ALIGN_CENTER: c_int = 3;
const GTK_WIN_POS_CENTER_ON_PARENT: c_int = 4;
const GDK_KEY_COMMA: c_uint = 0x002c;
const GDK_KEY_ESCAPE: c_uint = 0xff1b;
const KEYCODE_COMMA: u16 = 59;

pub type OnPrefsChanged = Box<dyn Fn(&UserPrefs)>;

pub const FONT_PRESETS: &[(&str, &str)] = &[
    ("DejaVu Sans Mono", "DejaVu Sans Mono 10.5"),
    ("JetBrains Mono", "JetBrains Mono 10.5"),
    ("IBM Plex Mono", "IBM Plex Mono 11"),
    ("Fira Code", "Fira Code 10.5"),
];

#[link(name = "gtk-3")]
unsafe extern "C" {
    fn gtk_window_new(kind: c_int) -> *mut GtkWidget;
    fn gtk_window_set_title(win: *mut GtkWidget, title: *const c_char);
    fn gtk_window_set_modal(win: *mut GtkWidget, modal: c_int);
    fn gtk_window_set_transient_for(win: *mut GtkWidget, parent: *mut GtkWidget);
    fn gtk_window_set_destroy_with_parent(win: *mut GtkWidget, setting: c_int);
    fn gtk_window_set_position(win: *mut GtkWidget, pos: c_int);
    fn gtk_window_set_default_size(win: *mut GtkWidget, w: c_int, h: c_int);
    fn gtk_box_new(orientation: c_int, spacing: c_int) -> *mut GtkWidget;
    fn gtk_box_pack_start(
        bx: *mut GtkWidget,
        child: *mut GtkWidget,
        expand: c_int,
        fill: c_int,
        pad: c_uint,
    );
    fn gtk_grid_new() -> *mut GtkWidget;
    fn gtk_grid_set_row_spacing(grid: *mut GtkWidget, spacing: c_uint);
    fn gtk_grid_set_column_spacing(grid: *mut GtkWidget, spacing: c_uint);
    fn gtk_grid_attach(
        grid: *mut GtkWidget,
        child: *mut GtkWidget,
        left: c_int,
        top: c_int,
        width: c_int,
        height: c_int,
    );
    fn gtk_container_add(container: *mut GtkWidget, widget: *mut GtkWidget);
    fn gtk_container_set_border_width(container: *mut GtkWidget, border_width: c_uint);
    fn gtk_label_new(text: *const c_char) -> *mut GtkWidget;
    fn gtk_label_set_line_wrap(label: *mut GtkWidget, wrap: c_int);
    fn gtk_entry_new() -> *mut GtkWidget;
    fn gtk_entry_set_text(entry: *mut GtkWidget, text: *const c_char);
    fn gtk_entry_get_text(entry: *mut GtkWidget) -> *const c_char;
    fn gtk_entry_set_placeholder_text(entry: *mut GtkWidget, text: *const c_char);
    fn gtk_combo_box_text_new() -> *mut GtkWidget;
    fn gtk_combo_box_text_append(
        combo: *mut GtkWidget,
        id: *const c_char,
        text: *const c_char,
    );
    fn gtk_combo_box_text_remove_all(combo: *mut GtkWidget);
    fn gtk_combo_box_set_active_id(combo: *mut GtkWidget, id: *const c_char) -> c_int;
    fn gtk_combo_box_get_active_id(combo: *mut GtkWidget) -> *const c_char;
    fn gtk_spin_button_new_with_range(min: f64, max: f64, step: f64) -> *mut GtkWidget;
    fn gtk_spin_button_set_value(spin: *mut GtkWidget, value: f64);
    fn gtk_spin_button_get_value_as_int(spin: *mut GtkWidget) -> c_int;
    fn gtk_switch_new() -> *mut GtkWidget;
    fn gtk_switch_set_active(sw: *mut GtkWidget, active: c_int);
    fn gtk_switch_get_active(sw: *mut GtkWidget) -> c_int;
    fn gtk_button_new_with_label(label: *const c_char) -> *mut GtkWidget;
    fn gtk_button_set_label(button: *mut GtkWidget, label: *const c_char);
    fn gtk_widget_set_hexpand(widget: *mut GtkWidget, expand: c_int);
    fn gtk_widget_set_halign(widget: *mut GtkWidget, align: c_int);
    fn gtk_widget_set_valign(widget: *mut GtkWidget, align: c_int);
    fn gtk_widget_show_all(widget: *mut GtkWidget);
    fn gtk_widget_destroy(widget: *mut GtkWidget);
}

struct SettingsWidgets {
    window: *mut GtkWidget,
    title_lbl: *mut GtkWidget,
    subtitle_lbl: *mut GtkWidget,
    theme_lbl: *mut GtkWidget,
    theme_combo: *mut GtkWidget,
    locale_lbl: *mut GtkWidget,
    locale_combo: *mut GtkWidget,
    font_lbl: *mut GtkWidget,
    font_entry: *mut GtkWidget,
    scrollback_lbl: *mut GtkWidget,
    scrollback_spin: *mut GtkWidget,
    osc52_lbl: *mut GtkWidget,
    osc52_switch: *mut GtkWidget,
    osc52_hint: *mut GtkWidget,
    notify_lbl: *mut GtkWidget,
    notify_spin: *mut GtkWidget,
    notify_hint: *mut GtkWidget,
    about_btn: *mut GtkWidget,
    close_btn: *mut GtkWidget,
}

pub(crate) struct SettingsState {
    widgets: SettingsWidgets,
    config_path: PathBuf,
    on_change: OnPrefsChanged,
    updating: Cell<bool>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct SettingsDialog {
    state: Rc<SettingsState>,
}

#[allow(dead_code)]
impl SettingsDialog {
    pub fn window(&self) -> *mut GtkWidget {
        self.state.widgets.window
    }

    pub fn theme_combo(&self) -> *mut GtkWidget {
        self.state.widgets.theme_combo
    }

    pub fn locale_combo(&self) -> *mut GtkWidget {
        self.state.widgets.locale_combo
    }

    pub fn font_entry(&self) -> *mut GtkWidget {
        self.state.widgets.font_entry
    }

    pub fn scrollback_spin(&self) -> *mut GtkWidget {
        self.state.widgets.scrollback_spin
    }

    pub fn osc52_switch(&self) -> *mut GtkWidget {
        self.state.widgets.osc52_switch
    }

    pub fn notify_spin(&self) -> *mut GtkWidget {
        self.state.widgets.notify_spin
    }

    pub fn about_button(&self) -> *mut GtkWidget {
        self.state.widgets.about_btn
    }

    pub fn close_button(&self) -> *mut GtkWidget {
        self.state.widgets.close_btn
    }

    pub fn current_prefs(&self) -> UserPrefs {
        read_prefs_from_widgets(&self.state.widgets)
    }
}

#[allow(dead_code)]
pub fn is_settings_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    let comma_key = keyval == GDK_KEY_COMMA || keycode == KEYCODE_COMMA;
    is_plain_ctrl_shortcut(state) && comma_key
}

#[allow(dead_code)]
pub fn open_settings_dialog(
    parent: *mut GtkWidget,
    initial: UserPrefs,
    config_path: PathBuf,
    on_change: OnPrefsChanged,
) -> SettingsDialog {
    let prefs = normalize_prefs(&initial);
    let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
    configure_settings_window(win, parent);
    let (vbox, widgets) = build_settings_layout(win, &prefs);
    unsafe { gtk_container_add(win, vbox) };
    let state = Rc::new(SettingsState {
        widgets,
        config_path,
        on_change,
        updating: Cell::new(false),
    });
    populate_initial_values(&state, &prefs);
    connect_settings_signals(&state);
    unsafe { gtk_widget_show_all(win) };
    SettingsDialog { state }
}

fn set_parent_modal(win: *mut GtkWidget, parent: *mut GtkWidget) {
    if parent.is_null() {
        return;
    }
    unsafe {
        gtk_window_set_transient_for(win, parent);
        gtk_window_set_modal(win, 1);
        gtk_window_set_destroy_with_parent(win, 1);
        gtk_window_set_position(win, GTK_WIN_POS_CENTER_ON_PARENT);
    }
}

fn configure_settings_window(win: *mut GtkWidget, parent: *mut GtkWidget) {
    unsafe {
        gtk_window_set_title(win, ui_text::settings_title().as_ptr());
        gtk_window_set_default_size(win, 540, 420);
        gtk_container_set_border_width(win, 18);
    }
    add_class(win, "settings-dialog");
    add_class(win, "main-bg");
    set_parent_modal(win, parent);
}

fn build_settings_layout(
    win: *mut GtkWidget,
    prefs: &UserPrefs,
) -> (*mut GtkWidget, SettingsWidgets) {
    let vbox = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 12) };
    let (header, title_lbl, subtitle_lbl) = build_settings_header();
    let grid = create_form_grid();
    let (about_btn, close_btn, footer) = build_settings_footer();
    let widgets = fill_settings_grid(grid, win, title_lbl, subtitle_lbl, about_btn, close_btn, prefs);
    unsafe {
        gtk_box_pack_start(vbox, header, 0, 0, 2);
        gtk_box_pack_start(vbox, grid, 1, 1, 4);
        gtk_box_pack_start(vbox, footer, 0, 0, 4);
    }
    (vbox, widgets)
}

fn fill_settings_grid(
    grid: *mut GtkWidget,
    window: *mut GtkWidget,
    title_lbl: *mut GtkWidget,
    subtitle_lbl: *mut GtkWidget,
    about_btn: *mut GtkWidget,
    close_btn: *mut GtkWidget,
    prefs: &UserPrefs,
) -> SettingsWidgets {
    let (theme_lbl, theme_combo) = add_combo_row(grid, 0, ui_text::settings_theme());
    let (locale_lbl, locale_combo) = add_combo_row(grid, 1, ui_text::settings_locale());
    let (font_lbl, font_entry) = add_font_row(grid, 2, &prefs.font_desc);
    let (scrollback_lbl, scrollback_spin) = add_scrollback_row(grid, 3, prefs.scrollback_lines);
    let (osc52_lbl, osc52_switch, osc52_hint) = add_osc52_row(grid, 4);
    let (notify_lbl, notify_spin, notify_hint) = add_notify_row(grid, 5, prefs.notify_secs);
    SettingsWidgets {
        window, title_lbl, subtitle_lbl, theme_lbl, theme_combo, locale_lbl, locale_combo,
        font_lbl, font_entry, scrollback_lbl, scrollback_spin, osc52_lbl, osc52_switch,
        osc52_hint, notify_lbl, notify_spin, notify_hint, about_btn, close_btn,
    }
}

fn add_combo_row(grid: *mut GtkWidget, row: c_int, title: &CStr) -> (*mut GtkWidget, *mut GtkWidget) {
    let (lbl, ctl) = build_combo_row(title);
    attach_grid_row(grid, row, lbl, ctl);
    (lbl, ctl)
}

fn add_font_row(grid: *mut GtkWidget, row: c_int, font_desc: &str) -> (*mut GtkWidget, *mut GtkWidget) {
    let (lbl, bx, entry) = build_font_row(font_desc);
    attach_grid_row(grid, row, lbl, bx);
    (lbl, entry)
}

fn add_scrollback_row(grid: *mut GtkWidget, row: c_int, lines: usize) -> (*mut GtkWidget, *mut GtkWidget) {
    let (lbl, spin) = build_scrollback_row(lines);
    attach_grid_row(grid, row, lbl, spin);
    (lbl, spin)
}

fn add_osc52_row(grid: *mut GtkWidget, row: c_int) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let (lbl, bx, sw, hint) = build_osc52_row();
    attach_grid_row(grid, row, lbl, bx);
    (lbl, sw, hint)
}

fn add_notify_row(grid: *mut GtkWidget, row: c_int, secs: u64) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let (lbl, bx, spin, hint) = build_notify_row(secs);
    attach_grid_row(grid, row, lbl, bx);
    (lbl, spin, hint)
}

fn build_settings_header() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let bx = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 4) };
    let title = make_left_label(ui_text::settings_title(), "settings-title");
    let sub = make_left_label(ui_text::settings_subtitle(), "settings-subtitle");
    unsafe {
        gtk_label_set_line_wrap(sub, 1);
        gtk_box_pack_start(bx, title, 0, 0, 0);
        gtk_box_pack_start(bx, sub, 0, 0, 0);
    }
    (bx, title, sub)
}

fn create_form_grid() -> *mut GtkWidget {
    let grid = unsafe { gtk_grid_new() };
    unsafe {
        gtk_grid_set_row_spacing(grid, 10);
        gtk_grid_set_column_spacing(grid, 14);
        add_class(grid, "settings-grid");
    }
    grid
}

fn attach_grid_row(grid: *mut GtkWidget, row: c_int, lbl: *mut GtkWidget, ctl: *mut GtkWidget) {
    unsafe {
        gtk_grid_attach(grid, lbl, 0, row, 1, 1);
        gtk_grid_attach(grid, ctl, 1, row, 1, 1);
    }
}

fn make_left_label(text: &CStr, class_name: &str) -> *mut GtkWidget {
    let lbl = unsafe { gtk_label_new(text.as_ptr()) };
    unsafe {
        gtk_widget_set_halign(lbl, GTK_ALIGN_START);
        add_class(lbl, class_name);
    }
    lbl
}

fn build_combo_row(title: &CStr) -> (*mut GtkWidget, *mut GtkWidget) {
    let lbl = make_left_label(title, "settings-key");
    let combo = unsafe { gtk_combo_box_text_new() };
    unsafe {
        gtk_widget_set_hexpand(combo, 1);
        add_class(combo, "settings-control");
    }
    (lbl, combo)
}

fn build_font_row(_font_desc: &str) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let lbl = make_left_label(ui_text::settings_font(), "settings-key");
    unsafe { gtk_widget_set_valign(lbl, GTK_ALIGN_START) };
    let container = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 6) };
    let entry = unsafe { gtk_entry_new() };
    unsafe {
        gtk_widget_set_hexpand(entry, 1);
        gtk_entry_set_placeholder_text(entry, ui_text::settings_font_placeholder().as_ptr());
        add_class(entry, "settings-control");
        gtk_box_pack_start(container, entry, 0, 0, 0);
    }
    let presets = build_font_presets_row(entry);
    unsafe { gtk_box_pack_start(container, presets, 0, 0, 0) };
    (lbl, container, entry)
}

fn build_font_presets_row(entry: *mut GtkWidget) -> *mut GtkWidget {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6) };
    for (idx, (short_label, _full_desc)) in FONT_PRESETS.iter().enumerate() {
        let btn = make_preset_button(short_label, entry, idx);
        unsafe { gtk_box_pack_start(row, btn, 0, 0, 0) };
    }
    row
}

fn make_preset_button(label: &str, entry: *mut GtkWidget, idx: usize) -> *mut GtkWidget {
    let c_lbl = CString::new(label).unwrap_or_default();
    let btn = unsafe { gtk_button_new_with_label(c_lbl.as_ptr()) };
    add_class(btn, "settings-preset-btn");
    let payload = Box::into_raw(Box::new((entry, idx))) as *mut c_void;
    unsafe {
        g_signal_connect_data(
            btn as *mut c_void,
            c"clicked".as_ptr(),
            on_font_preset_clicked as *const c_void,
            payload,
            drop_preset_payload as *const c_void,
            0,
        );
    }
    btn
}

fn build_scrollback_row(lines: usize) -> (*mut GtkWidget, *mut GtkWidget) {
    let lbl = make_left_label(ui_text::settings_scrollback(), "settings-key");
    let spin = unsafe {
        gtk_spin_button_new_with_range(
            MIN_SCROLLBACK_LINES as f64,
            MAX_SCROLLBACK_LINES as f64,
            500.0,
        )
    };
    unsafe {
        gtk_spin_button_set_value(spin, lines as f64);
        gtk_widget_set_hexpand(spin, 1);
        add_class(spin, "settings-control");
    }
    (lbl, spin)
}

fn build_osc52_row() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let lbl = make_left_label(ui_text::settings_osc52(), "settings-key");
    let bx = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10) };
    let sw = unsafe { gtk_switch_new() };
    let hint = make_left_label(ui_text::settings_osc52_hint(), "settings-hint");
    unsafe {
        gtk_widget_set_valign(sw, GTK_ALIGN_CENTER);
        gtk_label_set_line_wrap(hint, 1);
        gtk_box_pack_start(bx, sw, 0, 0, 0);
        gtk_box_pack_start(bx, hint, 1, 1, 0);
    }
    (lbl, bx, sw, hint)
}

fn build_notify_row(secs: u64) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let lbl = make_left_label(ui_text::settings_notify_secs(), "settings-key");
    let bx = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 4) };
    let spin = unsafe { gtk_spin_button_new_with_range(0.0, MAX_NOTIFY_SECS as f64, 5.0) };
    let hint = make_left_label(ui_text::settings_notify_hint(), "settings-hint");
    unsafe {
        gtk_spin_button_set_value(spin, secs as f64);
        gtk_widget_set_hexpand(spin, 1);
        add_class(spin, "settings-control");
        gtk_box_pack_start(bx, spin, 0, 0, 0);
        gtk_box_pack_start(bx, hint, 0, 0, 0);
    }
    (lbl, bx, spin, hint)
}

fn build_settings_footer() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let about = unsafe { gtk_button_new_with_label(ui_text::settings_about_btn().as_ptr()) };
    let close = unsafe { gtk_button_new_with_label(ui_text::settings_close_btn().as_ptr()) };
    add_class(about, "settings-about-btn");
    add_class(close, "settings-close-btn");
    unsafe {
        gtk_box_pack_start(row, about, 0, 0, 0);
        gtk_box_pack_start(row, close, 0, 0, 0);
        gtk_widget_set_halign(close, GTK_ALIGN_START);
    }
    (about, close, row)
}

fn populate_initial_values(state: &SettingsState, prefs: &UserPrefs) {
    state.updating.set(true);
    let w = &state.widgets;
    populate_theme_combo(w.theme_combo, prefs.theme);
    populate_locale_combo(w.locale_combo, prefs.locale);
    let font_c = CString::new(prefs.font_desc.as_str()).unwrap_or_default();
    unsafe {
        gtk_entry_set_text(w.font_entry, font_c.as_ptr());
        gtk_spin_button_set_value(w.scrollback_spin, prefs.scrollback_lines as f64);
        gtk_switch_set_active(w.osc52_switch, c_int::from(prefs.osc52_enabled));
        gtk_spin_button_set_value(w.notify_spin, prefs.notify_secs as f64);
    }
    state.updating.set(false);
}

fn populate_theme_combo(combo: *mut GtkWidget, active: ThemeMode) {
    unsafe {
        gtk_combo_box_text_remove_all(combo);
        gtk_combo_box_text_append(combo, c"dark".as_ptr(), ui_text::settings_theme_dark().as_ptr());
        gtk_combo_box_text_append(combo, c"light".as_ptr(), ui_text::settings_theme_light().as_ptr());
    }
    let id = match active {
        ThemeMode::Dark => c"dark",
        ThemeMode::Light => c"light",
    };
    unsafe { gtk_combo_box_set_active_id(combo, id.as_ptr()) };
}

fn populate_locale_combo(combo: *mut GtkWidget, active: UiLocale) {
    unsafe {
        gtk_combo_box_text_remove_all(combo);
        gtk_combo_box_text_append(combo, c"auto".as_ptr(), ui_text::settings_locale_auto().as_ptr());
        gtk_combo_box_text_append(combo, c"ru".as_ptr(), ui_text::settings_locale_ru().as_ptr());
        gtk_combo_box_text_append(combo, c"en".as_ptr(), ui_text::settings_locale_en().as_ptr());
    }
    let id = match active {
        UiLocale::Auto => c"auto",
        UiLocale::Ru => c"ru",
        UiLocale::En => c"en",
    };
    unsafe { gtk_combo_box_set_active_id(combo, id.as_ptr()) };
}

fn read_combo_id(combo: *mut GtkWidget) -> String {
    let ptr = unsafe { gtk_combo_box_get_active_id(combo) };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

fn read_entry_text(entry: *mut GtkWidget) -> String {
    let ptr = unsafe { gtk_entry_get_text(entry) };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

fn read_prefs_from_widgets(w: &SettingsWidgets) -> UserPrefs {
    let theme = match read_combo_id(w.theme_combo).as_str() {
        "light" => ThemeMode::Light,
        _ => ThemeMode::Dark,
    };
    let locale = UiLocale::from_config(&read_combo_id(w.locale_combo)).unwrap_or_default();
    let scrollback = unsafe { gtk_spin_button_get_value_as_int(w.scrollback_spin) };
    let osc52 = unsafe { gtk_switch_get_active(w.osc52_switch) } != 0;
    let notify = unsafe { gtk_spin_button_get_value_as_int(w.notify_spin) };
    normalize_prefs(&UserPrefs {
        theme,
        locale,
        font_desc: read_entry_text(w.font_entry),
        scrollback_lines: scrollback.max(0) as usize,
        osc52_enabled: osc52,
        notify_secs: notify.max(0) as u64,
    })
}

fn refresh_labels(state: &SettingsState, prefs: &UserPrefs) {
    state.updating.set(true);
    let w = &state.widgets;
    unsafe { gtk_window_set_title(w.window, ui_text::settings_title().as_ptr()) };
    refresh_text_labels(w);
    populate_theme_combo(w.theme_combo, prefs.theme);
    populate_locale_combo(w.locale_combo, prefs.locale);
    state.updating.set(false);
}

fn set_lbl_cstr(lbl: *mut GtkWidget, text: &CStr) {
    set_label_text(lbl, text.to_str().unwrap_or_default());
}

fn refresh_text_labels(w: &SettingsWidgets) {
    set_lbl_cstr(w.title_lbl, ui_text::settings_title());
    set_lbl_cstr(w.subtitle_lbl, ui_text::settings_subtitle());
    set_lbl_cstr(w.theme_lbl, ui_text::settings_theme());
    set_lbl_cstr(w.locale_lbl, ui_text::settings_locale());
    set_lbl_cstr(w.font_lbl, ui_text::settings_font());
    set_lbl_cstr(w.scrollback_lbl, ui_text::settings_scrollback());
    refresh_hint_and_action_labels(w);
}

fn refresh_hint_and_action_labels(w: &SettingsWidgets) {
    set_lbl_cstr(w.osc52_lbl, ui_text::settings_osc52());
    set_lbl_cstr(w.osc52_hint, ui_text::settings_osc52_hint());
    set_lbl_cstr(w.notify_lbl, ui_text::settings_notify_secs());
    set_lbl_cstr(w.notify_hint, ui_text::settings_notify_hint());
    unsafe {
        gtk_entry_set_placeholder_text(w.font_entry, ui_text::settings_font_placeholder().as_ptr());
        gtk_button_set_label(w.about_btn, ui_text::settings_about_btn().as_ptr());
        gtk_button_set_label(w.close_btn, ui_text::settings_close_btn().as_ptr());
    }
}

fn apply_live_change(state: &SettingsState) {
    if state.updating.get() {
        return;
    }
    let prefs = read_prefs_from_widgets(&state.widgets);
    set_locale(prefs.locale);
    let _ = save_user_preferences(&state.config_path, &prefs);
    refresh_labels(state, &prefs);
    (state.on_change)(&prefs);
}

fn connect_settings_signals(state: &Rc<SettingsState>) {
    let w = &state.widgets;
    connect_state_signal(w.theme_combo, c"changed", on_widget_changed as *const c_void, state);
    connect_state_signal(w.locale_combo, c"changed", on_widget_changed as *const c_void, state);
    connect_state_signal(w.font_entry, c"changed", on_widget_changed as *const c_void, state);
    connect_state_signal(w.scrollback_spin, c"value-changed", on_widget_changed as *const c_void, state);
    connect_state_signal(w.osc52_switch, c"notify::active", on_switch_active as *const c_void, state);
    connect_state_signal(w.notify_spin, c"value-changed", on_widget_changed as *const c_void, state);
    connect_state_signal(w.about_btn, c"clicked", on_about_clicked as *const c_void, state);
    connect_state_signal(w.close_btn, c"clicked", on_close_clicked as *const c_void, state);
    connect_state_signal(w.window, c"key-press-event", on_key_press as *const c_void, state);
    connect_state_signal(w.window, c"destroy", on_window_destroy as *const c_void, state);
}

fn connect_state_signal(
    widget: *mut GtkWidget,
    signal: &CStr,
    handler: *const c_void,
    state: &Rc<SettingsState>,
) {
    let ptr = Rc::into_raw(Rc::clone(state)) as *mut c_void;
    unsafe {
        g_signal_connect_data(
            widget as *mut c_void,
            signal.as_ptr(),
            handler,
            ptr,
            drop_rc_state as *const c_void,
            0,
        );
    }
}

unsafe extern "C" fn on_widget_changed(_widget: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let state = unsafe { &*(data as *const SettingsState) };
    apply_live_change(state);
}

unsafe extern "C" fn on_switch_active(
    _widget: *mut GtkWidget,
    _pspec: *mut c_void,
    data: *mut c_void,
) {
    if data.is_null() {
        return;
    }
    let state = unsafe { &*(data as *const SettingsState) };
    apply_live_change(state);
}

unsafe extern "C" fn on_about_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let state = unsafe { &*(data as *const SettingsState) };
    crate::about::open_about_dialog(state.widgets.window);
}

unsafe extern "C" fn on_close_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let state = unsafe { &*(data as *const SettingsState) };
    unsafe { gtk_widget_destroy(state.widgets.window) };
}

unsafe extern "C" fn on_window_destroy(_win: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let state = unsafe { &*(data as *const SettingsState) };
    state.updating.set(true);
}

unsafe extern "C" fn on_key_press(
    win: *mut GtkWidget,
    ev: *mut GdkEventKey,
    _data: *mut c_void,
) -> c_int {
    if ev.is_null() {
        return 0;
    }
    let keyval = unsafe { (*ev).keyval };
    if keyval == GDK_KEY_ESCAPE {
        unsafe { gtk_widget_destroy(win) };
        return 1;
    }
    let state = unsafe { (*ev).state };
    let is_close = keyval == GDK_KEY_COMMA && (state & GDK_CONTROL_MASK) != 0;
    if is_close {
        unsafe { gtk_widget_destroy(win) };
        return 1;
    }
    0
}

unsafe extern "C" fn on_font_preset_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let (entry, idx) = unsafe { &*(data as *const (*mut GtkWidget, usize)) };
    if let Some((_, full_desc)) = FONT_PRESETS.get(*idx) {
        let c_desc = CString::new(*full_desc).unwrap_or_default();
        unsafe { gtk_entry_set_text(*entry, c_desc.as_ptr()) };
    }
}

unsafe extern "C" fn drop_preset_payload(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data as *mut (*mut GtkWidget, usize))) };
    }
}

unsafe extern "C" fn drop_rc_state(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Rc::from_raw(data as *const SettingsState)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::{GTK_WINDOW_TOPLEVEL as FFI_TOPLEVEL, has_class};
    use crate::i18n::with_locale_lock;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use std::cell::RefCell;

    unsafe extern "C" {
        fn gtk_window_get_title(win: *mut GtkWidget) -> *const c_char;
        fn gtk_button_clicked(btn: *mut GtkWidget);
    }

    fn temp_config_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("akhterm-settings-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.conf")
    }

    #[test]
    fn recognizes_ctrl_comma_shortcut_in_ru_and_en() {
        assert!(is_settings_shortcut(GDK_KEY_COMMA, GDK_CONTROL_MASK, 0));
        assert!(is_settings_shortcut(0x06c2, GDK_CONTROL_MASK, KEYCODE_COMMA));
        assert!(!is_settings_shortcut(GDK_KEY_COMMA, 0, KEYCODE_COMMA));
        assert!(!is_settings_shortcut(GDK_KEY_COMMA, GDK_CONTROL_MASK | crate::ffi::GDK_SHIFT_MASK, 0));
    }

    unsafe fn drive_settings_controls(dlg: &SettingsDialog) {
        assert!(has_class(dlg.window(), "settings-dialog"));
        unsafe {
            gtk_combo_box_set_active_id(dlg.theme_combo(), c"light".as_ptr());
            gtk_combo_box_set_active_id(dlg.locale_combo(), c"en".as_ptr());
        }
        pump_gtk_events();
        let title = unsafe { CStr::from_ptr(gtk_window_get_title(dlg.window())) };
        assert_eq!(title.to_str().unwrap(), "AkhTerm Settings");
        unsafe {
            gtk_entry_set_text(dlg.font_entry(), c"IBM Plex Mono 12".as_ptr());
            gtk_switch_set_active(dlg.osc52_switch(), 0);
            gtk_spin_button_set_value(dlg.scrollback_spin(), 32000.0);
            gtk_spin_button_set_value(dlg.notify_spin(), 45.0);
        }
        pump_gtk_events();
    }

    fn verify_persisted_values(cfg_path: &PathBuf, last: &UserPrefs) {
        assert_eq!(last.theme, ThemeMode::Light);
        assert_eq!(last.locale, UiLocale::En);
        assert_eq!(last.font_desc, "IBM Plex Mono 12");
        assert!(!last.osc52_enabled);
        assert_eq!(last.scrollback_lines, 32000);
        assert_eq!(last.notify_secs, 45);
        let disk = std::fs::read_to_string(cfg_path).unwrap();
        assert!(disk.contains("local | dev | ~/work"));
        assert!(disk.contains("font | IBM Plex Mono 12"));
        assert!(disk.contains("osc52 | off"));
    }

    #[test]
    fn settings_dialog_updates_live_and_persists_config() {
        with_locale_lock(|| {
            set_locale(UiLocale::Ru);
            let cfg_path = temp_config_path("live");
            std::fs::write(&cfg_path, "# keep comment\nlocal | dev | ~/work\n").unwrap();
            let observed: Rc<RefCell<Vec<UserPrefs>>> = Rc::new(RefCell::new(Vec::new()));
            let obs_cb = Rc::clone(&observed);
            with_headless_gtk(|| unsafe {
                let parent = gtk_window_new(FFI_TOPLEVEL);
                let dlg = open_settings_dialog(
                    parent,
                    UserPrefs::default(),
                    cfg_path.clone(),
                    Box::new(move |p| obs_cb.borrow_mut().push(p.clone())),
                );
                pump_gtk_events();
                drive_settings_controls(&dlg);
                assert_eq!(dlg.current_prefs(), observed.borrow().last().unwrap().clone());
                verify_persisted_values(&cfg_path, observed.borrow().last().unwrap());
                assert!(!dlg.about_button().is_null());
                gtk_button_clicked(dlg.close_button());
                pump_gtk_events();
                gtk_widget_destroy(parent);
                pump_gtk_events();
            });
            set_locale(UiLocale::Auto);
            let _ = std::fs::remove_dir_all(cfg_path.parent().unwrap());
        });
    }

    #[test]
    fn settings_dialog_respects_minimum_mi() {
        let (_, _, score) = crate::mi::compute_file_mi(include_str!("settings_dialog.rs"));
        assert!(score >= 60.0, "settings_dialog.rs MI {score:.2} < 60.0");
    }
}
