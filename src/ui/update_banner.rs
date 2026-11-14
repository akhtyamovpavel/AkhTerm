//! Non-modal bottom-right floating update notification card (`GtkOverlay` child).

use std::cell::RefCell;
use std::ffi::{c_int, c_void, CStr};
use std::ptr;
use std::rc::Rc;

use crate::ffi::*;
use crate::ui_text as text;

const CARD_WIDTH_PX: c_int = 380;
const CARD_MARGIN_PX: c_int = 16;
const STATUS_MAX_WIDTH_CHARS: c_int = 42;

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateBannerModel {
    #[default]
    Hidden,
    Available { current_version: String, latest_version: String, release_url: String },
    Downloading { current_version: String, latest_version: String, release_url: String },
    Installing { current_version: String, latest_version: String, release_url: String },
    ReadyToRestart { current_version: String, latest_version: String, release_url: String },
    Failed { current_version: String, latest_version: Option<String>, release_url: String, error: String },
}

pub(crate) fn format_version_transition(current: &str, latest: Option<&str>, is_ru: bool) -> String {
    let cur = current.trim().trim_start_matches('v');
    let next = latest.map(|v| v.trim().trim_start_matches('v')).filter(|v| !v.is_empty());
    match next {
        Some(new_ver) if is_ru => format!("с v{cur} до {new_ver}"),
        Some(new_ver) => format!("v{cur} to {new_ver}"),
        None => format!("v{cur}"),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone)]
pub struct UpdateBannerCallbacks {
    pub on_update: Rc<dyn Fn()>, pub on_restart: Rc<dyn Fn()>,
    pub on_dismiss: Rc<dyn Fn()>, pub on_whats_new: Rc<dyn Fn(&str)>,
}

struct BannerInner {
    callbacks: UpdateBannerCallbacks, last_model: RefCell<UpdateBannerModel>,
    card: *mut GtkWidget, badge_lbl: *mut GtkWidget, author_lbl: *mut GtkWidget,
    version_lbl: *mut GtkWidget, close_btn: *mut GtkWidget, spinner: *mut GtkWidget,
    status_lbl: *mut GtkWidget, whats_new_btn: *mut GtkWidget, dismiss_btn: *mut GtkWidget,
    primary_btn: *mut GtkWidget,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone)]
pub struct UpdateBanner {
    inner: Rc<BannerInner>,
}

fn make_lbl(initial: &CStr, class: &str, xalign: f32) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(initial.as_ptr());
        gtk_label_set_xalign(lbl, xalign);
        add_class(lbl, class);
        lbl
    }
}

fn model_meta(model: &UpdateBannerModel) -> (&str, Option<&str>, &str) {
    match model {
        UpdateBannerModel::Hidden => ("", None, ""),
        UpdateBannerModel::Available { current_version, latest_version, release_url }
        | UpdateBannerModel::Downloading { current_version, latest_version, release_url }
        | UpdateBannerModel::Installing { current_version, latest_version, release_url }
        | UpdateBannerModel::ReadyToRestart { current_version, latest_version, release_url } => {
            (current_version, Some(latest_version.as_str()), release_url)
        }
        UpdateBannerModel::Failed { current_version, latest_version, release_url, .. } => {
            (current_version, latest_version.as_deref(), release_url)
        }
    }
}

fn make_btn(initial: &CStr, class: &str) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new_with_label(initial.as_ptr());
        gtk_widget_set_can_focus(btn, 0);
        add_class(btn, class);
        btn
    }
}

fn build_title_line() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let top = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let title = make_lbl(text::APP_NAME, "update-card-title", 0.0);
        let author = make_lbl(text::update_author(), "update-card-author", 0.0);
        let badge = make_lbl(text::update_badge(), "update-card-pill", 0.5);
        gtk_widget_set_valign(badge, GTK_ALIGN_CENTER);
        for w in [title, author, badge] {
            gtk_box_pack_start(top, w, 0, 0, 0);
        }
        (top, badge, author)
    }
}

fn build_header_row() -> (*mut GtkWidget, [*mut GtkWidget; 4]) {
    unsafe {
        let (row, meta) = (gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10), gtk_box_new(GTK_ORIENTATION_VERTICAL, 2));
        let (icon, version) = (make_lbl(text::UPDATE_ICON_TEXT, "update-card-icon", 0.5), make_lbl(c"", "update-card-version", 0.0));
        let (top, badge, author) = build_title_line();
        let close_btn = make_btn(text::UPDATE_CLOSE_BTN, "update-card-close");
        gtk_widget_set_valign(icon, GTK_ALIGN_START);
        gtk_widget_set_valign(close_btn, GTK_ALIGN_START);
        gtk_box_pack_start(meta, top, 0, 0, 0);
        gtk_box_pack_start(meta, version, 0, 0, 0);
        gtk_box_pack_start(row, icon, 0, 0, 0);
        gtk_box_pack_start(row, meta, 1, 1, 0);
        gtk_box_pack_end(row, close_btn, 0, 0, 0);
        (row, [badge, author, version, close_btn])
    }
}

fn make_status_label() -> *mut GtkWidget {
    unsafe {
        let status = make_lbl(text::update_status_available(), "update-card-status", 0.0);
        gtk_label_set_line_wrap(status, 1);
        gtk_label_set_line_wrap_mode(status, PANGO_WRAP_WORD_CHAR);
        gtk_label_set_ellipsize(status, PANGO_ELLIPSIZE_END);
        gtk_label_set_max_width_chars(status, STATUS_MAX_WIDTH_CHARS);
        gtk_widget_set_valign(status, GTK_ALIGN_CENTER);
        status
    }
}

fn build_status_row() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
        let spinner = gtk_spinner_new();
        add_class(spinner, "update-card-spinner");
        gtk_widget_set_size_request(spinner, 18, 18);
        gtk_widget_set_valign(spinner, GTK_ALIGN_CENTER);
        gtk_widget_set_no_show_all(spinner, 1);
        let status = make_status_label();
        gtk_box_pack_start(row, spinner, 0, 0, 0);
        gtk_box_pack_start(row, status, 1, 1, 0);
        (row, spinner, status)
    }
}

fn build_actions_row() -> (*mut GtkWidget, [*mut GtkWidget; 3]) {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        let whats_new = make_btn(text::update_whats_new(), "update-card-link");
        let dismiss = make_btn(text::update_dismiss(), "update-card-dismiss");
        let primary = make_btn(text::update_btn_update(), "update-card-primary");
        gtk_widget_set_no_show_all(primary, 1);
        gtk_box_pack_start(row, whats_new, 0, 0, 0);
        gtk_box_pack_end(row, primary, 0, 0, 0);
        gtk_box_pack_end(row, dismiss, 0, 0, 0);
        (row, [whats_new, dismiss, primary])
    }
}

extern "C" fn drop_inner_rc(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Rc::from_raw(data.cast::<BannerInner>())) };
    }
}

extern "C" fn on_dismiss_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if !data.is_null() {
        (unsafe { &*data.cast::<BannerInner>() }.callbacks.on_dismiss)();
    }
}

extern "C" fn on_whats_new_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if !data.is_null() {
        let inner = unsafe { &*data.cast::<BannerInner>() };
        let url = model_meta(&inner.last_model.borrow()).2.to_owned();
        if !url.is_empty() {
            (inner.callbacks.on_whats_new)(&url);
        }
    }
}

extern "C" fn on_primary_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    if !data.is_null() {
        let inner = unsafe { &*data.cast::<BannerInner>() };
        let model = inner.last_model.borrow().clone();
        match model {
            UpdateBannerModel::Available { .. } | UpdateBannerModel::Failed { .. } => (inner.callbacks.on_update)(),
            UpdateBannerModel::ReadyToRestart { .. } => (inner.callbacks.on_restart)(),
            _ => {}
        }
    }
}

fn wire_banner_clicks(inner: &Rc<BannerInner>) {
    let pairs: [(*mut GtkWidget, extern "C" fn(*mut GtkWidget, *mut c_void)); 4] = [
        (inner.close_btn, on_dismiss_clicked),
        (inner.dismiss_btn, on_dismiss_clicked),
        (inner.whats_new_btn, on_whats_new_clicked),
        (inner.primary_btn, on_primary_clicked),
    ];
    for (btn, handler) in pairs {
        let raw = Rc::into_raw(Rc::clone(inner)) as *mut c_void;
        unsafe {
            g_signal_connect_data(
                btn, c"clicked".as_ptr(), handler as *const c_void, raw, drop_inner_rc as *const c_void, 0,
            );
        }
    }
}

fn apply_overlay_geometry(card: *mut GtkWidget) {
    unsafe {
        gtk_widget_set_size_request(card, CARD_WIDTH_PX, -1);
        gtk_widget_set_halign(card, GTK_ALIGN_END);
        gtk_widget_set_valign(card, GTK_ALIGN_END);
        for setter in [
            gtk_widget_set_margin_start, gtk_widget_set_margin_end,
            gtk_widget_set_margin_top, gtk_widget_set_margin_bottom,
        ] {
            setter(card, CARD_MARGIN_PX);
        }
        gtk_widget_set_can_focus(card, 0);
    }
}

fn status_spec(model: &UpdateBannerModel) -> Option<(Option<&'static CStr>, Option<&str>, bool, Option<&'static CStr>, u8)> {
    match model {
        UpdateBannerModel::Hidden => None,
        UpdateBannerModel::Available { .. } => {
            Some((Some(text::update_status_available()), None, false, Some(text::update_btn_update()), 0))
        }
        UpdateBannerModel::Downloading { .. } => Some((Some(text::update_status_downloading()), None, true, None, 0)),
        UpdateBannerModel::Installing { .. } => Some((Some(text::update_status_installing()), None, true, None, 0)),
        UpdateBannerModel::ReadyToRestart { .. } => {
            Some((Some(text::update_status_ready()), None, false, Some(text::update_btn_restart()), 1))
        }
        UpdateBannerModel::Failed { error, .. } => {
            Some((None, Some(error.as_str()), false, Some(text::update_btn_retry()), 2))
        }
    }
}

fn sync_static_chrome(inner: &BannerInner) {
    unsafe {
        gtk_label_set_text(inner.badge_lbl, text::update_badge().as_ptr());
        gtk_label_set_text(inner.author_lbl, text::update_author().as_ptr());
        gtk_button_set_label(inner.whats_new_btn, text::update_whats_new().as_ptr());
        gtk_button_set_label(inner.dismiss_btn, text::update_dismiss().as_ptr());
        gtk_widget_set_tooltip_text(inner.close_btn, text::update_close_tip().as_ptr());
    }
}

fn sync_spinner_state(spinner: *mut GtkWidget, spinning: bool) {
    unsafe {
        gtk_widget_set_visible(spinner, i32::from(spinning));
        if spinning { gtk_spinner_start(spinner); } else { gtk_spinner_stop(spinner); }
    }
}

fn sync_status_label(status_lbl: *mut GtkWidget, c_msg: Option<&'static CStr>, err_msg: Option<&str>, tone: u8) {
    set_class(status_lbl, "update-card-status-ready", tone == 1);
    set_class(status_lbl, "update-card-status-error", tone == 2);
    if let Some(msg) = c_msg {
        unsafe {
            gtk_label_set_text(status_lbl, msg.as_ptr());
            gtk_widget_set_tooltip_text(status_lbl, ptr::null());
        }
    } else if let Some(err) = err_msg {
        set_label_text(status_lbl, err);
        set_tooltip(status_lbl, err);
    }
}

fn make_divider() -> *mut GtkWidget {
    unsafe {
        let divider = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
        add_class(divider, "update-card-divider");
        gtk_widget_set_size_request(divider, -1, 1);
        divider
    }
}

fn build_card_box() -> (*mut GtkWidget, [*mut GtkWidget; 9]) {
    unsafe {
        let card = gtk_box_new(GTK_ORIENTATION_VERTICAL, 12);
        add_class(card, "update-card");
        apply_overlay_geometry(card);
        let (header_row, [badge, author, version, close]) = build_header_row();
        let (status_row, spinner, status) = build_status_row();
        let (actions_row, [whats_new, dismiss, primary]) = build_actions_row();
        for row in [header_row, status_row, make_divider(), actions_row] {
            gtk_box_pack_start(card, row, 0, 0, 0);
        }
        gtk_widget_show_all(card);
        gtk_widget_set_visible(card, 0);
        gtk_widget_set_no_show_all(card, 1);
        (card, [badge, author, version, close, spinner, status, whats_new, dismiss, primary])
    }
}

fn build_card_shell(callbacks: UpdateBannerCallbacks) -> Rc<BannerInner> {
    let (card, w) = build_card_box();
    Rc::new(BannerInner {
        callbacks, last_model: RefCell::new(UpdateBannerModel::Hidden),
        card, badge_lbl: w[0], author_lbl: w[1], version_lbl: w[2], close_btn: w[3],
        spinner: w[4], status_lbl: w[5], whats_new_btn: w[6], dismiss_btn: w[7], primary_btn: w[8],
    })
}

fn hide_banner(inner: &BannerInner) {
    sync_spinner_state(inner.spinner, false);
    unsafe {
        gtk_widget_set_visible(inner.primary_btn, 0);
        gtk_widget_set_visible(inner.card, 0);
    }
}

fn show_banner(inner: &BannerInner, model: &UpdateBannerModel, spec: (Option<&'static CStr>, Option<&str>, bool, Option<&'static CStr>, u8)) {
    let (c_msg, err_msg, spinning, primary_lbl, tone) = spec;
    let (cur, lat, release_url) = model_meta(model);
    set_label_text(inner.version_lbl, &format_version_transition(cur, lat, text::locale_is_ru()));
    sync_spinner_state(inner.spinner, spinning);
    sync_status_label(inner.status_lbl, c_msg, err_msg, tone);
    sync_action_buttons(inner, release_url, primary_lbl);
}

fn sync_action_buttons(inner: &BannerInner, release_url: &str, primary_lbl: Option<&'static CStr>) {
    unsafe {
        gtk_widget_set_sensitive(inner.whats_new_btn, i32::from(!release_url.is_empty()));
        gtk_widget_set_visible(inner.primary_btn, i32::from(primary_lbl.is_some()));
        if let Some(lbl) = primary_lbl {
            gtk_button_set_label(inner.primary_btn, lbl.as_ptr());
        }
        gtk_widget_set_visible(inner.card, 1);
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl UpdateBanner {
    pub fn new(callbacks: UpdateBannerCallbacks) -> Self {
        let inner = build_card_shell(callbacks);
        wire_banner_clicks(&inner);
        Self { inner }
    }

    pub fn mount_in_overlay(&self, overlay: *mut GtkWidget) {
        if !overlay.is_null() {
            apply_overlay_geometry(self.inner.card);
            unsafe {
                gtk_overlay_add_overlay(overlay, self.inner.card);
                gtk_overlay_set_overlay_pass_through(overlay, self.inner.card, 0);
            }
        }
    }

    pub fn widget(&self) -> *mut GtkWidget {
        self.inner.card
    }

    pub fn apply_model(&self, model: &UpdateBannerModel) {
        *self.inner.last_model.borrow_mut() = model.clone();
        sync_static_chrome(&self.inner);
        match status_spec(model) {
            Some(spec) => show_banner(&self.inner, model, spec),
            None => hide_banner(&self.inner),
        }
    }

    pub fn refresh_locale(&self) {
        let model = self.inner.last_model.borrow().clone();
        self.apply_model(&model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::c_char;

    use crate::i18n::{set_locale, with_locale_lock, UiLocale};
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::theme::apply_gtk_theme_mode;
    use crate::theme_mode::ThemeMode;

    const URL: &str = "https://github.com/akhtyamovpavel/akhterm/releases/tag/v1.1.0";

    fn spinner_is_active(spinner: *mut GtkWidget) -> bool {
        let mut active: c_int = 0;
        unsafe { g_object_get(spinner, c"active".as_ptr(), &mut active as *mut c_int, ptr::null::<c_char>()) };
        active != 0
    }

    fn check_lbl(lbl: *mut GtkWidget, expected: &str) {
        let got = unsafe { CStr::from_ptr(gtk_label_get_text(lbl)).to_string_lossy().into_owned() };
        assert_eq!(got, expected);
    }

    fn check_btn(btn: *mut GtkWidget, expected: &str) {
        let got = unsafe { CStr::from_ptr(gtk_button_get_label(btn)).to_string_lossy().into_owned() };
        assert_eq!(got, expected);
    }

    fn mk_avail(c: String, l: String, url: String) -> UpdateBannerModel {
        UpdateBannerModel::Available { current_version: c, latest_version: l, release_url: url }
    }

    fn sample(kind: u8) -> UpdateBannerModel {
        let (c, l, url) = (String::from("1.0.0"), String::from("1.1.0"), String::from(URL));
        match kind {
            1 => mk_avail(c, l, url),
            2 => UpdateBannerModel::Downloading { current_version: c, latest_version: l, release_url: url },
            3 => UpdateBannerModel::Installing { current_version: c, latest_version: l, release_url: url },
            4 => UpdateBannerModel::ReadyToRestart { current_version: c, latest_version: l, release_url: url },
            5 => UpdateBannerModel::Failed {
                current_version: c, latest_version: Some(l), release_url: url, error: String::from("SHA-256 mismatch"),
            },
            _ => UpdateBannerModel::Hidden,
        }
    }

    fn make_test_banner() -> (UpdateBanner, [Rc<Cell<usize>>; 3], Rc<RefCell<Vec<String>>>) {
        let counts = [Rc::new(Cell::new(0)), Rc::new(Cell::new(0)), Rc::new(Cell::new(0))];
        let urls = Rc::new(RefCell::new(Vec::new()));
        let [u, r, d] = counts.clone();
        let w = Rc::clone(&urls);
        let banner = UpdateBanner::new(UpdateBannerCallbacks {
            on_update: Rc::new(move || u.set(u.get() + 1)),
            on_restart: Rc::new(move || r.set(r.get() + 1)),
            on_dismiss: Rc::new(move || d.set(d.get() + 1)),
            on_whats_new: Rc::new(move |url| w.borrow_mut().push(url.to_owned())),
        });
        (banner, counts, urls)
    }

    fn mount_test_window(banner: &UpdateBanner) -> *mut GtkWidget {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            let overlay = gtk_overlay_new();
            gtk_container_add(overlay, gtk_box_new(GTK_ORIENTATION_VERTICAL, 0));
            banner.mount_in_overlay(overlay);
            gtk_container_add(win, overlay);
            gtk_widget_show_all(win);
            pump_gtk_events();
            win
        }
    }

    fn verify_available_and_callbacks(banner: &UpdateBanner, u: &Rc<Cell<usize>>, d: &Rc<Cell<usize>>, urls: &Rc<RefCell<Vec<String>>>) {
        banner.apply_model(&sample(1));
        pump_gtk_events();
        check_lbl(banner.inner.badge_lbl, "UPDATE");
        check_lbl(banner.inner.version_lbl, "v1.0.0 to 1.1.0");
        check_btn(banner.inner.primary_btn, "UPDATE");
        assert!(!spinner_is_active(banner.inner.spinner));
        unsafe { [banner.inner.primary_btn, banner.inner.whats_new_btn, banner.inner.dismiss_btn].into_iter().for_each(|b| gtk_button_clicked(b)) };
        assert_eq!((u.get(), d.get(), urls.borrow().len()), (1, 1, 1));
    }

    fn verify_spinner_and_locale(banner: &UpdateBanner) {
        banner.apply_model(&sample(2));
        pump_gtk_events();
        assert!(spinner_is_active(banner.inner.spinner));
        assert_eq!(unsafe { gtk_widget_get_visible(banner.inner.primary_btn) }, 0);
        banner.apply_model(&sample(3));
        set_locale(UiLocale::Ru);
        banner.refresh_locale();
        pump_gtk_events();
        check_lbl(banner.inner.badge_lbl, "ОБНОВЛЕНИЕ");
        check_lbl(banner.inner.version_lbl, "с v1.0.0 до 1.1.0");
        check_lbl(banner.inner.status_lbl, "Установка обновления...");
    }

    fn verify_ready_and_failed(banner: &UpdateBanner, u: &Rc<Cell<usize>>, r: &Rc<Cell<usize>>) {
        banner.apply_model(&sample(4));
        pump_gtk_events();
        assert!(!spinner_is_active(banner.inner.spinner));
        check_btn(banner.inner.primary_btn, "Перезапустить");
        unsafe { gtk_button_clicked(banner.inner.primary_btn) };
        assert_eq!(r.get(), 1);
        banner.apply_model(&sample(5));
        pump_gtk_events();
        check_btn(banner.inner.primary_btn, "Повторить");
        assert!(has_class(banner.inner.status_lbl, "update-card-status-error"));
        unsafe { gtk_button_clicked(banner.inner.primary_btn) };
        assert_eq!(u.get(), 2);
    }

    fn verify_light_and_hide(banner: &UpdateBanner) {
        apply_gtk_theme_mode(ThemeMode::Light);
        banner.apply_model(&sample(0));
        pump_gtk_events();
        assert_eq!(unsafe { gtk_widget_get_visible(banner.widget()) }, 0);
        assert!(!spinner_is_active(banner.inner.spinner));
    }

    #[test]
    fn formats_bilingual_version_transitions_without_text_arrows() {
        let en = format_version_transition("1.0.0", Some("1.1.0"), false);
        let ru = format_version_transition("v1.0.0", Some("v1.1.0"), true);
        assert_eq!((en.as_str(), ru.as_str()), ("v1.0.0 to 1.1.0", "с v1.0.0 до 1.1.0"));
        assert_eq!(format_version_transition("1.0.0", None, true), "v1.0.0");
        assert!(!en.contains("->") && !en.contains('→') && !ru.contains("->") && !ru.contains('→'));
    }

    #[test]
    fn headless_broadway_update_banner_covers_all_six_states_callbacks_and_locales() {
        with_locale_lock(|| {
            set_locale(UiLocale::En);
            with_headless_gtk(|| {
                apply_gtk_theme_mode(ThemeMode::Dark);
                let (banner, [u, r, d], urls) = make_test_banner();
                let win = mount_test_window(&banner);
                assert_eq!(unsafe { gtk_widget_get_visible(banner.widget()) }, 0);
                verify_available_and_callbacks(&banner, &u, &d, &urls);
                verify_spinner_and_locale(&banner);
                verify_ready_and_failed(&banner, &u, &r);
                verify_light_and_hide(&banner);
                unsafe { gtk_widget_destroy(win) };
                pump_gtk_events();
            });
        });
    }

    #[test]
    fn update_banner_module_achieves_target_mi() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("update_banner.rs"));
        assert!(mi >= 61.0, "update_banner.rs MI {mi:.2} < 61.0");
    }
}
