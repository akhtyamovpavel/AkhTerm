//! Compact bottom status-bar pill and Dark/Light Zinc popover for live CPU, RAM, and battery telemetry.

use std::cell::RefCell;
use std::ffi::{c_void, CStr, CString};
use std::rc::Rc;

use crate::ffi::*;
use crate::sys_monitor::{
    alert_level, battery_alert, cpu_alert, format_eta, format_mem_size, format_pill_label,
    ram_alert, AlertLevel, BatteryInfo, BatteryState, CpuUsage, LoadAvg, MemInfo, SysSnapshot,
};
use crate::ui_text as text;

const KIB_PER_GIB: f64 = 1024.0 * 1024.0;

struct RowWidgets { row: *mut GtkWidget, key: *mut GtkWidget, val: *mut GtkWidget }

struct SysMonitorInner {
    btn: *mut GtkWidget, popover: *mut GtkWidget, title_lbl: *mut GtkWidget,
    cpu: RowWidgets, cpu_detail: RowWidgets, load: RowWidgets,
    ram: RowWidgets, swap: RowWidgets, tab_rss: RowWidgets,
    bat: RowWidgets, power: RowWidgets, eta: RowWidgets, health: RowWidgets,
    last_snap: RefCell<SysSnapshot>,
}

#[derive(Clone)]
pub struct SysMonitorWidget { inner: Rc<SysMonitorInner> }

fn make_lbl(initial: &CStr, class: &str, xalign: f32) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(initial.as_ptr());
        gtk_label_set_xalign(lbl, xalign);
        add_class(lbl, class);
        lbl
    }
}

fn make_row(parent: *mut GtkWidget, key_text: &CStr, val_class: &str) -> RowWidgets {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 12);
        let key = make_lbl(key_text, "sys-mon-key", 0.0);
        let val = make_lbl(c"—", val_class, 1.0);
        gtk_box_pack_start(row, key, 1, 1, 0);
        gtk_box_pack_end(row, val, 0, 0, 0);
        gtk_box_pack_start(parent, row, 0, 1, 0);
        RowWidgets { row, key, val }
    }
}

fn add_divider(parent: *mut GtkWidget) {
    unsafe {
        let sep = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
        add_class(sep, "sys-mon-divider");
        gtk_box_pack_start(parent, sep, 0, 1, 0);
    }
}

fn apply_alert_classes(widget: *mut GtkWidget, level: AlertLevel) {
    set_class(widget, "sys-mon-warn", level == AlertLevel::Warn);
    set_class(widget, "sys-mon-hot", level == AlertLevel::Hot);
}

fn battery_state_label(state: BatteryState) -> &'static str {
    match state {
        BatteryState::Discharging => text::sys_mon_bat_discharging(),
        BatteryState::Charging => text::sys_mon_bat_charging(),
        BatteryState::Full => text::sys_mon_bat_full(),
        BatteryState::NotCharging => text::sys_mon_bat_not_charging(),
        BatteryState::Unknown => text::sys_mon_bat_unknown(),
    }
}

fn format_cpu_detail(cpu: Option<CpuUsage>) -> (String, String) {
    match cpu {
        Some(c) => (
            format!("{:.1}%", c.total_pct),
            format!("user {:.1}% · sys {:.1}% · iowait {:.1}%", c.user_pct, c.sys_pct, c.iowait_pct),
        ),
        None => ("--%".to_owned(), "—".to_owned()),
    }
}

fn format_load_text(load: Option<LoadAvg>) -> String {
    match load {
        Some(l) => format!("{:.2} / {:.2} / {:.2}", l.one, l.five, l.fifteen),
        None => "—".to_owned(),
    }
}

fn format_mem_texts(mem: Option<MemInfo>) -> (String, String) {
    let Some(m) = mem else {
        return ("—".to_owned(), "—".to_owned());
    };
    let used_g = m.mem_used_kib() as f64 / KIB_PER_GIB;
    let total_g = m.mem_total_kib as f64 / KIB_PER_GIB;
    let ram_s = format!("{used_g:.1} / {total_g:.1} GiB ({:.0}%)", m.mem_used_pct());
    let swap_s = if m.swap_total_kib > 0 {
        let su = m.swap_used_kib() as f64 / KIB_PER_GIB;
        let st = m.swap_total_kib as f64 / KIB_PER_GIB;
        format!("{su:.1} / {st:.1} GiB")
    } else {
        text::sys_mon_swap_off().to_owned()
    };
    (ram_s, swap_s)
}

fn format_bat_texts(bat: BatteryInfo) -> (String, String, String, String) {
    let main = format!("{}% · {}", bat.capacity_pct, battery_state_label(bat.state));
    let power = bat.power_watts.filter(|&w| w >= 0.05).map_or_else(|| "—".to_owned(), |w| format!("{w:.1}W"));
    let eta = bat.eta_minutes.map_or_else(|| "—".to_owned(), |m| format_eta(m, text::locale_is_ru()));
    let health = bat.health_pct.map_or_else(|| "—".to_owned(), |h| format!("{h:.0}%"));
    (main, power, eta, health)
}

fn build_pill_button(on_open: extern "C" fn(*mut GtkWidget, *mut c_void)) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new_with_label(c"CPU --%".as_ptr());
        gtk_widget_set_can_focus(btn, 0);
        add_class(btn, "sys-mon-btn");
        gtk_widget_set_tooltip_text(btn, text::sys_mon_tip().as_ptr());
        crate::app_state::connect_click(btn, on_open, std::ptr::null_mut());
        btn
    }
}

fn build_popover_shell(btn: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let popover = gtk_popover_new(btn);
        gtk_popover_set_position(popover, GTK_POS_TOP);
        let box_w = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        add_class(box_w, "popover-box");
        add_class(box_w, "sys-mon-pop");
        gtk_container_add(popover, box_w);
        (popover, box_w)
    }
}

fn build_cpu_rows(pop_box: *mut GtkWidget) -> (RowWidgets, RowWidgets, RowWidgets) {
    let cpu = make_row(pop_box, text::sys_mon_cpu(), "sys-mon-val");
    let detail = make_row(pop_box, text::sys_mon_cpu_detail(), "sys-mon-sub");
    let load = make_row(pop_box, text::sys_mon_load(), "sys-mon-sub");
    add_divider(pop_box);
    (cpu, detail, load)
}

fn build_mem_rows(pop_box: *mut GtkWidget) -> (RowWidgets, RowWidgets, RowWidgets) {
    let ram = make_row(pop_box, text::sys_mon_ram(), "sys-mon-val");
    let swap = make_row(pop_box, text::sys_mon_swap(), "sys-mon-sub");
    let tab_rss = make_row(pop_box, text::sys_mon_tab_rss(), "sys-mon-sub");
    add_divider(pop_box);
    (ram, swap, tab_rss)
}

fn build_bat_rows(pop_box: *mut GtkWidget) -> (RowWidgets, RowWidgets, RowWidgets, RowWidgets) {
    let bat = make_row(pop_box, text::sys_mon_battery(), "sys-mon-val");
    let power = make_row(pop_box, text::sys_mon_power(), "sys-mon-sub");
    let eta = make_row(pop_box, text::sys_mon_eta_left(), "sys-mon-sub");
    let health = make_row(pop_box, text::sys_mon_health(), "sys-mon-sub");
    (bat, power, eta, health)
}

impl SysMonitorWidget {
    pub fn new(on_open: extern "C" fn(*mut GtkWidget, *mut c_void)) -> Self {
        let btn = build_pill_button(on_open);
        let (popover, pop_box) = build_popover_shell(btn);
        let title_lbl = make_lbl(text::sys_mon_title(), "sys-mon-title", 0.0);
        unsafe { gtk_box_pack_start(pop_box, title_lbl, 0, 1, 0) };
        let (cpu, cpu_detail, load) = build_cpu_rows(pop_box);
        let (ram, swap, tab_rss) = build_mem_rows(pop_box);
        let (bat, power, eta, health) = build_bat_rows(pop_box);
        unsafe {
            gtk_widget_show_all(pop_box);
            [power.row, eta.row, health.row].into_iter().for_each(|r| gtk_widget_set_no_show_all(r, 1));
        }
        Self {
            inner: Rc::new(SysMonitorInner {
                btn, popover, title_lbl, cpu, cpu_detail, load, ram, swap, tab_rss,
                bat, power, eta, health, last_snap: RefCell::new(SysSnapshot::default()),
            }),
        }
    }

    pub fn button(&self) -> *mut GtkWidget {
        self.inner.btn
    }

    #[cfg(test)]
    pub fn popover(&self) -> *mut GtkWidget {
        self.inner.popover
    }

    pub fn update(&self, snap: &SysSnapshot) {
        *self.inner.last_snap.borrow_mut() = snap.clone();
        self.render_snapshot(snap);
    }

    pub fn refresh_locale(&self) {
        let i = &self.inner;
        let pairs = [
            (i.title_lbl, text::sys_mon_title()), (i.cpu.key, text::sys_mon_cpu()),
            (i.cpu_detail.key, text::sys_mon_cpu_detail()), (i.load.key, text::sys_mon_load()),
            (i.ram.key, text::sys_mon_ram()), (i.swap.key, text::sys_mon_swap()),
            (i.tab_rss.key, text::sys_mon_tab_rss()), (i.bat.key, text::sys_mon_battery()),
            (i.power.key, text::sys_mon_power()), (i.health.key, text::sys_mon_health()),
        ];
        unsafe {
            gtk_widget_set_tooltip_text(i.btn, text::sys_mon_tip().as_ptr());
            pairs.into_iter().for_each(|(w, t)| gtk_label_set_text(w, t.as_ptr()));
        }
        let snap = i.last_snap.borrow().clone();
        self.render_snapshot(&snap);
    }

    pub fn toggle_popover(&self) {
        unsafe {
            if gtk_widget_get_visible(self.inner.popover) != 0 {
                gtk_popover_popdown(self.inner.popover);
            } else {
                gtk_popover_popup(self.inner.popover);
            }
        }
    }
}

impl SysMonitorWidget {
    fn render_snapshot(&self, snap: &SysSnapshot) {
        let i = &self.inner;
        if let Ok(c) = CString::new(format_pill_label(snap)) {
            unsafe { gtk_button_set_label(i.btn, c.as_ptr()) };
        }
        apply_alert_classes(i.btn, alert_level(snap));
        self.render_cpu_rows(snap.cpu, snap.load);
        self.render_mem_rows(snap.mem, snap.tab_rss_kib);
        self.render_battery(snap.battery);
    }

    fn render_cpu_rows(&self, cpu: Option<CpuUsage>, load: Option<LoadAvg>) {
        let i = &self.inner;
        let (cpu_s, cpu_det) = format_cpu_detail(cpu);
        set_label_text(i.cpu.val, &cpu_s);
        apply_alert_classes(i.cpu.val, cpu_alert(cpu));
        set_label_text(i.cpu_detail.val, &cpu_det);
        set_label_text(i.load.val, &format_load_text(load));
    }

    fn render_mem_rows(&self, mem: Option<MemInfo>, tab_rss_kib: Option<u64>) {
        let i = &self.inner;
        let (ram_s, swap_s) = format_mem_texts(mem);
        set_label_text(i.ram.val, &ram_s);
        apply_alert_classes(i.ram.val, ram_alert(mem));
        set_label_text(i.swap.val, &swap_s);
        let tab_s = tab_rss_kib.map_or_else(|| "—".to_owned(), format_mem_size);
        set_label_text(i.tab_rss.val, &tab_s);
    }

    fn set_bat_rows_visible(&self, visible: bool) {
        let i = &self.inner;
        let v = i32::from(visible);
        unsafe { [i.power.row, i.eta.row, i.health.row].into_iter().for_each(|r| gtk_widget_set_visible(r, v)) };
    }

    fn render_bat_present(&self, b: BatteryInfo) {
        let i = &self.inner;
        let (main, power, eta, health) = format_bat_texts(b);
        set_label_text(i.bat.val, &main);
        apply_alert_classes(i.bat.val, battery_alert(Some(b)));
        set_label_text(i.power.val, &power);
        let eta_key = if b.state == BatteryState::Charging { text::sys_mon_eta_full() } else { text::sys_mon_eta_left() };
        unsafe { gtk_label_set_text(i.eta.key, eta_key.as_ptr()) };
        set_label_text(i.eta.val, &eta);
        set_label_text(i.health.val, &health);
        self.set_bat_rows_visible(true);
    }

    fn render_battery(&self, bat: Option<BatteryInfo>) {
        let Some(b) = bat else {
            set_label_text(self.inner.bat.val, text::sys_mon_ac_only());
            apply_alert_classes(self.inner.bat.val, AlertLevel::Normal);
            self.set_bat_rows_visible(false);
            return;
        };
        self.render_bat_present(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    use crate::i18n::{set_locale, with_locale_lock, UiLocale};
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
    use crate::theme::apply_gtk_theme_mode;
    use crate::theme_mode::{build_stylesheet, ThemeMode};

    extern "C" fn noop_open(_btn: *mut GtkWidget, _data: *mut c_void) {}

    fn label_text(lbl: *mut GtkWidget) -> String {
        unsafe { CStr::from_ptr(gtk_label_get_text(lbl)).to_string_lossy().into_owned() }
    }

    fn button_text(btn: *mut GtkWidget) -> String {
        unsafe { CStr::from_ptr(gtk_button_get_label(btn)).to_string_lossy().into_owned() }
    }

    fn assert_css_clean(mode: ThemeMode) {
        let raw = build_stylesheet(mode, include_str!("theme.css"), "");
        let css = CString::new(raw).unwrap();
        let mut errs: usize = 0;
        extern "C" fn on_err(_p: *mut c_void, _s: *mut c_void, _e: *mut c_void, ud: *mut c_void) {
            unsafe { *(ud as *mut usize) += 1 };
        }
        unsafe {
            let prov = gtk_css_provider_new();
            g_signal_connect_data(prov, c"parsing-error".as_ptr(), on_err as *const c_void, &mut errs as *mut usize as *mut c_void, ptr::null(), 0);
            let mut err: *mut c_void = ptr::null_mut();
            let ok = gtk_css_provider_load_from_data(prov, css.as_ptr(), -1, &mut err);
            assert_ne!(ok, 0, "CSS failed for {mode:?}");
            assert!(err.is_null() && errs == 0, "CSS parse errors ({errs}) for {mode:?}");
            g_object_unref(prov);
        }
    }

    fn sample_laptop_snapshot() -> SysSnapshot {
        SysSnapshot {
            cpu: Some(CpuUsage { total_pct: 18.4, user_pct: 12.1, sys_pct: 5.8, iowait_pct: 0.5 }),
            load: Some(LoadAvg { one: 1.42, five: 1.18, fifteen: 0.95 }),
            mem: Some(MemInfo {
                mem_total_kib: 33_554_432, mem_available_kib: 18_664_652,
                swap_total_kib: 8_388_608, swap_free_kib: 6_291_456,
            }),
            tab_rss_kib: Some(256 * 1024),
            battery: Some(BatteryInfo {
                capacity_pct: 82, state: BatteryState::Discharging,
                power_watts: Some(14.8), eta_minutes: Some(195), health_pct: Some(90.0),
            }),
        }
    }

    fn assert_no_arrows(widget: &SysMonitorWidget) {
        let i = &widget.inner;
        let texts = [
            button_text(i.btn), label_text(i.title_lbl), label_text(i.cpu.val),
            label_text(i.cpu_detail.val), label_text(i.load.val), label_text(i.ram.val),
            label_text(i.swap.val), label_text(i.tab_rss.val), label_text(i.bat.val),
            label_text(i.power.val), label_text(i.eta.val), label_text(i.health.val),
        ];
        for t in texts {
            assert!(!t.contains("->") && !t.contains('→') && !t.contains("=>"), "arrow in UI text: {t}");
        }
    }

    fn check_locale_render(mon: &SysMonitorWidget, snap: &SysSnapshot, locale: UiLocale) {
        set_locale(locale);
        mon.refresh_locale();
        mon.update(snap);
        pump_gtk_events();
        assert_eq!(button_text(mon.button()), "CPU 18% · RAM 14.2/32G · 82% 14.8W");
        let i = &mon.inner;
        assert_eq!(label_text(i.cpu.val), "18.4%");
        assert_eq!(label_text(i.load.val), "1.42 / 1.18 / 0.95");
        assert_eq!(label_text(i.tab_rss.val), "256 MiB");
        assert_eq!(label_text(i.power.val), "14.8W");
        assert_eq!(label_text(i.health.val), "90%");
        if locale == UiLocale::Ru {
            assert_eq!(label_text(i.title_lbl), "Системный монитор");
            assert_eq!(label_text(i.eta.val), "3ч 15м");
        } else {
            assert_eq!(label_text(i.title_lbl), "System Monitor");
            assert_eq!(label_text(i.eta.val), "3h 15m");
        }
        assert_no_arrows(mon);
    }

    fn mount_test_footer() -> (*mut GtkWidget, SysMonitorWidget) {
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let footer = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8) };
        add_class(footer, "footer-bar");
        let mon = SysMonitorWidget::new(noop_open);
        unsafe {
            gtk_box_pack_end(footer, mon.button(), 0, 0, 0);
            gtk_container_add(win, footer);
            gtk_widget_show_all(win);
        }
        (win, mon)
    }

    fn verify_popover_alert_and_desktop(mon: &SysMonitorWidget, snap: &SysSnapshot) {
        mon.toggle_popover();
        pump_gtk_events();
        assert_ne!(unsafe { gtk_widget_get_visible(mon.popover()) }, 0);
        mon.toggle_popover();
        pump_gtk_events();
        let mut hot = snap.clone();
        hot.cpu = Some(CpuUsage { total_pct: 95.0, user_pct: 80.0, sys_pct: 15.0, iowait_pct: 0.0 });
        mon.update(&hot);
        assert!(has_class(mon.button(), "sys-mon-hot") && !has_class(mon.button(), "sys-mon-warn"));
        let mut desktop = snap.clone();
        desktop.battery = None;
        mon.update(&desktop);
        assert_eq!(label_text(mon.inner.bat.val), "AC / No battery");
        assert_eq!(unsafe { gtk_widget_get_visible(mon.inner.power.row) }, 0);
    }

    #[test]
    fn headless_broadway_pill_and_popover_ru_en_dark_light_zero_css_errors() {
        with_locale_lock(|| with_headless_gtk(|| {
            assert_css_clean(ThemeMode::Dark);
            assert_css_clean(ThemeMode::Light);
            let (win, mon) = mount_test_footer();
            let snap = sample_laptop_snapshot();
            apply_gtk_theme_mode(ThemeMode::Dark);
            check_locale_render(&mon, &snap, UiLocale::Ru);
            apply_gtk_theme_mode(ThemeMode::Light);
            check_locale_render(&mon, &snap, UiLocale::En);
            verify_popover_alert_and_desktop(&mon, &snap);
            unsafe { gtk_widget_destroy(win) };
        }));
    }
}
