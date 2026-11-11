//! OS, Wayland/X11 display-server, and desktop-environment adapter layer.

use std::ffi::{c_int, c_void, CStr, OsStr};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::Once;

use crate::ffi::*;
use crate::session::SessionCategory;
use crate::ui_text as text;

pub const APP_DESKTOP_ID: &CStr = c"org.akhterm.AkhTerm";

const PROJECT_ICONS: &[&CStr] = &[c"utilities-terminal-symbolic", c"utilities-terminal"];
const REMOTE_ICONS: &[&CStr] = &[
    c"network-server-symbolic",
    c"network-server",
    c"folder-remote-symbolic",
];
const DAEMON_ICONS: &[&CStr] = &[
    c"computer-symbolic",
    c"utilities-system-monitor-symbolic",
    c"utilities-system-monitor",
];

const OPENERS: &[(&str, &[&str])] = &[
    ("xdg-open", &[]),
    ("gio", &["open"]),
    ("kioclient5", &["exec"]),
    ("kde-open5", &[]),
];

pub const STRIPPED_PROXY_VARS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

static APP_NAME_ONCE: Once = Once::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayBackend {
    Wayland,
    X11,
    Broadway,
    Unknown,
}

impl DisplayBackend {
    pub fn css_class(self) -> Option<&'static str> {
        match self {
            Self::Wayland => Some("backend-wayland"),
            Self::X11 => Some("backend-x11"),
            Self::Broadway => Some("backend-broadway"),
            Self::Unknown => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopEnvironment {
    KdePlasma,
    Gnome,
    Wlroots,
    Xfce,
    Generic,
}

impl DesktopEnvironment {
    pub fn css_class(self) -> &'static str {
        match self {
            Self::KdePlasma => "de-kde",
            Self::Gnome => "de-gnome",
            Self::Wlroots => "de-wlroots",
            Self::Xfce => "de-xfce",
            Self::Generic => "de-generic",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenerCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

fn has_non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty())
}

fn parse_backend_token(raw: &str) -> Option<DisplayBackend> {
    let tok = raw.trim();
    if tok.eq_ignore_ascii_case("wayland") {
        Some(DisplayBackend::Wayland)
    } else if tok.eq_ignore_ascii_case("x11") {
        Some(DisplayBackend::X11)
    } else if tok.eq_ignore_ascii_case("broadway") {
        Some(DisplayBackend::Broadway)
    } else {
        None
    }
}

fn resolve_gdk_backend_override(
    raw_gdk: &str,
    has_wayland: bool,
    has_x11: bool,
) -> Option<DisplayBackend> {
    let parsed: Vec<DisplayBackend> = raw_gdk.split(',').filter_map(parse_backend_token).collect();
    if parsed.len() == 1 {
        return Some(parsed[0]);
    }
    parsed
        .iter()
        .copied()
        .find(|&b| match b {
            DisplayBackend::Wayland => has_wayland,
            DisplayBackend::X11 => has_x11,
            DisplayBackend::Broadway => true,
            DisplayBackend::Unknown => false,
        })
        .or_else(|| parsed.first().copied())
}

pub fn detect_display_backend_env(
    gdk_backend: Option<&str>,
    wayland_display: Option<&str>,
    xdg_session_type: Option<&str>,
    x11_display: Option<&str>,
) -> DisplayBackend {
    let has_wayland = has_non_empty(wayland_display);
    let has_x11 = has_non_empty(x11_display);
    if let Some(forced) = gdk_backend.and_then(|g| resolve_gdk_backend_override(g, has_wayland, has_x11)) {
        return forced;
    }
    if has_wayland {
        return DisplayBackend::Wayland;
    }
    if let Some(from_xdg) = xdg_session_type.and_then(parse_backend_token) {
        return from_xdg;
    }
    if has_x11 {
        DisplayBackend::X11
    } else {
        DisplayBackend::Unknown
    }
}

pub fn classify_gdk_display_type(type_name: &str) -> Option<DisplayBackend> {
    match type_name.trim() {
        "GdkWaylandDisplay" => Some(DisplayBackend::Wayland),
        "GdkX11Display" => Some(DisplayBackend::X11),
        "GdkBroadwayDisplay" => Some(DisplayBackend::Broadway),
        other if other.contains("Wayland") => Some(DisplayBackend::Wayland),
        other if other.contains("X11") => Some(DisplayBackend::X11),
        other if other.contains("Broadway") => Some(DisplayBackend::Broadway),
        _ => None,
    }
}

fn inspect_gdk_display_backend() -> Option<DisplayBackend> {
    let display = unsafe { gdk_display_get_default() };
    if display.is_null() {
        return None;
    }
    let name_ptr = unsafe { g_type_name_from_instance(display as *mut c_void) };
    if name_ptr.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name_ptr).to_string_lossy() };
    classify_gdk_display_type(&name)
}

fn detect_display_backend_from_process_env() -> DisplayBackend {
    let gdk = std::env::var("GDK_BACKEND").ok();
    let wayland = std::env::var("WAYLAND_DISPLAY").ok();
    let xdg = std::env::var("XDG_SESSION_TYPE").ok();
    let x11 = std::env::var("DISPLAY").ok();
    detect_display_backend_env(gdk.as_deref(), wayland.as_deref(), xdg.as_deref(), x11.as_deref())
}

pub fn detect_display_backend() -> DisplayBackend {
    inspect_gdk_display_backend().unwrap_or_else(detect_display_backend_from_process_env)
}

fn classify_desktop_token(token: &str) -> Option<DesktopEnvironment> {
    let low = token.trim().to_ascii_lowercase();
    if low.is_empty() {
        None
    } else if low == "kde" || low.contains("plasma") {
        Some(DesktopEnvironment::KdePlasma)
    } else if low.contains("gnome") {
        Some(DesktopEnvironment::Gnome)
    } else if low.contains("sway") || low.contains("hyprland") || low.contains("wlroots") {
        Some(DesktopEnvironment::Wlroots)
    } else if low.contains("xfce") {
        Some(DesktopEnvironment::Xfce)
    } else {
        None
    }
}

fn parse_desktop_list(raw: Option<&str>) -> Option<DesktopEnvironment> {
    raw?.split([':', ';']).find_map(classify_desktop_token)
}

pub fn detect_desktop_env(
    xdg_current: Option<&str>,
    xdg_session: Option<&str>,
    kde_full: Option<&str>,
) -> DesktopEnvironment {
    if let Some(de) = parse_desktop_list(xdg_current).or_else(|| parse_desktop_list(xdg_session)) {
        return de;
    }
    if kde_full.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1")) {
        DesktopEnvironment::KdePlasma
    } else {
        DesktopEnvironment::Generic
    }
}

pub fn detect_desktop_environment() -> DesktopEnvironment {
    let current = std::env::var("XDG_CURRENT_DESKTOP").ok();
    let session = std::env::var("XDG_SESSION_DESKTOP").ok();
    let kde = std::env::var("KDE_FULL_SESSION").ok();
    detect_desktop_env(current.as_deref(), session.as_deref(), kde.as_deref())
}

pub fn init_process_identity() {
    unsafe { g_set_prgname(APP_DESKTOP_ID.as_ptr()) };
    APP_NAME_ONCE.call_once(|| unsafe {
        g_set_application_name(text::APP_NAME.as_ptr());
    });
}

pub fn should_set_x11_wmclass(backend: DisplayBackend) -> bool {
    backend == DisplayBackend::X11
}

fn apply_window_platform_classes(
    win: *mut GtkWidget,
    backend: DisplayBackend,
    desktop: DesktopEnvironment,
) {
    let active_backend = backend.css_class();
    for cls in ["backend-wayland", "backend-x11", "backend-broadway"] {
        set_class(win, cls, active_backend == Some(cls));
    }
    let active_de = desktop.css_class();
    for cls in ["de-kde", "de-gnome", "de-wlroots", "de-xfce", "de-generic"] {
        set_class(win, cls, active_de == cls);
    }
}

pub fn configure_window_identity(
    win: *mut GtkWidget,
    backend: DisplayBackend,
    desktop: DesktopEnvironment,
) {
    if win.is_null() {
        return;
    }
    unsafe { gtk_window_set_default_icon_name(APP_DESKTOP_ID.as_ptr()) };
    if should_set_x11_wmclass(backend) {
        unsafe { gtk_window_set_wmclass(win, text::APP_NAME.as_ptr(), text::APP_NAME.as_ptr()) };
    }
    apply_window_platform_classes(win, backend, desktop);
}

pub fn translate_widget_x_to_ancestor(
    source_widget: *mut GtkWidget,
    ancestor_widget: *mut GtkWidget,
    local_x: f64,
) -> Option<f64> {
    if source_widget.is_null() || ancestor_widget.is_null() {
        return None;
    }
    let (mut ox, mut oy): (c_int, c_int) = (0, 0);
    let ok = unsafe {
        gtk_widget_translate_coordinates(source_widget, ancestor_widget, 0, 0, &mut ox, &mut oy)
    };
    (ok != 0).then(|| f64::from(ox) + local_x)
}

pub fn translate_event_x_to_ancestor(
    source_widget: *mut GtkWidget,
    ancestor_widget: *mut GtkWidget,
    event: *const GdkEvent,
) -> Option<f64> {
    if event.is_null() {
        return None;
    }
    let (mut ex, mut ey) = (0.0, 0.0);
    if unsafe { gdk_event_get_coords(event, &mut ex, &mut ey) } == 0 {
        return None;
    }
    translate_widget_x_to_ancestor(source_widget, ancestor_widget, ex)
}

pub fn category_icon_candidates(category: SessionCategory) -> &'static [&'static CStr] {
    match category {
        SessionCategory::Project => PROJECT_ICONS,
        SessionCategory::Remote => REMOTE_ICONS,
        SessionCategory::Daemon => DAEMON_ICONS,
    }
}

pub fn pick_category_icon(
    category: SessionCategory,
    mut has_icon: impl FnMut(&'static CStr) -> bool,
) -> &'static CStr {
    category_icon_candidates(category)
        .iter()
        .copied()
        .find(|&candidate| has_icon(candidate))
        .unwrap_or_else(|| category.default_icon_name())
}

fn default_icon_theme() -> *mut GtkIconTheme {
    unsafe {
        if gdk_screen_get_default().is_null() {
            ptr::null_mut()
        } else {
            gtk_icon_theme_get_default()
        }
    }
}

pub fn resolve_category_icon(category: SessionCategory) -> &'static CStr {
    let theme = default_icon_theme();
    if theme.is_null() {
        return category.default_icon_name();
    }
    pick_category_icon(category, |icon| unsafe {
        gtk_icon_theme_has_icon(theme, icon.as_ptr()) != 0
    })
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111) != 0)
}

pub fn find_in_path(binary: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let raw_path = path_env?;
    std::env::split_paths(raw_path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable_file(candidate))
}

pub fn resolve_opener_in_path(target: &str, path_env: Option<&OsStr>) -> Option<OpenerCommand> {
    OPENERS.iter().find_map(|&(bin, sub_args)| {
        let program = find_in_path(bin, path_env)?;
        let mut args: Vec<String> = sub_args.iter().map(|s| (*s).to_owned()).collect();
        args.push(target.to_owned());
        Some(OpenerCommand { program, args })
    })
}

pub fn build_opener_command(target: &str, path_env: Option<&OsStr>) -> Command {
    let spec = resolve_opener_in_path(target, path_env).unwrap_or_else(|| OpenerCommand {
        program: PathBuf::from("xdg-open"),
        args: vec![target.to_owned()],
    });
    let mut cmd = Command::new(spec.program);
    cmd.args(spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for &key in STRIPPED_PROXY_VARS {
        cmd.env_remove(key);
    }
    cmd
}

pub fn spawn_desktop_opener(target: &str) -> bool {
    let path_env = std::env::var_os("PATH");
    build_opener_command(target, path_env.as_deref()).spawn().is_ok()
}

pub fn notify_command_args(summary: &str, body: &str) -> [String; 5] {
    [
        "--app-name=AkhTerm".to_owned(),
        "--icon=org.akhterm.AkhTerm".to_owned(),
        "--".to_owned(),
        summary.to_owned(),
        body.to_owned(),
    ]
}

pub fn build_notify_command(program: &str, summary: &str, body: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(notify_command_args(summary, body))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

pub fn is_clipboard_image_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::thread;
    use std::time::Duration;

    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

    #[repr(C)]
    struct RawGdkEventCoords {
        type_: c_int,
        window: *mut c_void,
        send_event: i8,
        time: u32,
        x: f64,
        y: f64,
    }

    fn clean_test_dir(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
        let _ = fs::remove_dir(dir);
    }

    fn write_mock_exec(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn flush_gtk_layout() {
        thread::sleep(Duration::from_millis(25));
        pump_gtk_events();
    }

    #[test]
    fn detects_display_backend_from_env_matrix() {
        assert_eq!(detect_display_backend_env(None, Some("wayland-0"), Some("wayland"), Some(":0")), DisplayBackend::Wayland);
        assert_eq!(detect_display_backend_env(Some("x11"), Some("wayland-0"), Some("wayland"), Some(":0")), DisplayBackend::X11);
        assert_eq!(detect_display_backend_env(Some("broadway"), Some("wayland-0"), None, Some(":0")), DisplayBackend::Broadway);
        assert_eq!(detect_display_backend_env(Some("wayland,x11"), None, Some("x11"), Some(":1")), DisplayBackend::X11);
        assert_eq!(detect_display_backend_env(None, None, Some("x11"), Some(":0")), DisplayBackend::X11);
        assert_eq!(detect_display_backend_env(None, None, Some("tty"), None), DisplayBackend::Unknown);
    }

    #[test]
    fn classifies_gdk_display_type_names() {
        assert_eq!(classify_gdk_display_type("GdkWaylandDisplay"), Some(DisplayBackend::Wayland));
        assert_eq!(classify_gdk_display_type("GdkX11Display"), Some(DisplayBackend::X11));
        assert_eq!(classify_gdk_display_type("GdkBroadwayDisplay"), Some(DisplayBackend::Broadway));
        assert_eq!(classify_gdk_display_type("GdkQuartzDisplay"), None);
    }

    #[test]
    fn detects_desktop_environments_across_kde_gnome_wlroots_xfce() {
        assert_eq!(detect_desktop_env(Some("KDE"), None, None), DesktopEnvironment::KdePlasma);
        assert_eq!(detect_desktop_env(None, Some("plasma"), None), DesktopEnvironment::KdePlasma);
        assert_eq!(detect_desktop_env(None, None, Some("true")), DesktopEnvironment::KdePlasma);
        assert_eq!(detect_desktop_env(Some("ubuntu:GNOME"), Some("ubuntu"), None), DesktopEnvironment::Gnome);
        assert_eq!(detect_desktop_env(Some("sway"), None, None), DesktopEnvironment::Wlroots);
        assert_eq!(detect_desktop_env(Some("Hyprland"), None, None), DesktopEnvironment::Wlroots);
        assert_eq!(detect_desktop_env(Some("XFCE"), None, None), DesktopEnvironment::Xfce);
        assert_eq!(detect_desktop_env(None, None, Some("false")), DesktopEnvironment::Generic);
        assert!(should_set_x11_wmclass(DisplayBackend::X11) && !should_set_x11_wmclass(DisplayBackend::Wayland));
    }

    #[test]
    fn picks_category_icon_with_breeze_and_headless_fallbacks() {
        assert_eq!(pick_category_icon(SessionCategory::Project, |c| c == c"utilities-terminal-symbolic"), c"utilities-terminal-symbolic");
        assert_eq!(pick_category_icon(SessionCategory::Project, |c| c == c"utilities-terminal"), c"utilities-terminal");
        assert_eq!(pick_category_icon(SessionCategory::Remote, |c| c == c"folder-remote-symbolic"), c"folder-remote-symbolic");
        assert_eq!(pick_category_icon(SessionCategory::Daemon, |c| c == c"utilities-system-monitor"), c"utilities-system-monitor");
        assert_eq!(pick_category_icon(SessionCategory::Remote, |_| false), c"network-server-symbolic");
    }

    fn assert_opener_step(dir: &Path, bin: &str, url: &str, expected_args: &[&str]) {
        let exe = write_mock_exec(dir, bin);
        let path_os = OsString::from(dir.as_os_str());
        let spec = resolve_opener_in_path(url, Some(&path_os)).unwrap();
        let exp: Vec<String> = expected_args.iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(spec, OpenerCommand { program: exe, args: exp });
    }

    #[test]
    fn resolves_opener_fallback_chain_and_strips_proxy_env() {
        let dir = std::env::temp_dir().join(format!("akhterm_opener_{}", std::process::id()));
        clean_test_dir(&dir);
        fs::create_dir_all(&dir).unwrap();
        let url = "https://example.com/docs";
        assert_opener_step(&dir, "kde-open5", url, &[url]);
        assert_opener_step(&dir, "kioclient5", url, &["exec", url]);
        assert_opener_step(&dir, "gio", url, &["open", url]);
        assert_opener_step(&dir, "xdg-open", url, &[url]);

        let cmd = build_opener_command(url, Some(&OsString::from(dir.as_os_str())));
        let envs: Vec<(&OsStr, Option<&OsStr>)> = cmd.get_envs().collect();
        assert!(STRIPPED_PROXY_VARS.iter().all(|k| envs.contains(&(OsStr::new(k), None))));
        clean_test_dir(&dir);
    }

    #[test]
    fn builds_notify_command_with_akhterm_app_name_and_desktop_icon() {
        let cmd = build_notify_command("notify-send", "AkhTerm · Готово", "Проект ЦУ");
        assert_eq!(cmd.get_program(), OsStr::new("notify-send"));
        let args: Vec<&OsStr> = cmd.get_args().collect();
        let exp = ["--app-name=AkhTerm", "--icon=org.akhterm.AkhTerm", "--", "AkhTerm · Готово", "Проект ЦУ"];
        assert_eq!(args, exp.map(OsStr::new));
        assert!(is_clipboard_image_ext("bmp") && is_clipboard_image_ext("WEBP") && !is_clipboard_image_ext("txt"));
    }

    fn verify_broadway_identity_and_classes() {
        init_process_identity();
        init_process_identity();
        let prg = unsafe { CStr::from_ptr(g_get_prgname()).to_string_lossy().into_owned() };
        let app = unsafe { CStr::from_ptr(g_get_application_name()).to_string_lossy().into_owned() };
        assert_eq!((prg.as_str(), app.as_str()), ("org.akhterm.AkhTerm", "AkhTerm"));
        assert_eq!(detect_display_backend(), DisplayBackend::Broadway);

        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        configure_window_identity(win, DisplayBackend::Wayland, DesktopEnvironment::KdePlasma);
        let icon = unsafe { CStr::from_ptr(gtk_window_get_default_icon_name()).to_string_lossy().into_owned() };
        assert_eq!(icon, "org.akhterm.AkhTerm");
        assert!(has_class(win, "backend-wayland") && has_class(win, "de-kde") && !has_class(win, "backend-x11"));
        configure_window_identity(win, DisplayBackend::X11, DesktopEnvironment::Gnome);
        assert!(has_class(win, "backend-x11") && has_class(win, "de-gnome") && !has_class(win, "backend-wayland"));
        unsafe { gtk_widget_destroy(win) };
    }

    fn build_two_button_strip() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            let strip = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
            let b1 = gtk_button_new();
            let b2 = gtk_button_new();
            gtk_window_set_default_size(win, 400, 80);
            gtk_widget_set_size_request(b1, 140, 28);
            gtk_widget_set_size_request(b2, 90, 28);
            gtk_box_pack_start(strip, b1, 0, 0, 0);
            gtk_box_pack_start(strip, b2, 0, 0, 0);
            gtk_container_add(win, strip);
            gtk_widget_show_all(win);
            flush_gtk_layout();
            (win, strip, b2)
        }
    }

    fn verify_broadway_coordinate_translation() {
        let (win, strip, b2) = build_two_button_strip();
        let ev = unsafe { gdk_event_new(GDK_MOTION_NOTIFY) };
        unsafe { (*(ev as *mut RawGdkEventCoords)).x = 18.5 };
        let b2_x = translate_event_x_to_ancestor(b2, strip, ev).expect("translated x");
        assert!((b2_x - 162.5).abs() < 1.0, "expected ~162.5, got {b2_x}");
        unsafe { gtk_box_reorder_child(strip, b2, 0) };
        flush_gtk_layout();
        let b2_after = translate_event_x_to_ancestor(b2, strip, ev).expect("reordered x");
        assert!((b2_after - 18.5).abs() < 1.0, "expected ~18.5, got {b2_after}");
        unsafe {
            gdk_event_free(ev);
            gtk_widget_destroy(win);
        }
        pump_gtk_events();
    }

    #[test]
    fn headless_broadway_verifies_identity_css_and_wayland_safe_coords() {
        with_headless_gtk(|| {
            verify_broadway_identity_and_classes();
            verify_broadway_coordinate_translation();
            assert_eq!(resolve_category_icon(SessionCategory::Project), c"utilities-terminal-symbolic");
        });
    }

    #[test]
    fn platform_module_maintains_grade_b_mi_target() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("platform.rs"));
        assert!(mi >= 60.0, "platform.rs MI {mi:.2} < 60.0");
    }
}
