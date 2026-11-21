//! Unit and headless Broadway GTK tests for `ports_ui`, resilient tunnels, and pinned restore.

use super::*;
use crate::i18n::{set_locale, with_locale_lock};
use crate::port_radar::BindScope;
use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
use std::ffi::CStr;

fn sample_tunnel(host: &str, remote_port: u16, local_port: u16) -> TunnelInfo {
    TunnelInfo {
        id: 1,
        host: host.to_owned(),
        remote_port,
        local_port,
        scheme: "http".to_owned(),
        process: "uvicorn".to_owned(),
        session_ids: vec![1],
        pid: 9999,
        status: TunnelStatus::Active,
    }
}

fn sample_port(port: u16, process: &str) -> DetectedPort {
    DetectedPort {
        port,
        process: process.to_owned(),
        scheme: "http".to_owned(),
        scope: BindScope::Loopback,
    }
}

#[test]
fn formats_bilingual_button_and_tunnel_labels_without_text_arrows() {
    let idle = PortsSnapshot::default();
    assert_eq!(format_ports_button_label(&idle, UiLocale::Ru), "Порты ▾");
    assert_eq!(format_ports_button_label(&idle, UiLocale::En), "Ports ▾");

    let single = PortsSnapshot { local_ports: vec![sample_port(8000, "uvicorn")], ..idle.clone() };
    assert_eq!(format_ports_button_label(&single, UiLocale::Ru), "Порты :8000 ▾");
    assert_eq!(format_ports_button_label(&single, UiLocale::En), "Ports :8000 ▾");

    let multi = PortsSnapshot {
        remote_host: Some("gpu01".into()),
        remote_ports: vec![sample_port(8000, "uvicorn"), sample_port(5173, "vite")],
        tunnels: vec![sample_tunnel("gpu01", 8000, 18000)],
        ..idle
    };
    assert_eq!(format_ports_button_label(&multi, UiLocale::Ru), "Порты :18000 +1 ▾");
    assert_eq!(format_ports_button_label(&multi, UiLocale::En), "Ports :18000 +1 ▾");
}

#[test]
fn formats_tunnel_labels_in_ru_and_en_without_text_arrows() {
    let t = sample_tunnel("gpu01", 8000, 18000);
    let t_ru = format_tunnel_label(&t, UiLocale::Ru);
    let t_en = format_tunnel_label(&t, UiLocale::En);
    assert_eq!(t_ru, "gpu01 :8000 на :18000 · uvicorn");
    assert_eq!(t_en, "gpu01 :8000 to :18000 · uvicorn");
    for s in [&t_ru, &t_en] {
        assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"));
    }
}

fn btn_label_text(btn: *mut GtkWidget) -> String {
    unsafe { CStr::from_ptr(gtk_button_get_label(btn)).to_string_lossy().into_owned() }
}

fn remote_demo_snap() -> PortsSnapshot {
    PortsSnapshot {
        session_id: 2,
        remote_host: Some("gpu-box".into()),
        remote_ports: vec![sample_port(8000, "uvicorn"), sample_port(8888, "jupyter")],
        tunnels: vec![sample_tunnel("gpu-box", 8000, 18000)],
        ..PortsSnapshot::default()
    }
}

fn mount_test_ports_bar() -> (*mut GtkWidget, PortsMenu) {
    unsafe {
        let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        let bar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
        gtk_container_add(win, bar);
        let menu = PortsMenu::new(bar);
        gtk_widget_show_all(win);
        (win, menu)
    }
}

#[test]
fn headless_broadway_ports_popover_renders_local_ports() {
    with_headless_gtk(|| unsafe {
        let (win, menu) = mount_test_ports_bar();
        let local = PortsSnapshot {
            session_id: 1,
            local_ports: vec![sample_port(8000, "uvicorn"), sample_port(5173, "node")],
            ..PortsSnapshot::default()
        };
        menu.sync_button(&local, UiLocale::En);
        menu.populate(&local, UiLocale::En);
        menu.popup();
        pump_gtk_events();
        assert!(!menu.popover_widget().is_null());
        assert_eq!(btn_label_text(menu.button_widget()), "Ports :8000 +1 ▾");
        menu.popdown();
        gtk_widget_destroy(win);
        pump_gtk_events();
    });
}

#[test]
fn headless_broadway_ports_popover_renders_remote_and_tunnels() {
    with_headless_gtk(|| unsafe {
        let (win, menu) = mount_test_ports_bar();
        let remote = remote_demo_snap();
        menu.sync_button(&remote, UiLocale::Ru);
        menu.populate(&remote, UiLocale::Ru);
        menu.popup();
        pump_gtk_events();
        assert_eq!(btn_label_text(menu.button_widget()), "Порты :18000 +1 ▾");
        menu.popdown();
        gtk_widget_destroy(win);
        pump_gtk_events();
    });
}

fn take_test_urls() -> Vec<String> {
    TEST_OPENED_URLS.with(|slot| slot.borrow_mut().take().unwrap_or_default())
}

fn setup_ctrl_click_tabs() -> (*mut GtkWidget, usize, usize, *mut GtkWidget, *mut GtkWidget) {
    let (win, _) = crate::tests::install_test_window();
    app_state::with_app_mut(|app| {
        app.ssh_tunnels = crate::ssh_tunnel::SshTunnelManager::with_test_command("sleep", &["60"]);
    });
    let local_sid = crate::session_ops::add_tab(
        "Local", "", "/tmp", SessionCategory::Project, &["sleep", "30"], crate::tabs::Tab::local("shell"),
    );
    let remote_sid = crate::session_ops::add_tab(
        "GPU", "", "/tmp", SessionCategory::Remote, &["sleep", "30"], crate::tabs::Tab::remote("gpu01", None),
    );
    let (lv, rv) = app_state::with_app(|app| (app.sessions[0].vte, app.sessions[1].vte)).unwrap();
    (win, local_sid, remote_sid, lv, rv)
}

#[test]
fn ctrl_click_hook_forwards_remote_localhost_and_cleans_up_on_tab_close() {
    with_headless_gtk(|| {
        TEST_OPENED_URLS.with(|slot| *slot.borrow_mut() = Some(Vec::new()));
        let (win, local_sid, remote_sid, local_vte, remote_vte) = setup_ctrl_click_tabs();
        assert!(!intercept_vte_link_open(local_vte, "http://localhost:8000/docs"));
        assert!(!intercept_vte_link_open(remote_vte, "https://example.com:8000/docs"));
        assert!(intercept_vte_link_open(remote_vte, "http://localhost:8000/docs?x=1"));
        assert!(intercept_vte_link_open(remote_vte, "http://localhost:8000/docs?x=1"));
        let tunnels = app_state::with_app(|app| app.ssh_tunnels.list_tunnels_snapshot()).unwrap();
        assert_eq!(tunnels.len(), 1);
        let expected = format!("http://127.0.0.1:{}/docs?x=1", tunnels[0].local_port);
        assert!(take_test_urls().contains(&expected));
        let pid = tunnels[0].pid;
        crate::session_ops::close_session(remote_sid);
        assert!(app_state::with_app(|app| app.ssh_tunnels.list_tunnels_snapshot()).unwrap().is_empty());
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        crate::tests::cleanup_test_window(win, &[local_sid]);
    });
}

#[test]
fn command_palette_includes_bilingual_ports_entry() {
    let src = crate::palette::PaletteSource {
        sessions: &[], tmux_items: &[], profiles: &[], ssh_hosts: &[],
        briefs_dir: None, extra_handoffs: &[], active_git: None, active_is_worktree: false,
    };
    let ru = crate::palette::collect_items_for_locale(&src, UiLocale::Ru);
    let en = crate::palette::collect_items_for_locale(&src, UiLocale::En);
    assert!(ru.iter().any(|i| i.action == crate::palette::PaletteAction::OpenPortsMenu && i.title.contains("Порты")));
    assert!(en.iter().any(|i| i.action == crate::palette::PaletteAction::OpenPortsMenu && i.title.contains("Ports")));
}

#[test]
fn dark_and_light_theme_css_load_ports_classes_cleanly() {
    with_headless_gtk(|| {
        crate::theme::apply_gtk_theme_mode(crate::theme_mode::ThemeMode::Dark);
        crate::theme::apply_gtk_theme_mode(crate::theme_mode::ThemeMode::Light);
        crate::theme::apply_gtk_theme_mode(crate::theme_mode::ThemeMode::Dark);
    });
}

fn open_remote_popover_for_test() -> (*mut GtkWidget, usize, PortsMenu) {
    let (win, _) = crate::tests::install_test_window();
    let sid = crate::session_ops::add_tab(
        "GPU", "", "/tmp", SessionCategory::Remote, &["sleep", "30"], crate::tabs::Tab::remote("gpu01", None),
    );
    unsafe { gtk_widget_show_all(win) };
    open_ports_popover_for_app();
    pump_gtk_events();
    let menu = app_state::with_app(|app| app.ports_menu.clone()).unwrap();
    (win, sid, menu)
}

#[test]
fn remote_ports_ready_repopulates_open_popover_and_skips_closed() {
    with_headless_gtk(|| {
        let (win, sid, menu) = open_remote_popover_for_test();
        assert!(menu.is_visible());
        let before = menu.populate_count();
        app_state::with_app(|app| app.remote_ports.seed_cache_for_test("gpu01", vec![sample_port(8000, "uvicorn")]));
        (remote_ports_ready_cb())(ptr::null_mut());
        pump_gtk_events();
        assert_eq!(menu.populate_count(), before + 1);
        assert!(btn_label_text(menu.button_widget()).contains(":8000"));
        menu.popdown();
        pump_gtk_events();
        (remote_ports_ready_cb())(ptr::null_mut());
        assert_eq!(menu.populate_count(), before + 1);
        crate::tests::cleanup_test_window(win, &[sid]);
    });
}

fn spawn_test_tunnel_in_app() -> (u64, u32, PortsMenu) {
    app_state::with_app_mut(|app| {
        app.ssh_tunnels = crate::ssh_tunnel::SshTunnelManager::with_test_command("sleep", &["60"]);
        let req = TunnelRequest {
            host: "gpu01".into(), ssh_pid: 0, remote_port: 8000,
            scheme: "http".into(), process: "uvicorn".into(), session_id: 1,
        };
        let res = app.ssh_tunnels.ensure_tunnel(req).unwrap();
        (res.info.id, res.info.pid, app.ports_menu.clone())
    }).unwrap()
}

#[test]
fn ports_popover_propagates_natural_height_for_populated_rows() {
    with_headless_gtk(|| {
        let (win, _) = crate::tests::install_test_window();
        let menu = app_state::with_app(|app| app.ports_menu.clone()).unwrap();
        let snap = PortsSnapshot {
            local_ports: vec![sample_port(3000, "vite"), sample_port(8000, "uvicorn")],
            ..PortsSnapshot::default()
        };
        menu.populate(&snap, UiLocale::Ru);
        menu.popup();
        pump_gtk_events();
        let (mut min_h, mut nat_h) = (0, 0);
        unsafe { gtk_widget_get_preferred_height(menu.popover_widget(), &mut min_h, &mut nat_h) };
        assert!(nat_h >= 120, "expected expanded Ports popover natural height >= 120px, got {nat_h}px");
        menu.popdown();
        crate::tests::cleanup_test_window(win, &[]);
    });
}

#[test]
fn stop_tunnel_row_click_safely_destroys_button_and_stops_child() {
    with_headless_gtk(|| {
        let (win, _) = crate::tests::install_test_window();
        let (tid, pid, menu) = spawn_test_tunnel_in_app();
        open_ports_popover_for_app();
        let act = PortRowAction::StopTunnel { id: tid, label: "gpu01 :8000 to :8000".into() };
        let btn = make_action_button("×", "ports-stop-btn", act);
        unsafe {
            gtk_box_pack_start(menu.inner_widget(), btn, 0, 0, 0);
            gtk_button_clicked(btn.cast());
        }
        pump_gtk_events();
        assert!(app_state::with_app(|app| app.ssh_tunnels.list_tunnels_snapshot()).unwrap().is_empty());
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        crate::tests::cleanup_test_window(win, &[]);
    });
}

#[repr(C)]
struct TestGList {
    data: *mut c_void,
    next: *mut TestGList,
    prev: *mut TestGList,
}

extern "C" {
    fn gtk_container_get_children(container: *mut GtkWidget) -> *mut TestGList;
    fn g_list_free(list: *mut TestGList);
}

fn collect_widget_texts(root: *mut GtkWidget, out: &mut Vec<String>) {
    if root.is_null() {
        return;
    }
    let kind = unsafe { CStr::from_ptr(g_type_name_from_instance(root)).to_string_lossy() };
    if kind == "GtkLabel" {
        let lbl = unsafe { gtk_label_get_text(root) };
        if !lbl.is_null() {
            let s = unsafe { CStr::from_ptr(lbl).to_string_lossy().into_owned() };
            if !s.is_empty() { out.push(s); }
        }
        return;
    }
    if kind == "GtkButton" {
        let lbl = unsafe { gtk_button_get_label(root) };
        if !lbl.is_null() {
            let s = unsafe { CStr::from_ptr(lbl).to_string_lossy().into_owned() };
            if !s.is_empty() { out.push(s); }
        }
    }
    if matches!(kind.as_ref(), "GtkImage" | "GtkEntry" | "GtkSearchEntry" | "GtkSeparator") {
        return;
    }
    unsafe {
        let head = gtk_container_get_children(root);
        let mut cur = head;
        while !cur.is_null() {
            collect_widget_texts((*cur).data as *mut GtkWidget, out);
            cur = (*cur).next;
        }
        if !head.is_null() {
            g_list_free(head);
        }
    }
}

#[test]
fn headless_broadway_disconnected_tunnel_reconnect_and_pinned_restore() {
    with_locale_lock(|| with_headless_gtk(|| {
        let root = std::env::temp_dir().join(format!("akhterm_ports_pin_ui_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let prev = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", &root) };
        set_locale(UiLocale::En);

        let (win, _) = crate::tests::install_test_window();
        let sid = crate::session_ops::add_tab(
            "GPU", "", "/tmp", SessionCategory::Remote, &["sleep", "30"], crate::tabs::Tab::remote("gpu01", None),
        );
        let tid = app_state::with_app_mut(|app| {
            app.ssh_tunnels = crate::ssh_tunnel::SshTunnelManager::with_test_command("true", &[]);
            let req = TunnelRequest {
                host: "gpu01".into(), ssh_pid: 0, remote_port: 8000,
                scheme: "http".into(), process: "uvicorn".into(), session_id: sid,
            };
            app.ssh_tunnels.ensure_tunnel(req).unwrap().info.id
        }).unwrap();
        for _ in 0..40 {
            let alive = app_state::with_app_mut(|app| app.ssh_tunnels.is_tunnel_alive(tid)).unwrap();
            if !alive { break; }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }

        handle_row_action(PortRowAction::PinTunnel(PinnedTunnel {
            host: "gpu01".into(), remote_port: 8000, local_port: 18000, scheme: "http".into(), process: "uvicorn".into(),
        }));
        open_ports_popover_for_app();
        pump_gtk_events();
        let inner = app_state::with_app(|app| app.ports_menu.inner_widget()).unwrap();
        let mut texts = Vec::new();
        collect_widget_texts(inner, &mut texts);
        assert!(texts.iter().any(|t| t == "Disconnected"), "expected Disconnected badge: {texts:?}");
        assert!(texts.iter().any(|t| t == "Reconnect"), "expected Reconnect button: {texts:?}");
        assert!(texts.iter().any(|t| t == "Unpin"), "expected Unpin button: {texts:?}");
        assert!(texts.iter().any(|t| t == "Restore Pinned"), "expected Restore Pinned button: {texts:?}");

        app_state::with_app_mut(|app| app.ssh_tunnels.set_test_command("sleep", &["60"]));
        handle_row_action(PortRowAction::ReconnectTunnel { id: tid, ssh_pid: 0, host: "gpu01".into(), remote_port: 8000 });
        let snap = app_state::with_app(|app| app.ssh_tunnels.list_tunnels_snapshot()).unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].status, TunnelStatus::Active);

        handle_row_action(PortRowAction::StopTunnel { id: tid, label: "gpu01".into() });
        assert!(app_state::with_app(|app| app.ssh_tunnels.list_tunnels_snapshot()).unwrap().is_empty());
        handle_row_action(PortRowAction::RestorePinned { host: "gpu01".into(), ssh_pid: 0, sid });
        let restored = app_state::with_app(|app| app.ssh_tunnels.list_tunnels_snapshot()).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].status, TunnelStatus::Active);

        crate::tests::cleanup_test_window(win, &[sid]);
        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
        let _ = std::fs::remove_dir_all(&root);
    }));
}
