//! Unified RU/EN localization leak audit and popover sizing tests across UI modules.

use std::ffi::{c_void, CStr};

use crate::app_state;
use crate::ffi::*;
use crate::i18n::{set_locale, with_locale_lock, UiLocale};
use crate::port_radar::{BindScope, DetectedPort};
use crate::ports_ui::{PortsMenu, PortsSnapshot};
use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
use crate::session::{SessionCategory, WorkState};
use crate::ssh_tunnel::{PinnedTunnel, TunnelInfo, TunnelStatus};
use crate::tabs::Tab;
use crate::telemetry::{
    build_tokens_view_for_mode, format_working_sidebar_status, TabTokenRow, TelemetrySource,
    TokenSnapshot, TokensMenu,
};
use crate::theme_mode::ThemeMode;

#[repr(C)]
struct AuditGList { data: *mut c_void, next: *mut AuditGList, prev: *mut AuditGList }

extern "C" {
    fn gtk_container_get_children(container: *mut GtkWidget) -> *mut AuditGList;
    fn g_list_free(list: *mut AuditGList);
}

extern "C" fn noop_click(_w: *mut GtkWidget, _d: *mut c_void) {}

fn has_cyrillic(s: &str) -> bool {
    s.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
}

fn cptr_str(ptr: *const std::ffi::c_char) -> String {
    if ptr.is_null() { String::new() } else { unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() } }
}

fn btn_text(btn: *mut GtkWidget) -> String { cptr_str(unsafe { gtk_button_get_label(btn) }) }
fn lbl_text(lbl: *mut GtkWidget) -> String { cptr_str(unsafe { gtk_label_get_text(lbl) }) }

fn push_non_empty(out: &mut Vec<String>, text: String) {
    if !text.is_empty() { out.push(text); }
}

fn collect_all_labels(root: *mut GtkWidget, out: &mut Vec<String>) {
    if root.is_null() { return; }
    let kind = cptr_str(unsafe { g_type_name_from_instance(root) });
    if kind == "GtkLabel" { push_non_empty(out, lbl_text(root)); return; }
    if kind == "GtkButton" { push_non_empty(out, btn_text(root)); }
    if matches!(kind.as_str(), "GtkImage" | "GtkEntry" | "GtkSearchEntry" | "GtkSeparator") { return; }
    collect_children(root, out);
}

fn collect_children(root: *mut GtkWidget, out: &mut Vec<String>) {
    unsafe {
        let head = gtk_container_get_children(root);
        let mut cur = head;
        while !cur.is_null() {
            collect_all_labels((*cur).data as *mut GtkWidget, out);
            cur = (*cur).next;
        }
        if !head.is_null() { g_list_free(head); }
    }
}

fn assert_no_cyrillic(items: &[String], ctx: &str) {
    for s in items {
        assert!(!has_cyrillic(s), "RU leak in EN {ctx}: {s}");
    }
}

fn assert_none_equal(items: &[String], forbidden: &[&str], ctx: &str) {
    for bad in forbidden {
        assert!(!items.iter().any(|s| s == bad), "EN leak in RU {ctx}: {bad}");
    }
}

fn sample_token_rows() -> Vec<TabTokenRow> {
    let mut s1 = TokenSnapshot {
        source: TelemetrySource::Claude, input_tokens: 12_000, cache_tokens: 48_000,
        output_tokens: 2_400, context_tokens: 60_000, context_pct: Some(30),
        cost_usd: Some(0.18), ..TokenSnapshot::default()
    };
    s1.requests.record(Some(40_000), 50_000, Some(1));
    let s2 = TokenSnapshot { source: TelemetrySource::VteFallback, context_pct: Some(45), ..TokenSnapshot::default() };
    vec![
        TabTokenRow { title: "Project".into(), agent: "claude".into(), is_active: true, working_secs: Some(75), last_turn_secs: None, snap: s1 },
        TabTokenRow { title: "Worker".into(), agent: "codex".into(), is_active: false, working_secs: None, last_turn_secs: Some(42), snap: s2 },
    ]
}

fn assert_tokens_en(menu: TokensMenu, rows: &[TabTokenRow]) {
    set_locale(UiLocale::En);
    menu.apply(&build_tokens_view_for_mode(&[], ThemeMode::Dark));
    assert_eq!(btn_text(menu.button_widget()), "Tokens ▾");
    assert_eq!(format_working_sidebar_status(72), "Working · 1m 12s");
    let view = build_tokens_view_for_mode(rows, ThemeMode::Dark);
    menu.apply(&view);
    let markup = view.markup.as_deref().unwrap_or("");
    assert_no_cyrillic(&[btn_text(menu.button_widget()), lbl_text(menu.header_widget()), markup.to_owned()], "tokens_ui");
    assert!(markup.contains("Window total (2)") && markup.contains("(working)") && markup.contains("TUI status"));
}

fn assert_tokens_ru(menu: TokensMenu, rows: &[TabTokenRow]) {
    set_locale(UiLocale::Ru);
    let view = build_tokens_view_for_mode(rows, ThemeMode::Dark);
    menu.apply(&view);
    let markup = view.markup.as_deref().unwrap_or("");
    assert!(btn_text(menu.button_widget()).starts_with("Токены · "));
    assert!(markup.contains("Итого по окну (2)") && markup.contains("(в работе)") && markup.contains("статус TUI"));
    assert!(!markup.contains("Window total") && !markup.contains("TUI status"));
    assert_eq!(format_working_sidebar_status(72), "В работе · 1m 12s");
}

#[test]
fn tokens_ui_zero_ru_leak_in_en_and_zero_en_leak_in_ru() {
    with_locale_lock(|| with_headless_gtk(|| {
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let bar = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4) };
        unsafe { gtk_container_add(win, bar) };
        set_locale(UiLocale::En);
        let menu = TokensMenu::new(bar, noop_click);
        let rows = sample_token_rows();
        assert_tokens_en(menu, &rows);
        assert_tokens_ru(menu, &rows);
        unsafe { gtk_widget_destroy(win) };
        pump_gtk_events();
    }));
}

fn sample_ports_snap() -> PortsSnapshot {
    PortsSnapshot {
        session_id: 1, remote_host: Some("gpu01".into()), ssh_pid: 0, local_ports: Vec::new(),
        remote_ports: vec![DetectedPort { port: 8888, process: "jupyter".into(), scheme: "http".into(), scope: BindScope::Loopback }],
        tunnels: vec![
            TunnelInfo { id: 1, host: "gpu01".into(), remote_port: 8000, local_port: 18000, scheme: "http".into(), process: "uvicorn".into(), session_ids: vec![1], pid: 4242, status: TunnelStatus::Active },
            TunnelInfo { id: 2, host: "gpu01".into(), remote_port: 5173, local_port: 15173, scheme: "http".into(), process: "vite".into(), session_ids: vec![1], pid: 0, status: TunnelStatus::Disconnected },
        ],
        pinned_tunnels: vec![PinnedTunnel { host: "gpu01".into(), remote_port: 9090, local_port: 19090, scheme: "http".into(), process: "prom".into() }],
    }
}

fn collect_ports_labels(menu: &PortsMenu, snap: &PortsSnapshot, locale: UiLocale) -> Vec<String> {
    set_locale(locale);
    menu.sync_button(snap, locale);
    menu.populate(snap, locale);
    let mut out = vec![btn_text(menu.button_widget())];
    collect_all_labels(menu.popover_widget(), &mut out);
    out
}

#[test]
fn ports_ui_zero_ru_leak_in_en_and_zero_en_leak_in_ru() {
    with_locale_lock(|| with_headless_gtk(|| {
        let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let bar = unsafe { gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4) };
        unsafe { gtk_container_add(win, bar) };
        let snap = sample_ports_snap();
        let menu = PortsMenu::new(bar);
        let en = collect_ports_labels(&menu, &snap, UiLocale::En);
        assert_no_cyrillic(&en, "ports_ui");
        assert!(en.iter().any(|s| s == "Disconnected") && en.iter().any(|s| s == "Reconnect") && en.iter().any(|s| s == "Restore Pinned"));
        let ru = collect_ports_labels(&menu, &snap, UiLocale::Ru);
        assert!(ru.iter().any(|s| s == "Разорван") && ru.iter().any(|s| s == "Переподключить") && ru.iter().any(|s| s == "Восстановить закреплённые"));
        assert_none_equal(&ru, &["Disconnected", "Reconnect", "Restore Pinned", "Forward & Open"], "ports_ui");
        unsafe { gtk_widget_destroy(win) };
        pump_gtk_events();
    }));
}

fn setup_four_sidebar_tabs() -> [usize; 4] {
    let s1 = crate::session_ops::add_tab("Alpha", "", "/tmp", SessionCategory::Project, &["sleep", "30"], Tab::local("a"));
    let s2 = crate::session_ops::add_tab("Beta", "", "/tmp", SessionCategory::Project, &["sleep", "30"], Tab::local("b"));
    let s3 = crate::session_ops::add_tab("Daemon", "", "/tmp", SessionCategory::Daemon, &["sleep", "30"], Tab::local("d"));
    let s4 = crate::session_ops::add_tab("GPU", "", "/tmp", SessionCategory::Remote, &["sleep", "30"], Tab::remote("gpu01", None));
    app_state::with_app_mut(|app| {
        app.sessions[0].state = WorkState::Working;
        app.sessions[1].state = WorkState::Ready;
        app.sessions[1].unread = true;
        app.sessions[3].child_pid = 0;
    });
    [s1, s2, s3, s4]
}

fn collect_topbar_pills() -> (*mut GtkWidget, Vec<String>) {
    app_state::with_app(|app| {
        let pills = vec![
            btn_text(app.box_btn), btn_text(app.servers_btn), btn_text(app.prefs_btn),
            btn_text(app.handoff_btn), btn_text(app.agents_btn), btn_text(app.swarm_btn),
            btn_text(app.reconnect_btn), btn_text(app.ports_menu.button_widget()),
            btn_text(app.tokens_menu.button_widget()),
        ];
        (app.sidebar.panel_widget(), pills)
    }).unwrap()
}

fn with_temp_config_env(f: impl FnOnce()) {
    let dir = std::env::temp_dir().join(format!("akhterm_i18n_cfg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let prev = std::env::var_os("AKHTERM_CONFIG");
    unsafe { std::env::set_var("AKHTERM_CONFIG", dir.join("config.conf")) };
    f();
    match prev {
        Some(v) => unsafe { std::env::set_var("AKHTERM_CONFIG", v) },
        None => unsafe { std::env::remove_var("AKHTERM_CONFIG") },
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sidebar_and_topbar_zero_ru_leak_in_en_and_consistent_in_ru() {
    with_locale_lock(|| with_temp_config_env(|| with_headless_gtk(|| {
        set_locale(UiLocale::En);
        let (win, _) = crate::tests::install_test_window();
        let sids = setup_four_sidebar_tabs();
        crate::topbar::refresh_ui();
        pump_gtk_events();
        let (sidebar_w, pills) = collect_topbar_pills();
        assert_no_cyrillic(&pills, "topbar");
        let mut sb_en = Vec::new();
        collect_all_labels(sidebar_w, &mut sb_en);
        assert_no_cyrillic(&sb_en, "sidebar");
        for ok in ["Working", "Ready", "Online", "Disconnected"] {
            assert!(sb_en.iter().any(|s| s == ok), "missing EN sidebar status {ok}");
        }
        crate::topbar::toggle_locale_for_app();
        crate::topbar::refresh_ui();
        pump_gtk_events();
        let mut sb_ru = Vec::new();
        collect_all_labels(sidebar_w, &mut sb_ru);
        for ok in ["В работе", "Готово", "Онлайн", "Нет связи"] {
            assert!(sb_ru.iter().any(|s| s == ok), "missing RU sidebar status {ok}");
        }
        assert_none_equal(&sb_ru, &["Working", "Ready", "Online", "Disconnected", "PROJECTS & SESSIONS"], "sidebar");
        crate::tests::cleanup_test_window(win, &sids);
    })));
}

fn assert_popover_expands(name: &str, pop: *mut GtkWidget) {
    unsafe { gtk_popover_popup(pop) };
    pump_gtk_events();
    let (mut min_h, mut nat_h) = (0, 0);
    unsafe { gtk_widget_get_preferred_height(pop, &mut min_h, &mut nat_h) };
    assert!(nat_h >= 100, "{name} popover collapsed: nat_h={nat_h}px");
    unsafe { gtk_popover_popdown(pop) };
}

#[test]
fn popover_scrolled_windows_propagate_natural_height_and_do_not_collapse() {
    for rel in [
        "src/swarm/git_status.rs", "src/swarm/git_review.rs", "src/swarm/handoff.rs",
        "src/telemetry/tokens_ui.rs", "src/ui/ports_ui.rs", "src/ui/ssh_ui.rs", "src/ui/palette.rs",
    ] {
        let src = std::fs::read_to_string(rel).expect(rel);
        assert!(src.contains("gtk_scrolled_window_set_propagate_natural_height"), "{rel} missing propagate_natural_height");
    }
    with_locale_lock(|| with_headless_gtk(|| {
        set_locale(UiLocale::En);
        let (win, _) = crate::tests::install_test_window();
        let (t_pop, p_pop, s_pop, t_menu, p_menu) = app_state::with_app(|a| {
            (a.tokens_menu.popover(), a.ports_menu.popover_widget(), a.sys_monitor.popover(), a.tokens_menu, a.ports_menu.clone())
        }).unwrap();
        t_menu.apply(&build_tokens_view_for_mode(&sample_token_rows(), ThemeMode::Dark));
        p_menu.populate(&sample_ports_snap(), UiLocale::En);
        assert_popover_expands("tokens", t_pop);
        assert_popover_expands("ports", p_pop);
        assert_popover_expands("sys_monitor", s_pop);
        crate::tests::cleanup_test_window(win, &[]);
    }));
}
