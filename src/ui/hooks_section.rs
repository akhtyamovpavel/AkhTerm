//! Compact Agent Hooks installer section embedded in the Settings dialog.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::PathBuf;
use std::rc::Rc;

use crate::ffi::{add_class, g_signal_connect_data, set_label_text, GtkWidget};
use crate::hook_installer::{
    inspect_all, install_all_hooks, install_hooks, uninstall_hooks, AgentHookTarget,
    HookInstallStatus,
};
use crate::ui_text;

const GTK_ORIENTATION_HORIZONTAL: c_int = 0;
const GTK_ORIENTATION_VERTICAL: c_int = 1;
const GTK_ALIGN_START: c_int = 1;
const GTK_ALIGN_CENTER: c_int = 3;

#[link(name = "gtk-3")]
unsafe extern "C" {
    fn gtk_box_new(orientation: c_int, spacing: c_int) -> *mut GtkWidget;
    fn gtk_box_pack_start(bx: *mut GtkWidget, child: *mut GtkWidget, expand: c_int, fill: c_int, pad: c_uint);
    fn gtk_box_pack_end(bx: *mut GtkWidget, child: *mut GtkWidget, expand: c_int, fill: c_int, pad: c_uint);
    fn gtk_label_new(text: *const c_char) -> *mut GtkWidget;
    fn gtk_label_set_line_wrap(label: *mut GtkWidget, wrap: c_int);
    fn gtk_button_new_with_label(label: *const c_char) -> *mut GtkWidget;
    fn gtk_button_set_label(button: *mut GtkWidget, label: *const c_char);
    fn gtk_widget_set_hexpand(widget: *mut GtkWidget, expand: c_int);
    fn gtk_widget_set_halign(widget: *mut GtkWidget, align: c_int);
    fn gtk_widget_set_valign(widget: *mut GtkWidget, align: c_int);
    fn gtk_widget_set_sensitive(widget: *mut GtkWidget, sensitive: c_int);
}

struct HookRowWidgets {
    target: AgentHookTarget,
    status_lbl: *mut GtkWidget,
    install_btn: *mut GtkWidget,
    remove_btn: *mut GtkWidget,
}

struct HooksSectionState {
    root: *mut GtkWidget,
    title_lbl: *mut GtkWidget,
    subtitle_lbl: *mut GtkWidget,
    install_all_btn: *mut GtkWidget,
    feedback_lbl: *mut GtkWidget,
    rows: Vec<HookRowWidgets>,
    home_dir: PathBuf,
}

#[derive(Clone)]
pub(crate) struct HooksSection {
    state: Rc<HooksSectionState>,
}

impl HooksSection {
    pub(crate) fn widget(&self) -> *mut GtkWidget {
        self.state.root
    }

    pub(crate) fn refresh_labels(&self) {
        refresh_section(&self.state);
    }
}

fn make_lbl(text: &CStr, class_name: &str) -> *mut GtkWidget {
    let lbl = unsafe { gtk_label_new(text.as_ptr()) };
    unsafe {
        gtk_widget_set_halign(lbl, GTK_ALIGN_START);
        gtk_widget_set_valign(lbl, GTK_ALIGN_CENTER);
        add_class(lbl, class_name);
    }
    lbl
}

fn make_btn(label: &CStr) -> *mut GtkWidget {
    let btn = unsafe { gtk_button_new_with_label(label.as_ptr()) };
    add_class(btn, "settings-preset-btn");
    btn
}

fn build_target_row(target: AgentHookTarget) -> (*mut GtkWidget, HookRowWidgets) {
    let row = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let name_c = CString::new(target.label()).unwrap_or_default();
    let name_lbl = make_lbl(&name_c, "settings-row-title");
    let status_lbl = make_lbl(ui_text::settings_hooks_not_installed(), "settings-row-hint");
    let install_btn = make_btn(ui_text::settings_hooks_install());
    let remove_btn = make_btn(ui_text::settings_hooks_remove());
    unsafe {
        gtk_widget_set_hexpand(status_lbl, 1);
        gtk_box_pack_start(row, name_lbl, 0, 0, 0);
        gtk_box_pack_start(row, status_lbl, 1, 1, 4);
        gtk_box_pack_end(row, remove_btn, 0, 0, 0);
        gtk_box_pack_end(row, install_btn, 0, 0, 0);
    }
    (row, HookRowWidgets { target, status_lbl, install_btn, remove_btn })
}

fn build_hooks_header() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    let hdr = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
    let title_lbl = make_lbl(ui_text::settings_hooks_title(), "settings-row-title");
    let install_all_btn = make_btn(ui_text::settings_hooks_install_all());
    unsafe {
        gtk_box_pack_start(hdr, title_lbl, 1, 1, 0);
        gtk_box_pack_end(hdr, install_all_btn, 0, 0, 0);
    }
    (hdr, title_lbl, install_all_btn)
}

fn build_all_rows(root: *mut GtkWidget) -> Vec<HookRowWidgets> {
    let mut rows = Vec::with_capacity(3);
    for target in AgentHookTarget::ALL {
        let (row_box, rw) = build_target_row(target);
        unsafe { gtk_box_pack_start(root, row_box, 0, 0, 0) };
        rows.push(rw);
    }
    rows
}

pub(crate) fn build_hooks_section(home_dir: PathBuf) -> HooksSection {
    let root = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 6) };
    add_class(root, "settings-card");
    let (hdr, title_lbl, install_all_btn) = build_hooks_header();
    let subtitle_lbl = make_lbl(ui_text::settings_hooks_subtitle(), "settings-row-hint");
    let feedback_lbl = make_lbl(c"", "settings-row-hint");
    unsafe {
        gtk_label_set_line_wrap(subtitle_lbl, 1);
        gtk_label_set_line_wrap(feedback_lbl, 1);
        gtk_box_pack_start(root, hdr, 0, 0, 0);
        gtk_box_pack_start(root, subtitle_lbl, 0, 0, 0);
    }
    let rows = build_all_rows(root);
    unsafe { gtk_box_pack_start(root, feedback_lbl, 0, 0, 0) };
    let state = Rc::new(HooksSectionState {
        root, title_lbl, subtitle_lbl, install_all_btn, feedback_lbl, rows, home_dir,
    });
    connect_signals(&state);
    refresh_section(&state);
    HooksSection { state }
}

fn status_cstr(status: HookInstallStatus) -> &'static CStr {
    match status {
        HookInstallStatus::Installed => ui_text::settings_hooks_installed(),
        HookInstallStatus::NotInstalled => ui_text::settings_hooks_not_installed(),
        HookInstallStatus::Outdated => ui_text::settings_hooks_outdated(),
    }
}

fn refresh_row(row: &HookRowWidgets, status: HookInstallStatus) {
    let install_lbl = if status == HookInstallStatus::Outdated {
        ui_text::settings_hooks_update()
    } else {
        ui_text::settings_hooks_install()
    };
    set_label_text(row.status_lbl, status_cstr(status).to_str().unwrap_or_default());
    unsafe {
        gtk_button_set_label(row.install_btn, install_lbl.as_ptr());
        gtk_button_set_label(row.remove_btn, ui_text::settings_hooks_remove().as_ptr());
        gtk_widget_set_sensitive(row.remove_btn, i32::from(status != HookInstallStatus::NotInstalled));
    }
}

fn refresh_section(state: &HooksSectionState) {
    set_label_text(state.title_lbl, ui_text::settings_hooks_title().to_str().unwrap_or_default());
    set_label_text(state.subtitle_lbl, ui_text::settings_hooks_subtitle().to_str().unwrap_or_default());
    unsafe { gtk_button_set_label(state.install_all_btn, ui_text::settings_hooks_install_all().as_ptr()) };
    let statuses = inspect_all(&state.home_dir);
    for (row, (_, status)) in state.rows.iter().zip(statuses) {
        refresh_row(row, status);
    }
}

struct RowPayload {
    state: Rc<HooksSectionState>,
    target: AgentHookTarget,
    install: bool,
}

fn connect_row_btn(btn: *mut GtkWidget, state: &Rc<HooksSectionState>, target: AgentHookTarget, install: bool) {
    let payload = Box::into_raw(Box::new(RowPayload { state: Rc::clone(state), target, install }));
    unsafe {
        g_signal_connect_data(
            btn as *mut c_void,
            c"clicked".as_ptr(),
            on_row_action as *const c_void,
            payload as *mut c_void,
            drop_row_payload as *const c_void,
            0,
        );
    }
}

fn connect_signals(state: &Rc<HooksSectionState>) {
    let all_ptr = Rc::into_raw(Rc::clone(state)) as *mut c_void;
    unsafe {
        g_signal_connect_data(
            state.install_all_btn as *mut c_void,
            c"clicked".as_ptr(),
            on_install_all_clicked as *const c_void,
            all_ptr,
            drop_rc_state as *const c_void,
            0,
        );
    }
    for row in &state.rows {
        connect_row_btn(row.install_btn, state, row.target, true);
        connect_row_btn(row.remove_btn, state, row.target, false);
    }
}

fn handle_row_action(payload: &RowPayload) {
    let res = if payload.install {
        install_hooks(&payload.state.home_dir, payload.target)
    } else {
        uninstall_hooks(&payload.state.home_dir, payload.target)
    };
    let msg = res.map_or_else(|e| e, |r| r.detail);
    set_label_text(payload.state.feedback_lbl, &msg);
    refresh_section(&payload.state);
}

fn handle_install_all(state: &HooksSectionState) {
    let msg = match install_all_hooks(&state.home_dir) {
        Ok(reports) => {
            let changed = reports.iter().filter(|r| r.changed).count();
            format!("{} ({changed}/3)", ui_text::settings_hooks_installed().to_str().unwrap_or_default())
        }
        Err(err) => err,
    };
    set_label_text(state.feedback_lbl, &msg);
    refresh_section(state);
}

unsafe extern "C" fn on_row_action(_btn: *mut GtkWidget, data: *mut c_void) {
    if !data.is_null() {
        handle_row_action(unsafe { &*(data as *const RowPayload) });
    }
}

unsafe extern "C" fn on_install_all_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if !data.is_null() {
        handle_install_all(unsafe { &*(data as *const HooksSectionState) });
    }
}

unsafe extern "C" fn drop_row_payload(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data as *mut RowPayload)) };
    }
}

unsafe extern "C" fn drop_rc_state(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Rc::from_raw(data as *const HooksSectionState)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{set_locale, with_locale_lock, UiLocale};
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

    unsafe extern "C" {
        fn gtk_button_clicked(btn: *mut GtkWidget);
        fn gtk_label_get_text(lbl: *mut GtkWidget) -> *const c_char;
        fn gtk_widget_destroy(widget: *mut GtkWidget);
    }

    fn lbl_text(lbl: *mut GtkWidget) -> String {
        unsafe { CStr::from_ptr(gtk_label_get_text(lbl)).to_string_lossy().into_owned() }
    }

    fn drive_hooks_ui(sec: &HooksSection) {
        assert_eq!(lbl_text(sec.state.rows[0].status_lbl), "Not installed");
        unsafe { gtk_button_clicked(sec.state.install_all_btn) };
        pump_gtk_events();
        for row in &sec.state.rows {
            assert_eq!(lbl_text(row.status_lbl), "Installed");
        }
        unsafe { gtk_button_clicked(sec.state.rows[0].remove_btn) };
        pump_gtk_events();
        assert_eq!(lbl_text(sec.state.rows[0].status_lbl), "Not installed");
    }

    fn verify_ru_refresh(sec: &HooksSection) {
        set_locale(UiLocale::Ru);
        sec.refresh_labels();
        assert_eq!(lbl_text(sec.state.title_lbl), "Хуки агентов");
        assert_eq!(lbl_text(sec.state.rows[0].status_lbl), "Не установлено");
        assert_eq!(lbl_text(sec.state.rows[1].status_lbl), "Установлено");
    }

    #[test]
    fn hooks_section_installs_and_removes_in_isolated_temp_home() {
        with_locale_lock(|| {
            set_locale(UiLocale::En);
            let home = std::env::temp_dir().join(format!("akhterm-hooks-ui-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(&home).unwrap();
            with_headless_gtk(|| {
                let sec = build_hooks_section(home.clone());
                drive_hooks_ui(&sec);
                verify_ru_refresh(&sec);
                unsafe { gtk_widget_destroy(sec.widget()) };
                pump_gtk_events();
            });
            set_locale(UiLocale::Auto);
            let _ = std::fs::remove_dir_all(&home);
        });
    }
}
