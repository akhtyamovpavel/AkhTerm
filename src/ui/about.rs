//! About AkhTerm modal dialog, runtime GTK/VTE version inspection, and `--version` CLI output.

use std::ffi::{c_int, c_uint, c_void, CStr, CString};
use std::ptr;
use std::rc::Rc;

use crate::context_menu::copy_text_to_clipboard;
use crate::ffi::*;
use crate::paths::{config_path, shorten_home, state_dir};
use crate::platform::{
    detect_desktop_environment, detect_display_backend, DesktopEnvironment, DisplayBackend,
};
use crate::ui_text as text;

const GTK_WIN_POS_CENTER_ON_PARENT: c_int = 4;
const GDK_KEY_ESCAPE: c_uint = 0xff1b;

extern "C" {
    fn gtk_get_major_version() -> c_uint;
    fn gtk_get_minor_version() -> c_uint;
    fn gtk_get_micro_version() -> c_uint;
    fn vte_get_major_version() -> c_uint;
    fn vte_get_minor_version() -> c_uint;
    fn vte_get_micro_version() -> c_uint;
    fn gtk_grid_new() -> *mut GtkWidget;
    fn gtk_grid_attach(
        grid: *mut GtkWidget,
        child: *mut GtkWidget,
        left: c_int,
        top: c_int,
        width: c_int,
        height: c_int,
    );
    fn gtk_grid_set_row_spacing(grid: *mut GtkWidget, spacing: c_uint);
    fn gtk_grid_set_column_spacing(grid: *mut GtkWidget, spacing: c_uint);
    fn gtk_label_set_selectable(label: *mut GtkWidget, setting: c_int);
    fn gtk_window_set_modal(window: *mut GtkWindow, modal: c_int);
    fn gtk_window_set_transient_for(window: *mut GtkWindow, parent: *mut GtkWindow);
    fn gtk_window_set_destroy_with_parent(window: *mut GtkWindow, setting: c_int);
    fn gtk_window_set_position(window: *mut GtkWindow, position: c_int);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildInfoSnapshot {
    pub version: &'static str,
    pub git_sha: &'static str,
    pub target: &'static str,
    pub gtk_version: String,
    pub vte_version: String,
    pub backend: &'static str,
    pub desktop: &'static str,
    pub config_path: String,
    pub state_dir: String,
    pub license: &'static str,
}

pub fn format_cli_version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let sha = env!("AKHTERM_GIT_SHA");
    let target = env!("AKHTERM_BUILD_TARGET");
    format!("akhterm {version} ({sha} {target})")
}

pub fn check_version_arg(args: &[String]) -> Option<String> {
    let has_flag = args
        .iter()
        .skip(1)
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--version" || arg == "-V");
    has_flag.then(format_cli_version)
}

pub fn runtime_gtk_version() -> String {
    unsafe {
        let maj = gtk_get_major_version();
        let min = gtk_get_minor_version();
        let mic = gtk_get_micro_version();
        format!("{maj}.{min}.{mic}")
    }
}

pub fn runtime_vte_version() -> String {
    unsafe {
        let maj = vte_get_major_version();
        let min = vte_get_minor_version();
        let mic = vte_get_micro_version();
        format!("{maj}.{min}.{mic}")
    }
}

pub fn display_backend_label(backend: DisplayBackend) -> &'static str {
    match backend {
        DisplayBackend::Wayland => "Wayland",
        DisplayBackend::X11 => "X11",
        DisplayBackend::Broadway => "Broadway",
        DisplayBackend::Unknown => "Unknown",
    }
}

pub fn desktop_env_label(desktop: DesktopEnvironment) -> &'static str {
    match desktop {
        DesktopEnvironment::KdePlasma => "KDE Plasma",
        DesktopEnvironment::Gnome => "GNOME",
        DesktopEnvironment::Wlroots => "wlroots",
        DesktopEnvironment::Xfce => "XFCE",
        DesktopEnvironment::Generic => "Generic",
    }
}

pub fn collect_build_info() -> BuildInfoSnapshot {
    BuildInfoSnapshot {
        version: env!("CARGO_PKG_VERSION"),
        git_sha: env!("AKHTERM_GIT_SHA"),
        target: env!("AKHTERM_BUILD_TARGET"),
        gtk_version: runtime_gtk_version(),
        vte_version: runtime_vte_version(),
        backend: display_backend_label(detect_display_backend()),
        desktop: desktop_env_label(detect_desktop_environment()),
        config_path: config_path().to_string_lossy().into_owned(),
        state_dir: state_dir().to_string_lossy().into_owned(),
        license: "MIT",
    }
}

pub fn format_diagnostic_block(info: &BuildInfoSnapshot) -> String {
    [
        format!("AkhTerm v{} ({} {})", info.version, info.git_sha, info.target),
        format!("Stack: Rust · GTK {} · VTE {}", info.gtk_version, info.vte_version),
        format!("Display: {} · DE: {}", info.backend, info.desktop),
        format!("Config: {}", info.config_path),
        format!("State: {}", info.state_dir),
        format!("License: {}", info.license),
    ]
    .join("\n")
}

pub struct AboutDialog {
    window: *mut GtkWidget,
    #[cfg(test)]
    copy_btn: *mut GtkWidget,
    #[cfg(test)]
    close_btn: *mut GtkWidget,
    #[cfg(test)]
    diagnostic_text: String,
}

impl AboutDialog {
    pub fn window(&self) -> *mut GtkWidget {
        self.window
    }

    #[cfg(test)]
    pub fn copy_button(&self) -> *mut GtkWidget {
        self.copy_btn
    }

    #[cfg(test)]
    pub fn close_button(&self) -> *mut GtkWidget {
        self.close_btn
    }

    #[cfg(test)]
    pub fn diagnostic_text(&self) -> &str {
        &self.diagnostic_text
    }
}

struct CopyPayload {
    diagnostic: String,
}

struct ClosePayload {
    window: *mut GtkWidget,
}

extern "C" fn on_copy_clicked(button: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let payload = unsafe { &*data.cast::<CopyPayload>() };
    copy_text_to_clipboard(&payload.diagnostic);
    unsafe { gtk_button_set_label(button, text::about_copied_btn().as_ptr()) };
}

extern "C" fn drop_copy_payload(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Rc::from_raw(data.cast::<CopyPayload>())) };
    }
}

extern "C" fn on_close_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let payload = unsafe { &*data.cast::<ClosePayload>() };
    if !payload.window.is_null() {
        unsafe { gtk_widget_destroy(payload.window) };
    }
}

extern "C" fn drop_close_payload(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data.cast::<ClosePayload>())) };
    }
}

extern "C" fn on_about_key_press(
    win: *mut GtkWidget,
    event: *const GdkEvent,
    _data: *mut c_void,
) -> c_int {
    let mut keyval: c_uint = 0;
    if unsafe { gdk_event_get_keyval(event, &mut keyval) } != 0 && keyval == GDK_KEY_ESCAPE {
        unsafe { gtk_widget_destroy(win) };
        return 1;
    }
    0
}

fn make_c_label(raw: &CStr, class: &str, selectable: bool) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(raw.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, class);
        if selectable {
            gtk_label_set_selectable(lbl, 1);
        }
        lbl
    }
}

fn make_text_label(raw: &str, class: &str, tooltip: Option<&str>) -> *mut GtkWidget {
    let c_val = CString::new(raw).unwrap_or_default();
    let lbl = make_c_label(&c_val, class, true);
    unsafe {
        gtk_label_set_ellipsize(lbl, PANGO_ELLIPSIZE_MIDDLE);
        gtk_label_set_max_width_chars(lbl, 44);
    }
    if let Some(tip) = tooltip {
        set_tooltip(lbl, tip);
    }
    lbl
}

fn attach_grid_row(grid: *mut GtkWidget, row: c_int, key: &CStr, val: *mut GtkWidget) {
    let k_lbl = make_c_label(key, "about-key", false);
    unsafe {
        gtk_grid_attach(grid, k_lbl, 0, row, 1, 1);
        gtk_grid_attach(grid, val, 1, row, 1, 1);
    }
}

fn build_about_header(info: &BuildInfoSnapshot) -> *mut GtkWidget {
    unsafe {
        let box_v = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        let top = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        let title = make_c_label(text::APP_NAME, "about-title", false);
        let ver = CString::new(format!("v{}", info.version)).unwrap_or_default();
        let badge = make_c_label(&ver, "about-version-badge", false);
        let sub = make_c_label(text::about_subtitle(), "about-subtitle", false);
        gtk_box_pack_start(top, title, 0, 0, 0);
        gtk_box_pack_start(top, badge, 0, 0, 0);
        gtk_box_pack_start(box_v, top, 0, 0, 0);
        gtk_box_pack_start(box_v, sub, 0, 0, 0);
        box_v
    }
}

fn build_info_grid(info: &BuildInfoSnapshot) -> *mut GtkWidget {
    let grid = unsafe { gtk_grid_new() };
    add_class(grid, "about-card");
    unsafe {
        gtk_grid_set_row_spacing(grid, 8);
        gtk_grid_set_column_spacing(grid, 16);
    }
    let build_val = format!("{} · {}", info.git_sha, info.target);
    let stack_val = format!("Rust · GTK {} · VTE {}", info.gtk_version, info.vte_version);
    let disp_val = format!("{} · {}", info.backend, info.desktop);
    attach_grid_row(grid, 0, text::about_build_lbl(), make_text_label(&build_val, "about-value", None));
    attach_grid_row(grid, 1, text::about_stack_lbl(), make_text_label(&stack_val, "about-value", None));
    attach_grid_row(grid, 2, text::about_display_lbl(), make_text_label(&disp_val, "about-value", None));
    let cfg_short = shorten_home(&info.config_path);
    let state_short = shorten_home(&info.state_dir);
    attach_grid_row(grid, 3, text::about_config_lbl(), make_text_label(&cfg_short, "about-value", Some(&info.config_path)));
    attach_grid_row(grid, 4, text::about_state_lbl(), make_text_label(&state_short, "about-value", Some(&info.state_dir)));
    attach_grid_row(grid, 5, text::about_license_lbl(), make_c_label(text::about_license_val(), "about-value", false));
    grid
}

fn wire_copy_button(copy_btn: *mut GtkWidget, diagnostic: &str) {
    let payload = Rc::new(CopyPayload { diagnostic: diagnostic.to_owned() });
    let raw = Rc::into_raw(payload) as *mut c_void;
    unsafe {
        g_signal_connect_data(
            copy_btn,
            c"clicked".as_ptr(),
            on_copy_clicked as *const c_void,
            raw,
            drop_copy_payload as *const c_void,
            0,
        );
    }
}

fn wire_close_button(close_btn: *mut GtkWidget, win: *mut GtkWidget) {
    let payload = Box::into_raw(Box::new(ClosePayload { window: win })) as *mut c_void;
    unsafe {
        g_signal_connect_data(
            close_btn,
            c"clicked".as_ptr(),
            on_close_clicked as *const c_void,
            payload,
            drop_close_payload as *const c_void,
            0,
        );
    }
}

fn build_about_footer(win: *mut GtkWidget, diagnostic: &str) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        let copy_btn = gtk_button_new_with_label(text::about_copy_btn().as_ptr());
        let close_btn = gtk_button_new_with_label(text::about_close_btn().as_ptr());
        wire_copy_button(copy_btn, diagnostic);
        wire_close_button(close_btn, win);
        gtk_box_pack_start(row, copy_btn, 0, 0, 0);
        gtk_box_pack_end(row, close_btn, 0, 0, 0);
        (row, copy_btn, close_btn)
    }
}

fn configure_about_window(win: *mut GtkWidget, parent: *mut GtkWidget) {
    add_class(win, "main-bg");
    add_class(win, "about-dialog");
    unsafe {
        gtk_window_set_title(win, text::about_title().as_ptr());
        gtk_window_set_default_size(win, 480, -1);
        gtk_window_set_resizable(win, 0);
        gtk_window_set_modal(win, 1);
        if !parent.is_null() {
            gtk_window_set_transient_for(win, parent);
            gtk_window_set_destroy_with_parent(win, 1);
            gtk_window_set_position(win, GTK_WIN_POS_CENTER_ON_PARENT);
        }
        g_signal_connect_data(
            win,
            c"key-press-event".as_ptr(),
            on_about_key_press as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );
    }
}

pub fn open_about_dialog(parent: *mut GtkWidget) -> AboutDialog {
    let info = collect_build_info();
    let diagnostic_text = format_diagnostic_block(&info);
    let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
    configure_about_window(win, parent);
    let vbox = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 12) };
    let header = build_about_header(&info);
    let grid = build_info_grid(&info);
    let (footer, copy_btn, close_btn) = build_about_footer(win, &diagnostic_text);
    unsafe {
        gtk_box_pack_start(vbox, header, 0, 0, 6);
        gtk_box_pack_start(vbox, grid, 0, 1, 0);
        gtk_box_pack_start(vbox, footer, 0, 1, 4);
        gtk_container_add(win, vbox);
        gtk_widget_show_all(win);
    }
    #[cfg(test)]
    return AboutDialog { window: win, copy_btn, close_btn, diagnostic_text };
    #[cfg(not(test))]
    {
        let _ = (copy_btn, close_btn);
        AboutDialog { window: win }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{set_locale, with_locale_lock, UiLocale};
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

    #[test]
    fn formats_cli_version_and_handles_version_flags() {
        let ver = format_cli_version();
        assert!(ver.starts_with("akhterm "));
        assert!(ver.contains(env!("CARGO_PKG_VERSION")));
        assert!(ver.contains(env!("AKHTERM_GIT_SHA")));
        assert!(ver.contains(env!("AKHTERM_BUILD_TARGET")));

        let args_long = vec!["akhterm".into(), "--version".into()];
        assert_eq!(check_version_arg(&args_long), Some(ver.clone()));
        let args_short = vec!["akhterm".into(), "-V".into()];
        assert_eq!(check_version_arg(&args_short), Some(ver));
        let args_after_dash = vec!["akhterm".into(), "--".into(), "-V".into()];
        assert_eq!(check_version_arg(&args_after_dash), None);
        let args_none = vec!["akhterm".into()];
        assert_eq!(check_version_arg(&args_none), None);
    }

    #[test]
    fn collects_and_formats_build_diagnostic_block() {
        let info = BuildInfoSnapshot {
            version: "1.0.0",
            git_sha: "abc1234",
            target: "x86_64-linux",
            gtk_version: "3.24.41".into(),
            vte_version: "0.76.0".into(),
            backend: "Wayland",
            desktop: "KDE Plasma",
            config_path: "/home/user/.config/akhterm/config.conf".into(),
            state_dir: "/home/user/.local/state/akhterm".into(),
            license: "MIT",
        };
        let block = format_diagnostic_block(&info);
        assert!(block.contains("AkhTerm v1.0.0 (abc1234 x86_64-linux)"));
        assert!(block.contains("GTK 3.24.41 · VTE 0.76.0"));
        assert!(block.contains("Wayland · DE: KDE Plasma"));
        assert!(block.contains("Config: /home/user/.config/akhterm/config.conf"));
        assert!(block.contains("License: MIT"));
    }

    fn read_clipboard_text() -> String {
        unsafe {
            let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
            let raw = gtk_clipboard_wait_for_text(clip);
            assert!(!raw.is_null());
            let out = CStr::from_ptr(raw).to_string_lossy().into_owned();
            g_free(raw as *mut c_void);
            out
        }
    }

    #[test]
    fn headless_broadway_about_dialog_renders_and_copies_build_info() {
        with_locale_lock(|| {
            set_locale(UiLocale::Ru);
            with_headless_gtk(|| unsafe {
                let parent = gtk_window_new(GTK_WINDOW_TOPLEVEL);
                let dlg = open_about_dialog(parent);
                pump_gtk_events();
                assert!(has_class(dlg.window(), "about-dialog"));
                let copy_lbl = CStr::from_ptr(gtk_button_get_label(dlg.copy_button())).to_str().unwrap();
                assert_eq!(copy_lbl, "Скопировать данные сборки");
                gtk_button_clicked(dlg.copy_button());
                pump_gtk_events();
                let copied_lbl = CStr::from_ptr(gtk_button_get_label(dlg.copy_button())).to_str().unwrap();
                assert_eq!(copied_lbl, "Скопировано ✓");
                let clip = read_clipboard_text();
                assert_eq!(clip, dlg.diagnostic_text());
                assert!(clip.contains(env!("AKHTERM_GIT_SHA")));
                gtk_button_clicked(dlg.close_button());
                pump_gtk_events();
                gtk_widget_destroy(parent);
                pump_gtk_events();
            });
        });
    }

    #[test]
    fn about_module_achieves_target_mi() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("about.rs"));
        assert!(mi >= 60.0, "about.rs MI {mi:.2} < 60.0");
    }
}
