#![allow(dead_code)]
//! Minimal, safe-wrapped C FFI bindings for GTK3, GDK3, GLib, Pango, and VTE-2.91.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};

pub type GtkWidget = c_void;
pub type GtkWindow = c_void;
pub type GtkContainer = c_void;
pub type GtkBox = c_void;
pub type GtkStack = c_void;
pub type GtkPaned = c_void;
pub type GtkButton = c_void;
pub type GtkToggleButton = c_void;
pub type GtkLabel = c_void;
pub type GtkAdjustment = c_void;
pub type GtkPopover = c_void;
pub type GtkStyleContext = c_void;
pub type GtkCssProvider = c_void;
pub type GtkSettings = c_void;
pub type GtkHeaderBar = c_void;
pub type GtkClipboard = c_void;
pub type GtkIconTheme = c_void;
pub type GtkSelectionData = c_void;
pub type GtkTargetList = c_void;
pub type GdkDisplay = c_void;
pub type GdkDragContext = c_void;
pub type GdkScreen = c_void;
pub type GdkEvent = c_void;
pub type GdkPixbuf = c_void;
pub type GdkAtom = *mut c_void;
pub type VteTerminal = c_void;
pub type VteRegex = c_void;
pub type PangoFontDescription = c_void;

pub const GDK_SELECTION_CLIPBOARD: GdkAtom = 69usize as GdkAtom;

pub const PCRE2_CASELESS: c_uint = 0x0000_0008;
pub const PCRE2_MULTILINE_UTF: c_uint = 0x0008_0400; // PCRE2_UTF (0x80000) | PCRE2_MULTILINE (0x400)
pub const PCRE2_SEARCH_CASELESS: c_uint = PCRE2_MULTILINE_UTF | PCRE2_CASELESS; // 0x0008_0408

pub const GTK_WINDOW_TOPLEVEL: c_int = 0;
pub const GTK_ORIENTATION_HORIZONTAL: c_int = 0;
pub const GTK_ORIENTATION_VERTICAL: c_int = 1;
pub const GTK_ALIGN_START: c_int = 1;
pub const GTK_ALIGN_END: c_int = 2;
pub const GTK_ALIGN_CENTER: c_int = 3;
pub const GTK_POLICY_NEVER: c_int = 2;
pub const GTK_POLICY_AUTOMATIC: c_int = 1;
pub const GTK_SCROLL_NATURAL: c_int = 1;
pub const GTK_ICON_SIZE_MENU: c_int = 1;
pub const PANGO_ELLIPSIZE_MIDDLE: c_int = 2;
pub const PANGO_ELLIPSIZE_END: c_int = 3;
pub const GTK_STYLE_PROVIDER_PRIORITY_APPLICATION: c_uint = 600;
pub const G_SPAWN_SEARCH_PATH: c_int = 4;
pub const VTE_FORMAT_TEXT: c_int = 1;

pub const GDK_MOTION_NOTIFY: c_int = 3;
pub const GDK_BUTTON_PRESS: c_int = 4;
pub const GDK_2BUTTON_PRESS: c_int = 5;
pub const GDK_BUTTON_RELEASE: c_int = 7;
pub const GDK_BUTTON1_MOTION_MASK: c_int = 1 << 5;
pub const GDK_BUTTON_PRESS_MASK: c_int = 1 << 8;
pub const GDK_BUTTON_RELEASE_MASK: c_int = 1 << 9;
pub const GDK_CONTROL_MASK: c_uint = 1 << 2;
pub const GDK_SHIFT_MASK: c_uint = 1 << 0;
pub const GDK_MOD1_MASK: c_uint = 1 << 3; // Alt
pub const GTK_DEST_DEFAULT_ALL: c_uint = 0x07;
pub const DEST_DEFAULT_ALL: c_uint = GTK_DEST_DEFAULT_ALL;
pub const GDK_ACTION_COPY: c_uint = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GtkTargetEntry {
    pub target: *const c_char,
    pub flags: c_uint,
    pub info: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GdkRGBA {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

impl GdkRGBA {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self {
            red: (r as f64) / 255.0,
            green: (g as f64) / 255.0,
            blue: (b as f64) / 255.0,
            alpha: 1.0,
        }
    }
}

extern "C" {
    pub fn kill(pid: i32, sig: c_int) -> c_int;
    pub fn getpgid(pid: i32) -> i32;
    pub fn flock(fd: c_int, operation: c_int) -> c_int;

    // GLib / GObject
    pub fn g_free(mem: *mut c_void);
    pub fn g_strfreev(str_array: *mut *mut c_char);
    pub fn g_set_prgname(prgname: *const c_char);
    pub fn g_get_prgname() -> *const c_char;
    pub fn g_set_application_name(application_name: *const c_char);
    pub fn g_get_application_name() -> *const c_char;
    pub fn g_type_name_from_instance(instance: *mut c_void) -> *const c_char;
    pub fn g_filename_from_uri(
        uri: *const c_char,
        hostname: *mut *mut c_char,
        error: *mut *mut c_void,
    ) -> *mut c_char;
    pub fn g_object_get(object: *mut c_void, first_property_name: *const c_char, ...);
    pub fn g_object_set(object: *mut c_void, first_property_name: *const c_char, ...);
    pub fn g_object_ref(object: *mut c_void) -> *mut c_void;
    pub fn g_object_unref(object: *mut c_void);
    pub fn g_timeout_add(interval: c_uint, function: extern "C" fn(*mut c_void) -> c_int, data: *mut c_void) -> c_uint;
    pub fn g_idle_add(function: extern "C" fn(*mut c_void) -> c_int, data: *mut c_void) -> c_uint;
    pub fn g_unix_signal_add(signum: c_int, function: extern "C" fn(*mut c_void) -> c_int, data: *mut c_void) -> c_uint;
    pub fn g_signal_connect_data(
        instance: *mut c_void,
        detailed_signal: *const c_char,
        c_handler: *const c_void,
        data: *mut c_void,
        destroy_data: *const c_void,
        connect_flags: c_int,
    ) -> usize;

    // GDK3
    pub fn gdk_display_get_default() -> *mut GdkDisplay;
    pub fn gdk_screen_get_default() -> *mut GdkScreen;
    pub fn gdk_atom_intern_static_string(atom_name: *const c_char) -> GdkAtom;
    pub fn gdk_event_new(event_type: c_int) -> *mut GdkEvent;
    pub fn gdk_event_free(event: *mut GdkEvent);
    pub fn gdk_event_get_event_type(event: *const GdkEvent) -> c_int;
    pub fn gdk_event_get_button(event: *const GdkEvent, button: *mut c_uint) -> c_int;
    pub fn gdk_event_get_coords(event: *const GdkEvent, x_win: *mut f64, y_win: *mut f64) -> c_int;
    pub fn gdk_event_get_keyval(event: *const GdkEvent, keyval: *mut c_uint) -> c_int;
    pub fn gdk_event_get_keycode(event: *const GdkEvent, keycode: *mut u16) -> c_int;
    pub fn gdk_event_get_state(event: *const GdkEvent, state: *mut c_uint) -> c_int;

    // Pango
    pub fn pango_font_description_from_string(str: *const c_char) -> *mut PangoFontDescription;
    pub fn pango_font_description_free(desc: *mut PangoFontDescription);

    // GTK3
    pub fn gtk_init(argc: *mut c_int, argv: *mut *mut *mut c_char);
    pub fn gtk_settings_get_default() -> *mut GtkSettings;
    pub fn gtk_icon_theme_get_default() -> *mut GtkIconTheme;
    pub fn gtk_icon_theme_has_icon(icon_theme: *mut GtkIconTheme, icon_name: *const c_char) -> c_int;
    pub fn gtk_widget_set_name(widget: *mut GtkWidget, name: *const c_char);
    pub fn gtk_main();
    pub fn gtk_main_quit();

    pub fn gtk_window_new(wtype: c_int) -> *mut GtkWidget;
    pub fn gtk_window_set_default_icon_name(name: *const c_char);
    pub fn gtk_window_get_default_icon_name() -> *const c_char;
    pub fn gtk_window_set_title(window: *mut GtkWindow, title: *const c_char);
    pub fn gtk_window_set_titlebar(window: *mut GtkWindow, titlebar: *mut GtkWidget);
    pub fn gtk_window_set_default_size(window: *mut GtkWindow, width: c_int, height: c_int);
    pub fn gtk_window_set_decorated(window: *mut GtkWindow, setting: c_int);
    pub fn gtk_window_set_resizable(window: *mut GtkWindow, resizable: c_int);
    pub fn gtk_window_set_wmclass(window: *mut GtkWindow, wmclass_name: *const c_char, wmclass_class: *const c_char);
    pub fn gtk_window_is_active(window: *mut GtkWindow) -> c_int;
    pub fn gtk_window_set_urgency_hint(window: *mut GtkWindow, setting: c_int);
    pub fn gtk_window_get_urgency_hint(window: *mut GtkWindow) -> c_int;
    pub fn gtk_window_maximize(window: *mut GtkWindow);
    pub fn gtk_window_unmaximize(window: *mut GtkWindow);
    pub fn gtk_window_is_maximized(window: *mut GtkWindow) -> c_int;
    pub fn gtk_window_fullscreen(window: *mut GtkWindow);
    pub fn gtk_window_unfullscreen(window: *mut GtkWindow);
    pub fn gtk_window_get_focus(window: *mut GtkWindow) -> *mut GtkWidget;

    pub fn gtk_header_bar_new() -> *mut GtkWidget;
    pub fn gtk_header_bar_set_custom_title(bar: *mut GtkHeaderBar, title: *mut GtkWidget);
    pub fn gtk_header_bar_set_show_close_button(bar: *mut GtkHeaderBar, setting: c_int);

    pub fn gtk_css_provider_new() -> *mut GtkCssProvider;
    pub fn gtk_css_provider_load_from_data(
        provider: *mut GtkCssProvider,
        data: *const c_char,
        length: isize,
        error: *mut *mut c_void,
    ) -> c_int;
    pub fn gtk_style_context_add_provider_for_screen(
        screen: *mut GdkScreen,
        provider: *mut GtkCssProvider,
        priority: c_uint,
    );
    pub fn gtk_style_context_reset_widgets(screen: *mut GdkScreen);
    pub fn gtk_style_context_lookup_color(
        context: *mut GtkStyleContext,
        color_name: *const c_char,
        color: *mut GdkRGBA,
    ) -> c_int;
    pub fn gtk_widget_get_style_context(widget: *mut GtkWidget) -> *mut GtkStyleContext;
    pub fn gtk_style_context_add_class(context: *mut GtkStyleContext, class_name: *const c_char);
    pub fn gtk_style_context_remove_class(context: *mut GtkStyleContext, class_name: *const c_char);
    pub fn gtk_style_context_has_class(context: *mut GtkStyleContext, class_name: *const c_char) -> c_int;

    pub fn gtk_box_new(orientation: c_int, spacing: c_int) -> *mut GtkWidget;
    pub fn gtk_box_pack_start(box_: *mut GtkBox, child: *mut GtkWidget, expand: c_int, fill: c_int, padding: c_uint);
    pub fn gtk_box_pack_end(box_: *mut GtkBox, child: *mut GtkWidget, expand: c_int, fill: c_int, padding: c_uint);
    pub fn gtk_box_reorder_child(box_: *mut GtkBox, child: *mut GtkWidget, position: c_int);

    pub fn gtk_scrolled_window_new(hadj: *mut c_void, vadj: *mut c_void) -> *mut GtkWidget;
    pub fn gtk_scrolled_window_set_policy(sw: *mut GtkWidget, hpol: c_int, vpol: c_int);
    pub fn gtk_scrolled_window_set_kinetic_scrolling(
        scrolled_window: *mut GtkWidget,
        kinetic_scrolling: c_int,
    );
    #[cfg(test)]
    pub fn gtk_scrolled_window_get_kinetic_scrolling(scrolled_window: *mut GtkWidget) -> c_int;
    pub fn gtk_scrolled_window_get_hadjustment(sw: *mut GtkWidget) -> *mut GtkAdjustment;
    pub fn gtk_scrolled_window_get_vadjustment(sw: *mut GtkWidget) -> *mut GtkAdjustment;
    pub fn gtk_scrolled_window_set_min_content_width(sw: *mut GtkWidget, width: c_int);
    pub fn gtk_scrolled_window_set_min_content_height(sw: *mut GtkWidget, height: c_int);
    pub fn gtk_scrolled_window_set_max_content_height(sw: *mut GtkWidget, height: c_int);
    pub fn gtk_viewport_new(hadj: *mut GtkAdjustment, vadj: *mut GtkAdjustment) -> *mut GtkWidget;
    pub fn gtk_scrollable_set_hscroll_policy(scrollable: *mut GtkWidget, policy: c_int);

    pub fn gtk_entry_new() -> *mut GtkWidget;
    pub fn gtk_search_entry_new() -> *mut GtkWidget;
    pub fn gtk_entry_set_placeholder_text(entry: *mut GtkWidget, text: *const c_char);
    pub fn gtk_entry_set_text(entry: *mut GtkWidget, text: *const c_char);
    pub fn gtk_entry_get_text(entry: *mut GtkWidget) -> *const c_char;
    pub fn gtk_editable_select_region(editable: *mut GtkWidget, start_pos: c_int, end_pos: c_int);

    pub fn gtk_stack_new() -> *mut GtkWidget;
    pub fn gtk_stack_add_named(stack: *mut GtkStack, child: *mut GtkWidget, name: *const c_char);
    pub fn gtk_stack_set_visible_child_name(stack: *mut GtkStack, name: *const c_char);

    pub fn gtk_paned_new(orientation: c_int) -> *mut GtkWidget;
    pub fn gtk_paned_pack1(paned: *mut GtkPaned, child: *mut GtkWidget, resize: c_int, shrink: c_int);
    pub fn gtk_paned_pack2(paned: *mut GtkPaned, child: *mut GtkWidget, resize: c_int, shrink: c_int);
    pub fn gtk_paned_get_child1(paned: *mut GtkPaned) -> *mut GtkWidget;
    pub fn gtk_paned_get_child2(paned: *mut GtkPaned) -> *mut GtkWidget;
    pub fn gtk_paned_set_position(paned: *mut GtkPaned, position: c_int);
    pub fn gtk_paned_set_wide_handle(paned: *mut GtkPaned, wide: c_int);

    pub fn gtk_button_new() -> *mut GtkWidget;
    pub fn gtk_button_new_with_label(label: *const c_char) -> *mut GtkWidget;
    pub fn gtk_button_get_label(button: *mut GtkButton) -> *const c_char;
    pub fn gtk_button_set_label(button: *mut GtkButton, label: *const c_char);
    pub fn gtk_button_clicked(button: *mut GtkButton);
    pub fn gtk_toggle_button_new_with_label(label: *const c_char) -> *mut GtkWidget;
    pub fn gtk_toggle_button_get_active(toggle_button: *mut GtkToggleButton) -> c_int;
    pub fn gtk_toggle_button_set_active(toggle_button: *mut GtkToggleButton, is_active: c_int);
    pub fn gtk_image_new_from_icon_name(icon_name: *const c_char, size: c_int) -> *mut GtkWidget;
    pub fn gtk_image_set_from_icon_name(image: *mut GtkWidget, icon_name: *const c_char, size: c_int);
    pub fn gtk_expander_new(label: *const c_char) -> *mut GtkWidget;
    pub fn gtk_expander_set_expanded(expander: *mut GtkWidget, expanded: c_int);
    pub fn gtk_expander_set_label_widget(expander: *mut GtkWidget, label: *mut GtkWidget);

    pub fn gtk_label_new(str: *const c_char) -> *mut GtkWidget;
    pub fn gtk_label_get_text(label: *mut GtkLabel) -> *const c_char;
    pub fn gtk_label_set_text(label: *mut GtkLabel, str: *const c_char);
    pub fn gtk_label_set_markup(label: *mut GtkLabel, str: *const c_char);
    pub fn gtk_label_set_xalign(label: *mut GtkLabel, xalign: f32);
    pub fn gtk_label_set_ellipsize(label: *mut GtkLabel, mode: c_int);
    pub fn gtk_label_set_max_width_chars(label: *mut GtkLabel, count: c_int);

    pub fn gtk_popover_new(relative_to: *mut GtkWidget) -> *mut GtkWidget;
    pub fn gtk_popover_set_relative_to(popover: *mut GtkPopover, relative_to: *mut GtkWidget);
    pub fn gtk_popover_popup(popover: *mut GtkPopover);
    pub fn gtk_popover_popdown(popover: *mut GtkPopover);

    pub fn gtk_menu_new() -> *mut GtkWidget;
    pub fn gtk_menu_shell_append(menu_shell: *mut GtkWidget, child: *mut GtkWidget);
    pub fn gtk_menu_item_new_with_label(label: *const c_char) -> *mut GtkWidget;
    pub fn gtk_menu_item_get_label(menu_item: *mut GtkWidget) -> *const c_char;
    pub fn gtk_menu_item_set_submenu(menu_item: *mut GtkWidget, submenu: *mut GtkWidget);
    pub fn gtk_menu_item_get_submenu(menu_item: *mut GtkWidget) -> *mut GtkWidget;
    pub fn gtk_menu_item_activate(menu_item: *mut GtkWidget);
    pub fn gtk_separator_menu_item_new() -> *mut GtkWidget;
    pub fn gtk_menu_attach_to_widget(
        menu: *mut GtkWidget,
        attach_widget: *mut GtkWidget,
        detacher: *const c_void,
    );
    pub fn gtk_menu_popup_at_pointer(menu: *mut GtkWidget, trigger_event: *const GdkEvent);

    pub fn gtk_container_add(container: *mut GtkContainer, widget: *mut GtkWidget);
    pub fn gtk_container_remove(container: *mut GtkContainer, widget: *mut GtkWidget);
    pub fn gtk_container_set_focus_hadjustment(container: *mut GtkContainer, adjustment: *mut GtkAdjustment);
    pub fn gtk_adjustment_clamp_page(adjustment: *mut GtkAdjustment, lower: f64, upper: f64);
    pub fn gtk_widget_add_events(widget: *mut GtkWidget, events: c_int);
    pub fn gtk_widget_show_all(widget: *mut GtkWidget);
    pub fn gtk_widget_get_visible(widget: *mut GtkWidget) -> c_int;
    pub fn gtk_widget_set_visible(widget: *mut GtkWidget, visible: c_int);
    pub fn gtk_widget_get_sensitive(widget: *mut GtkWidget) -> c_int;
    pub fn gtk_widget_set_sensitive(widget: *mut GtkWidget, sensitive: c_int);
    pub fn gtk_widget_set_no_show_all(widget: *mut GtkWidget, no_show_all: c_int);
    pub fn gtk_widget_destroy(widget: *mut GtkWidget);
    pub fn gtk_widget_set_can_focus(widget: *mut GtkWidget, can_focus: c_int);
    pub fn gtk_widget_grab_focus(widget: *mut GtkWidget);
    pub fn gtk_widget_set_size_request(widget: *mut GtkWidget, width: c_int, height: c_int);
    pub fn gtk_widget_set_halign(widget: *mut GtkWidget, align: c_int);
    pub fn gtk_widget_set_valign(widget: *mut GtkWidget, align: c_int);
    pub fn gtk_widget_get_allocated_width(widget: *mut GtkWidget) -> c_int;
    pub fn gtk_widget_get_allocated_height(widget: *mut GtkWidget) -> c_int;
    pub fn gtk_widget_get_parent(widget: *mut GtkWidget) -> *mut GtkWidget;
    pub fn gtk_widget_is_focus(widget: *mut GtkWidget) -> c_int;
    pub fn gtk_widget_has_focus(widget: *mut GtkWidget) -> c_int;
    pub fn gtk_widget_translate_coordinates(
        source: *mut GtkWidget,
        destination: *mut GtkWidget,
        x: c_int,
        y: c_int,
        dest_x: *mut c_int,
        dest_y: *mut c_int,
    ) -> c_int;
    pub fn gtk_widget_set_tooltip_text(widget: *mut GtkWidget, text: *const c_char);
    pub fn gtk_init_check(argc: *mut c_int, argv: *mut *mut *mut c_char) -> c_int;
    pub fn gtk_events_pending() -> c_int;
    pub fn gtk_main_iteration_do(blocking: c_int) -> c_int;

    // GTK3 Clipboard & GdkPixbuf
    pub fn gtk_clipboard_get(selection: GdkAtom) -> *mut GtkClipboard;
    pub fn gtk_clipboard_set_text(clipboard: *mut GtkClipboard, text: *const c_char, len: c_int);
    pub fn gtk_clipboard_set_image(clipboard: *mut GtkClipboard, pixbuf: *mut GdkPixbuf);
    pub fn gtk_clipboard_wait_for_targets(
        clipboard: *mut GtkClipboard,
        targets: *mut *mut GdkAtom,
        n_targets: *mut c_int,
    ) -> c_int;
    pub fn gtk_clipboard_wait_for_image(clipboard: *mut GtkClipboard) -> *mut GdkPixbuf;
    pub fn gtk_clipboard_wait_for_text(clipboard: *mut GtkClipboard) -> *mut c_char;
    pub fn gtk_clipboard_wait_for_uris(clipboard: *mut GtkClipboard) -> *mut *mut c_char;
    pub fn gtk_targets_include_text(targets: *const GdkAtom, n_targets: c_int) -> c_int;
    pub fn gtk_targets_include_image(
        targets: *const GdkAtom,
        n_targets: c_int,
        writable: c_int,
    ) -> c_int;
    pub fn gtk_targets_include_uri(targets: *const GdkAtom, n_targets: c_int) -> c_int;
    pub fn gtk_drag_dest_set(w: *mut GtkWidget, f: c_uint, t: *const GtkTargetEntry, n: c_int, a: c_uint);
    pub fn gtk_drag_dest_add_uri_targets(widget: *mut GtkWidget);
    pub fn gtk_drag_dest_get_target_list(widget: *mut GtkWidget) -> *mut GtkTargetList;
    pub fn gtk_target_list_find(list: *mut GtkTargetList, target: GdkAtom, info: *mut c_uint) -> c_int;
    pub fn gtk_selection_data_get_uris(selection_data: *const GtkSelectionData) -> *mut *mut c_char;
    #[cfg(test)]
    pub fn gtk_clipboard_set_with_data(
        clipboard: *mut GtkClipboard,
        targets: *const GtkTargetEntry,
        n_targets: u32,
        get_func: extern "C" fn(*mut GtkClipboard, *mut GtkSelectionData, u32, *mut c_void),
        clear_func: extern "C" fn(*mut GtkClipboard, *mut c_void),
        user_data: *mut c_void,
    ) -> c_int;
    #[cfg(test)]
    pub fn gtk_selection_data_set_uris(selection_data: *mut GtkSelectionData, uris: *const *const c_char) -> c_int;
    #[cfg(test)]
    pub fn gtk_selection_data_set_text(selection_data: *mut GtkSelectionData, str: *const c_char, len: c_int) -> c_int;
    pub fn gtk_drag_finish(ctx: *mut GdkDragContext, success: c_int, del: c_int, time: c_uint);
    pub fn gdk_pixbuf_new(
        colorspace: c_int,
        has_alpha: c_int,
        bits_per_sample: c_int,
        width: c_int,
        height: c_int,
    ) -> *mut GdkPixbuf;
    pub fn gdk_pixbuf_save_to_bufferv(
        pixbuf: *mut GdkPixbuf,
        buffer: *mut *mut c_char,
        buffer_size: *mut usize,
        img_type: *const c_char,
        option_keys: *mut *mut c_char,
        option_values: *mut *mut c_char,
        error: *mut *mut c_void,
    ) -> c_int;

    // VTE-2.91
    pub fn vte_terminal_new() -> *mut GtkWidget;
    pub fn vte_terminal_feed(terminal: *mut VteTerminal, data: *const c_char, length: isize);
    pub fn vte_terminal_select_all(terminal: *mut VteTerminal);
    pub fn vte_terminal_unselect_all(terminal: *mut VteTerminal);
    pub fn vte_terminal_get_has_selection(terminal: *mut VteTerminal) -> c_int;
    pub fn vte_terminal_spawn_sync(
        terminal: *mut VteTerminal,
        pty_flags: c_int,
        working_directory: *const c_char,
        argv: *const *const c_char,
        envv: *const *const c_char,
        spawn_flags: c_int,
        child_setup: *const c_void,
        child_setup_data: *mut c_void,
        child_pid: *mut i32,
        cancellable: *mut c_void,
        error: *mut *mut c_void,
    ) -> c_int;
    pub fn vte_terminal_feed_child(terminal: *mut VteTerminal, text: *const c_char, length: isize);
    pub fn vte_terminal_set_colors(
        terminal: *mut VteTerminal,
        foreground: *const GdkRGBA,
        background: *const GdkRGBA,
        palette: *const GdkRGBA,
        palette_size: usize,
    );
    pub fn vte_terminal_set_color_cursor(terminal: *mut VteTerminal, cursor_background: *const GdkRGBA);
    pub fn vte_terminal_set_font(terminal: *mut VteTerminal, font_desc: *const PangoFontDescription);
    pub fn vte_terminal_set_font_scale(terminal: *mut VteTerminal, scale: f64);
    pub fn vte_terminal_get_font_scale(terminal: *mut VteTerminal) -> f64;
    pub fn vte_terminal_set_scrollback_lines(terminal: *mut VteTerminal, lines: isize);
    pub fn vte_terminal_set_allow_hyperlink(terminal: *mut VteTerminal, allow: c_int);
    pub fn vte_regex_new_for_match(
        pattern: *const c_char,
        pattern_length: isize,
        flags: c_uint,
        error: *mut *mut c_void,
    ) -> *mut VteRegex;
    pub fn vte_regex_new_for_search(
        pattern: *const c_char,
        pattern_length: isize,
        flags: c_uint,
        error: *mut *mut c_void,
    ) -> *mut VteRegex;
    pub fn vte_regex_unref(regex: *mut VteRegex);
    pub fn vte_terminal_search_set_regex(
        terminal: *mut VteTerminal,
        regex: *mut VteRegex,
        flags: c_uint,
    );
    pub fn vte_terminal_search_set_wrap_around(terminal: *mut VteTerminal, wrap_around: c_int);
    pub fn vte_terminal_search_find_previous(terminal: *mut VteTerminal) -> c_int;
    pub fn vte_terminal_search_find_next(terminal: *mut VteTerminal) -> c_int;
    pub fn vte_terminal_match_add_regex(terminal: *mut VteTerminal, regex: *mut VteRegex, flags: c_uint) -> c_int;
    pub fn vte_terminal_match_set_cursor_name(terminal: *mut VteTerminal, tag: c_int, cursor_name: *const c_char);
    pub fn vte_terminal_match_check_event(
        terminal: *mut VteTerminal,
        event: *const GdkEvent,
        tag: *mut c_int,
    ) -> *mut c_char;
    pub fn vte_terminal_hyperlink_check_event(terminal: *mut VteTerminal, event: *const GdkEvent) -> *mut c_char;
    pub fn vte_terminal_get_window_title(terminal: *mut VteTerminal) -> *const c_char;
    pub fn vte_terminal_get_text_selected(terminal: *mut VteTerminal, format: c_int) -> *mut c_char;
    pub fn vte_terminal_get_text_format(terminal: *mut VteTerminal, format: c_int) -> *mut c_char;
    pub fn vte_terminal_copy_clipboard_format(terminal: *mut VteTerminal, format: c_int);
    pub fn vte_terminal_paste_clipboard(terminal: *mut VteTerminal);
    pub fn vte_terminal_reset(
        terminal: *mut VteTerminal,
        clear_tabstops: c_int,
        clear_history: c_int,
    );
    pub fn vte_terminal_get_text_range_format(
        terminal: *mut VteTerminal,
        format: c_int,
        start_row: isize,
        start_col: isize,
        end_row: isize,
        end_col: isize,
        length: *mut usize,
    ) -> *mut c_char;
    pub fn vte_terminal_get_cursor_position(
        terminal: *mut VteTerminal,
        column: *mut isize,
        row: *mut isize,
    );
    pub fn vte_terminal_get_row_count(terminal: *mut VteTerminal) -> isize;
    pub fn vte_terminal_get_column_count(terminal: *mut VteTerminal) -> isize;
}

pub fn add_class(widget: *mut GtkWidget, cls: &str) {
    if let Ok(c) = CString::new(cls) {
        unsafe {
            let ctx = gtk_widget_get_style_context(widget);
            gtk_style_context_add_class(ctx, c.as_ptr());
        }
    }
}

pub fn remove_class(widget: *mut GtkWidget, cls: &str) {
    if let Ok(c) = CString::new(cls) {
        unsafe {
            let ctx = gtk_widget_get_style_context(widget);
            gtk_style_context_remove_class(ctx, c.as_ptr());
        }
    }
}

pub fn set_class(widget: *mut GtkWidget, class: &str, enabled: bool) {
    if enabled {
        add_class(widget, class);
    } else {
        remove_class(widget, class);
    }
}

pub fn has_class(widget: *mut GtkWidget, cls: &str) -> bool {
    let (false, Ok(c)) = (widget.is_null(), CString::new(cls)) else { return false };
    let ctx = unsafe { gtk_widget_get_style_context(widget) };
    !ctx.is_null() && unsafe { gtk_style_context_has_class(ctx, c.as_ptr()) != 0 }
}

pub fn set_label_text(widget: *mut GtkWidget, text: &str) {
    if let Ok(c) = CString::new(text) {
        unsafe {
            gtk_label_set_text(widget, c.as_ptr());
        }
    }
}

pub fn set_tooltip(widget: *mut GtkWidget, text: &str) {
    if let Ok(text) = CString::new(text) {
        unsafe {
            gtk_widget_set_tooltip_text(widget, text.as_ptr());
        }
    }
}

pub fn enable_uri_drag_dest(widget: *mut GtkWidget) {
    if !widget.is_null() {
        unsafe {
            gtk_drag_dest_set(widget, DEST_DEFAULT_ALL, std::ptr::null(), 0, GDK_ACTION_COPY);
            gtk_drag_dest_add_uri_targets(widget);
        }
    }
}

pub fn finish_drag(context: *mut GdkDragContext, ok: bool, time: c_uint) {
    if !context.is_null() {
        unsafe {
            gtk_drag_finish(context, i32::from(ok), 0, time);
        }
    }
}

pub fn widget_accepts_uri_drop(widget: *mut GtkWidget) -> bool {
    if widget.is_null() {
        return false;
    }
    unsafe {
        let list = gtk_drag_dest_get_target_list(widget);
        if list.is_null() {
            return false;
        }
        let atom = gdk_atom_intern_static_string(c"text/uri-list".as_ptr());
        let mut info: c_uint = 0;
        gtk_target_list_find(list, atom, &mut info) != 0
    }
}

pub fn set_image_icon(image: *mut GtkWidget, icon: &CStr) {
    if !image.is_null() {
        unsafe { gtk_image_set_from_icon_name(image, icon.as_ptr(), GTK_ICON_SIZE_MENU) };
    }
}
