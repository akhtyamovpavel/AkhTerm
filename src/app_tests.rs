//! Headless GTK integration suite and full-stack Maintainability Index gates.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::{cell::Cell, path::Path, ptr, sync::Arc, sync::Mutex, thread, time::Duration};

use crate::app_state::{self, AppState, APP};
use crate::ffi::*;
use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
use crate::tabs::{Tab, TabStore, TabsState};
use crate::{config::WorkspaceConfig, context_menu, file_drop, git_status, handoff, mi};
use crate::{mi_watch, palette, pane_actions, platform, proc_radar, remote_telemetry, search_bar};
use crate::{session::SessionCategory, session_ops, shortcuts, sidebar, tab_order, tab_strip};
use crate::{telemetry, theme_mode, topbar};

fn make_test_widgets(vbox: *mut GtkWidget) -> [*mut GtkWidget; 4] {
    unsafe {
        let btn = gtk_button_new();
        gtk_box_pack_start(vbox, btn, 0, 0, 0);
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_box_pack_start(vbox, lbl, 0, 0, 0);
        let footer = gtk_label_new(c"".as_ptr());
        gtk_box_pack_start(vbox, footer, 0, 0, 0);
        let stack = gtk_stack_new();
        gtk_box_pack_start(vbox, stack, 1, 1, 0);
        [btn, lbl, footer, stack]
    }
}

pub(crate) fn install_test_window() -> (*mut GtkWidget, *mut GtkWidget) {
    let win = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
    let app = make_test_app(win);
    let footer = app.footer_status;
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    (win, footer)
}

fn make_test_app(win: *mut GtkWidget) -> AppState {
    unsafe {
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_container_add(win, vbox);
        let [btn, lbl, footer, stack] = make_test_widgets(vbox);
        build_test_app_state(win, vbox, btn, lbl, footer, stack)
    }
}

fn make_test_overlays(
    vbox: *mut GtkWidget,
) -> (palette::CommandPalette, search_bar::SearchBar, tab_order::TabPinPopover) {
    let pal = palette::CommandPalette::new(
        vbox, shortcuts::execute_palette_action, app_state::focus_active_vte,
    );
    let bar = search_bar::SearchBar::new(
        shortcuts::active_search_vte, shortcuts::collect_close_search_vtes,
    );
    let pin = tab_order::TabPinPopover::new(
        vbox, session_ops::apply_pinned_tab_title, app_state::focus_active_vte,
    );
    (pal, bar, pin)
}

fn make_test_menus(
    vbox: *mut GtkWidget,
    btn: *mut GtkWidget,
) -> (telemetry::TokensMenu, git_status::GitMenu, handoff::HandoffMenu) {
    let tokens = telemetry::TokensMenu::new(vbox, topbar::tokens_open_cb());
    let git = git_status::GitMenu::new(btn, pane_actions::git_open_cb(), pane_actions::git_key_cb());
    let handoff = handoff::HandoffMenu::new(btn, pane_actions::handoff_open_cb());
    (tokens, git, handoff)
}

fn build_test_app_state(
    win: *mut GtkWidget,
    vbox: *mut GtkWidget,
    btn: *mut GtkWidget,
    lbl: *mut GtkWidget,
    footer: *mut GtkWidget,
    stack: *mut GtkWidget,
) -> AppState {
    let (tokens_menu, git_menu, handoff) = make_test_menus(vbox, btn);
    let (palette, search_bar, pin_pop) = make_test_overlays(vbox);
    let side = sidebar::Sidebar::new(vbox, session_ops::sidebar_search_cb());
    side.set_new_folder_tab_handler(session_ops::new_tab_in_folder_cb());
    let strip = tab_strip::TabStrip::new(vbox, session_ops::new_tab_cb());
    assemble_test_app_state(win, vbox, btn, lbl, footer, stack, side, strip, tokens_menu, git_menu, handoff, palette, search_bar, pin_pop)
}

#[allow(clippy::too_many_arguments)]
fn assemble_test_app_state(
    win: *mut GtkWidget, vbox: *mut GtkWidget, btn: *mut GtkWidget, lbl: *mut GtkWidget,
    footer: *mut GtkWidget, stack: *mut GtkWidget, sidebar: sidebar::Sidebar,
    tab_strip: tab_strip::TabStrip, tokens_menu: telemetry::TokensMenu,
    git_menu: git_status::GitMenu, handoff: handoff::HandoffMenu,
    palette: palette::CommandPalette, search_bar: search_bar::SearchBar,
    pin_pop: tab_order::TabPinPopover,
) -> AppState {
    let swarm_cb = crate::swarm_view::build_app_swarm_callbacks();
    let swarm_pop = crate::swarm_view::SwarmPopover::new(btn, swarm_cb.clone(), crate::i18n::UiLocale::Ru);
    let swarm_view = crate::swarm_view::SwarmView::new(swarm_cb, crate::i18n::UiLocale::Ru);
    swarm_view.mount_in_stack(stack.cast());
    let split_v = unsafe { gtk_button_new() };
    let split_h = unsafe { gtk_button_new() };
    unsafe {
        gtk_box_pack_start(vbox, split_v, 0, 0, 0);
        gtk_box_pack_start(vbox, split_h, 0, 0, 0);
    }
    AppState {
        config: WorkspaceConfig::load(), win, is_fullscreen: false, sidebar, sidebar_visible: true,
        theme_mode: theme_mode::ThemeMode::Dark, theme_btn: btn, box_btn: btn, servers_btn: btn,
        handoff_btn: btn, agents_btn: btn, prefs_btn: btn, swarm_btn: btn, font_scale: 1.0, zoom_btn: btn,
        reconnect_btn: btn, split_vert_btn: split_v, split_horiz_btn: split_h, next_id: 1, active_id: 1,
        sessions: Vec::new(), tab_store: None, stack, tab_strip, mi_btn: btn,
        mi_pop: ptr::null_mut(), mi_pop_label: lbl, mi_refactor_btn: btn, tokens_menu,
        agents_pop: ptr::null_mut(), box_pop: ptr::null_mut(), box_gpu_label: lbl,
        box_tmux_pop_box: vbox, settings_pop: ptr::null_mut(), ssh_count_label: lbl, git_menu,
        handoff, footer_status: footer, footer_hold_until: Cell::new(None), ssh_hosts: Vec::new(),
        ssh_toggle_btns: Vec::new(), ssh_rows: Vec::new(), box_ssh_rows: Vec::new(),
        sidebar_filter: String::new(), box_filter: String::new(),
        enabled_hosts_shared: Arc::new(Mutex::new(Vec::new())), tmux_jump_btns: Vec::new(),
        last_tmux_keys: Vec::new(), mi_scheduler: mi_watch::MiScheduler::default(),
        git_scheduler: git_status::GitStatusScheduler::default(),
        remote_telemetry: remote_telemetry::RemoteTelemetryScheduler::default(),
        box_state: Arc::new(Mutex::new(proc_radar::BoxTelemetry::default())),
        palette, search_bar, pin_pop, swarm_pop, swarm_view,
    }
}

fn set_first_ssh_host(host: &str) {
    APP.with(|c| {
        c.borrow_mut().as_mut().unwrap().sessions[0].proc_info.ssh_host = host.to_owned();
    });
}

fn verify_drop_target_transitions(
    local_sid: usize,
    local_vte: *mut GtkWidget,
    rem_sid: usize,
    rem_vte: *mut GtkWidget,
) {
    assert_eq!(pane_actions::lookup_vte_drop_target(local_vte), file_drop::DropTarget::Local);
    set_first_ssh_host("jump-box");
    assert_eq!(
        pane_actions::lookup_vte_drop_target(local_vte),
        file_drop::DropTarget::Remote { sid: local_sid, host: "jump-box".to_owned() }
    );
    set_first_ssh_host("");
    assert_eq!(pane_actions::lookup_vte_drop_target(local_vte), file_drop::DropTarget::Local);
    assert_eq!(
        pane_actions::lookup_vte_drop_target(rem_vte),
        file_drop::DropTarget::Remote { sid: rem_sid, host: "gpu-box".to_owned() }
    );
}

fn first_two_vtes() -> (*mut GtkWidget, *mut GtkWidget) {
    APP.with(|c| {
        let a = c.borrow();
        let a = a.as_ref().unwrap();
        (a.sessions[0].vte, a.sessions[1].vte)
    })
}

fn verify_app_drop_routing_and_mid_upload_close() {
    pane_actions::install_file_drop_hooks();
    let (win, footer) = install_test_window();
    let local_sid = session_ops::add_tab("Local", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
    let rem_sid = session_ops::add_tab(
        "gpu-box", "", "/tmp", SessionCategory::Remote, &["sh"], Tab::remote("gpu-box", Some("main")),
    );
    let (local_vte, rem_vte) = first_two_vtes();
    verify_drop_target_transitions(local_sid, local_vte, rem_sid, rem_vte);
    verify_mid_upload_close_footer(rem_sid, rem_vte, footer);
    cleanup_test_window(win, &[local_sid]);
}

fn verify_mid_upload_close_footer(rem_sid: usize, rem_vte: *mut GtkWidget, footer: *mut GtkWidget) {
    session_ops::close_session(rem_sid);
    pump_gtk_events();
    assert!(pane_actions::find_live_drop_vte(&[], 0, rem_sid, rem_vte).is_none());
    file_drop::schedule_upload_completion(file_drop::UploadCompletion {
        sid: rem_sid,
        vte_addr: rem_vte as usize,
        injected_text: Some("/tmp/akhterm_closed ".to_owned()),
        footer_msg: "📎 Загружено после закрытия".to_owned(),
    });
    pump_gtk_events();
    let txt = unsafe { CStr::from_ptr(gtk_label_get_text(footer)).to_string_lossy().into_owned() };
    assert_eq!(txt, "📎 Загружено после закрытия");
}

pub(crate) fn cleanup_test_window(win: *mut GtkWidget, sids: &[usize]) {
    for &sid in sids {
        session_ops::close_session(sid);
    }
    APP.with(|cell| *cell.borrow_mut() = None);
    unsafe { gtk_widget_destroy(win) };
    pump_gtk_events();
}

fn decode_saved_pinned(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).expect("tabs-v1");
    TabsState::decode(&raw).expect("valid").pinned_at(0).map(str::to_owned)
}

fn temp_test_path(prefix: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("{prefix}_{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn verify_unified_rename_persists_and_resets_in_tabs_v1() {
    let state_path = temp_test_path("akhterm_k8_tabs");
    let (win, _) = install_test_window();
    APP.with(|c| c.borrow_mut().as_mut().unwrap().tab_store = Some(TabStore::new_for_test(state_path.clone())));
    let sid = session_ops::add_tab("Shell", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
    pane_actions::commit_tab_rename(sid, "Прод ЦУ".to_owned());
    assert_eq!(decode_saved_pinned(&state_path).as_deref(), Some("Прод ЦУ"));
    pane_actions::commit_tab_rename(sid, "   ".to_owned());
    assert_eq!(decode_saved_pinned(&state_path), None);
    cleanup_test_window(win, &[sid]);
    let _ = std::fs::remove_file(&state_path);
}

#[repr(C)]
struct RawTestEventButton {
    type_: c_int, window: *mut c_void, send_event: i8, time: u32,
    x: f64, y: f64, axes: *mut f64, state: c_uint, button: c_uint,
    device: *mut c_void, x_root: f64, y_root: f64,
}

fn current_session_ids() -> Vec<usize> {
    APP.with(|cell| cell.borrow().as_ref().unwrap().sessions.iter().map(|s| s.id).collect())
}

fn set_raw_btn_event(ev: *mut GdkEvent, x: f64, button: c_uint) {
    unsafe {
        (*(ev as *mut RawTestEventButton)).x = x;
        (*(ev as *mut RawTestEventButton)).button = button;
    }
}

fn assert_initial_drag_reorder(
    btn1: *mut GtkWidget,
    press: *mut GdkEvent,
    motion: *mut GdkEvent,
    sid1: usize,
    sid2: usize,
) {
    set_raw_btn_event(press, 12.0, 1);
    (session_ops::tab_button_press_cb())(btn1, press, sid1 as *mut c_void);
    set_raw_btn_event(motion, 15.0, 0);
    assert_eq!((session_ops::tab_motion_cb())(btn1, motion, sid1 as *mut c_void), 0);
    assert_eq!(current_session_ids(), vec![sid1, sid2]);
    set_raw_btn_event(motion, 300.0, 0);
    assert_eq!((session_ops::tab_motion_cb())(btn1, motion, sid1 as *mut c_void), 1);
    let tab_w = APP.with(|c| c.borrow().as_ref().unwrap().sessions[1].top_tab.tab_widget());
    assert!(has_css_class(tab_w, "tab-dragging"), "tab-dragging missing during drag");
    assert_eq!(current_session_ids(), vec![sid2, sid1]);
}

fn has_css_class(widget: *mut GtkWidget, class_name: &str) -> bool {
    let cstr = std::ffi::CString::new(class_name).expect("cstr");
    unsafe { gtk_style_context_has_class(gtk_widget_get_style_context(widget), cstr.as_ptr()) != 0 }
}

fn set_raw_btn_coords(ev: *mut GdkEvent, x: f64, y: f64, button: c_uint) {
    unsafe {
        let raw = &mut *(ev as *mut RawTestEventButton);
        raw.x = x;
        raw.y = y;
        raw.button = button;
    }
}

fn assert_post_drag_shift(
    btn1: *mut GtkWidget,
    tabs_box: *mut GtkWidget,
    motion: *mut GdkEvent,
    sid1: usize,
    sid2: usize,
) {
    thread::sleep(Duration::from_millis(25));
    pump_gtk_events();
    let post_ox = platform::translate_widget_x_to_ancestor(btn1, tabs_box, 0.0).unwrap();
    assert!(post_ox > 20.0, "expected shifted tab allocation, got {post_ox}");
    set_raw_btn_event(motion, 300.0 - post_ox, 0);
    assert_eq!((session_ops::tab_motion_cb())(btn1, motion, sid1 as *mut c_void), 1);
    assert_eq!(current_session_ids(), vec![sid2, sid1]);
}

fn drive_tab_drag_events(
    btn1: *mut GtkWidget,
    tabs_box: *mut GtkWidget,
    sid1: usize,
    sid2: usize,
) {
    unsafe {
        let press = gdk_event_new(GDK_BUTTON_PRESS);
        let motion = gdk_event_new(GDK_MOTION_NOTIFY);
        assert_initial_drag_reorder(btn1, press, motion, sid1, sid2);
        assert_post_drag_shift(btn1, tabs_box, motion, sid1, sid2);
        (session_ops::tab_button_release_cb())(btn1, press, sid1 as *mut c_void);
        let tab_w = APP.with(|c| {
            c.borrow().as_ref().unwrap().sessions.iter().find(|s| s.id == sid1).unwrap().top_tab.tab_widget()
        });
        assert!(!has_css_class(tab_w, "tab-dragging"), "tab-dragging persisted after release");
        app_state::with_app_mut(|a| a.active_id = sid2);
        (session_ops::select_session_cb())(btn1, sid1 as *mut c_void);
        assert_eq!(app_state::active_session_id(), sid2, "post-drag click must be suppressed");
        (session_ops::select_session_cb())(btn1, sid1 as *mut c_void);
        assert_eq!(app_state::active_session_id(), sid1, "plain click must switch active tab");
        gdk_event_free(press);
        gdk_event_free(motion);
    }
}

fn verify_wayland_safe_mouse_tab_drag_without_root_coords() {
    let (win, _) = install_test_window();
    unsafe { gtk_window_set_default_size(win, 640, 320) };
    let (tabs_box, strip_scroll, side_scroll) = APP.with(|c| {
        let a = c.borrow();
        let app = a.as_ref().unwrap();
        (app.tab_strip.tabs_box(), app.tab_strip.scroll_widget(), app.sidebar.scroll_widget())
    });
    assert_eq!(unsafe { gtk_scrolled_window_get_kinetic_scrolling(strip_scroll) }, 0);
    assert_eq!(unsafe { gtk_scrolled_window_get_kinetic_scrolling(side_scroll) }, 0);
    let sid1 = session_ops::add_tab("Длинная вкладка ЦУ", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
    let sid2 = session_ops::add_tab("Кратко", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
    unsafe { gtk_widget_show_all(win) };
    thread::sleep(Duration::from_millis(25));
    pump_gtk_events();
    let btn1 = APP.with(|cell| cell.borrow().as_ref().unwrap().sessions[0].top_tab.select_button());
    drive_tab_drag_events(btn1, tabs_box, sid1, sid2);
    cleanup_test_window(win, &[sid1, sid2]);
}

fn set_test_clipboard_pixbuf() {
    unsafe {
        let clip = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
        let pix = gdk_pixbuf_new(0, 1, 8, 4, 4);
        gtk_clipboard_set_image(clip, pix);
        g_object_unref(pix);
        pump_gtk_events();
    }
}

fn wait_captured_output(path: &Path, needle: &str) -> String {
    for _ in 0..40 {
        pump_gtk_events();
        if std::fs::read_to_string(path).is_ok_and(|s| s.contains(needle)) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    std::fs::read_to_string(path).unwrap_or_default()
}

fn verify_smart_copy_on_vte(vte: *mut GtkWidget) {
    let banner = "Скопируй меня";
    unsafe { vte_terminal_feed(vte, banner.as_ptr() as *const c_char, banner.len() as isize) };
    thread::sleep(Duration::from_millis(40));
    pump_gtk_events();
    unsafe { vte_terminal_select_all(vte) };
    pump_gtk_events();
    assert!(pane_actions::copy_active_selection(true));
    assert!(!pane_actions::copy_active_selection(true));
    assert!(!proc_radar::clipboard_has_direct_image());
}

fn verify_smart_ctrl_c_and_ctrl_v_across_shell_and_agy() {
    let out_file = temp_test_path("akhterm_ctrlv");
    let cmd = format!("cat > '{}'", out_file.display());
    let (win, _) = install_test_window();
    let sid = session_ops::add_tab("AGY", "", "/tmp", SessionCategory::Project, &["sh", "-c", &cmd], Tab::local("shell"));
    let vte = APP.with(|cell| cell.borrow().as_ref().unwrap().sessions[0].vte);
    unsafe { gtk_widget_show_all(win) };
    pump_gtk_events();
    exercise_agy_paste_and_copy(vte, &out_file);
    let _ = std::fs::remove_file(&out_file);
    cleanup_test_window(win, &[sid]);
}

fn exercise_agy_paste_and_copy(vte: *mut GtkWidget, out_file: &Path) {
    APP.with(|c| c.borrow_mut().as_mut().unwrap().sessions[0].proc_info.agent = "AGY".into());
    context_menu::copy_text_to_clipboard("текст для AGY\n");
    pump_gtk_events();
    assert!(pane_actions::paste_plain_ctrl_v());
    set_test_clipboard_pixbuf();
    APP.with(|c| c.borrow_mut().as_mut().unwrap().sessions[0].proc_info.agent = "AGY".into());
    assert!(!pane_actions::paste_plain_ctrl_v());
    verify_smart_copy_on_vte(vte);
    file_drop::feed_vte_text(vte, "\x04", false);
    let captured = wait_captured_output(out_file, "текст для AGY");
    assert!(captured.contains("текст для AGY"), "captured: {captured:?}");
}

#[test]
fn app_routes_local_and_remote_drops_and_ignores_closed_vte_on_idle() {
    with_headless_gtk(|| {
        verify_app_drop_routing_and_mid_upload_close();
        verify_unified_rename_persists_and_resets_in_tabs_v1();
        verify_wayland_safe_mouse_tab_drag_without_root_coords();
        verify_smart_ctrl_c_and_ctrl_v_across_shell_and_agy();
    });
}

fn collect_src_mi_rows() -> Vec<(String, usize, usize, f64, &'static str)> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir("src").expect("src dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let (sloc, cc, score) = mi::compute_file_mi(&std::fs::read_to_string(&path).expect("rs"));
            rows.push((name, sloc, cc, score, mi::grade_for_mi(score)));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn check_min_targets(name: &str, score: f64, targets: &[(&str, f64)], failures: &mut Vec<String>) {
    for &(target, min_mi) in targets {
        if name == target && score < min_mi {
            failures.push(format!("{name} MI {score:.2} < {min_mi:.1}"));
        }
    }
}

fn check_module_mi_gates(name: &str, score: f64, failures: &mut Vec<String>) {
    let core_and_v04: &[(&str, f64)] = &[
        ("main.rs", 58.0), ("platform.rs", 60.0), ("remote_telemetry.rs", 59.0),
        ("telemetry.rs", 58.5), ("telemetry_agy.rs", 62.0), ("context_menu.rs", 58.0),
        ("file_drop.rs", 58.0), ("git_status.rs", 58.0), ("search_bar.rs", 58.0),
        ("ssh_detect.rs", 58.0), ("tab_order.rs", 58.0), ("theme_mode.rs", 58.0),
    ];
    let wave0_and_1: &[(&str, f64)] = &[
        ("about.rs", 60.0), ("app_state.rs", 60.0), ("app_tests.rs", 60.0),
        ("git_review.rs", 60.0), ("git_wt.rs", 60.0), ("i18n.rs", 60.0),
        ("osc_parser.rs", 60.0), ("palette.rs", 58.85), ("pane_actions.rs", 60.0),
        ("pane_header.rs", 60.0), ("pane_radar.rs", 60.0), ("pane_registry.rs", 60.0),
        ("pty_tap.rs", 60.0), ("session_ops.rs", 60.0), ("settings_dialog.rs", 60.0),
        ("shortcuts.rs", 60.0), ("sidebar.rs", 60.0), ("sidebar_folders.rs", 60.0),
        ("ssh_ui.rs", 60.0), ("swarm_view.rs", 60.0),
        ("task_dispatch.rs", 60.0), ("topbar.rs", 60.0),
    ];
    check_min_targets(name, score, core_and_v04, failures);
    check_min_targets(name, score, wave0_and_1, failures);
}

fn assert_remote_split_buttons_enabled() {
    let sensitive = APP.with(|c| {
        let app = c.borrow();
        let a = app.as_ref().unwrap();
        unsafe { gtk_widget_get_sensitive(a.split_vert_btn) != 0 && gtk_widget_get_sensitive(a.split_horiz_btn) != 0 }
    });
    assert!(sensitive, "split buttons must be enabled on remote tabs");
}

fn assert_remote_pane_count_and_headers(sid: usize) {
    let (count, visible) = APP.with(|c| {
        let borrow = c.borrow();
        let app = borrow.as_ref().unwrap();
        let s = app.sessions.iter().find(|s| s.id == sid).unwrap();
        (s.panes.extra.len(), s.panes.all_headers().iter().all(|h| h.is_visible()))
    });
    assert_eq!(count, 1);
    assert!(visible, "2+ remote panes must show 22px PaneHeaders");
}

fn assert_zoom_toggle_for_active(expected: bool) {
    session_ops::toggle_active_pane_zoom();
    pump_gtk_events();
    let zoomed = APP.with(|c| c.borrow().as_ref().unwrap().sessions[0].panes.is_zoomed());
    assert_eq!(zoomed, expected);
}

fn assert_remote_session_split_and_zoom(sid: usize) {
    session_ops::split_pane_in_session(Some(sid), ptr::null_mut(), GTK_ORIENTATION_HORIZONTAL);
    pump_gtk_events();
    assert_remote_pane_count_and_headers(sid);
    assert!(session_ops::focus_pane_direction(crate::split_pane::FocusDir::Left));
    assert_zoom_toggle_for_active(true);
    assert_zoom_toggle_for_active(false);
}

fn install_mock_ssh_dir() -> (std::path::PathBuf, Option<std::ffi::OsString>) {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_test_path("akhterm_mock_ssh_bin");
    std::fs::create_dir_all(&dir).unwrap();
    let ssh_bin = dir.join("ssh");
    std::fs::write(&ssh_bin, "#!/bin/sh\nexec sleep 5\n").unwrap();
    std::fs::set_permissions(&ssh_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let prev = std::env::var_os("PATH");
    let joined = match &prev {
        Some(p) => format!("{}:{}", dir.display(), p.to_string_lossy()),
        None => dir.display().to_string(),
    };
    unsafe { std::env::set_var("PATH", joined) };
    (dir, prev)
}

fn restore_mock_ssh_dir(dir: &Path, prev: Option<std::ffi::OsString>) {
    match prev {
        Some(v) => unsafe { std::env::set_var("PATH", v) },
        None => unsafe { std::env::remove_var("PATH") },
    }
    let _ = std::fs::remove_dir_all(dir);
}

fn verify_remote_splits_zoom_and_tabs_v2_flow() {
    let (ssh_dir, prev_path) = install_mock_ssh_dir();
    let state_path = temp_test_path("akhterm_v2_app_flow");
    let (win, _) = install_test_window();
    APP.with(|c| c.borrow_mut().as_mut().unwrap().tab_store = Some(TabStore::new_for_test(state_path.clone())));
    let sid = session_ops::add_remote_session("gpu01", Some("dev"));
    pump_gtk_events();
    assert_remote_split_buttons_enabled();
    assert_remote_session_split_and_zoom(sid);
    topbar::set_zoom(1.4);
    let saved = TabsState::decode(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert!((saved.font_scale() - 1.4).abs() < 0.002);
    assert!(saved.splits_at(0).is_some(), "remote tmux split persists in tabs-v2");
    cleanup_test_window(win, &[sid]);
    let _ = std::fs::remove_file(&state_path);
    restore_mock_ssh_dir(&ssh_dir, prev_path);
}

#[test]
fn headless_broadway_remote_splits_nav_zoom_headers_and_v2_state() {
    with_headless_gtk(verify_remote_splits_zoom_and_tabs_v2_flow);
}

fn assert_live_prefs_and_locale_wiring() {
    let mut prefs = APP.with(|c| {
        let app = c.borrow();
        let a = app.as_ref().unwrap();
        a.config.to_user_prefs(a.theme_mode)
    });
    prefs.locale = crate::i18n::UiLocale::En;
    prefs.osc52_enabled = false;
    prefs.notify_secs = 25;
    topbar::apply_live_preferences(&prefs);
    assert_eq!(crate::i18n::current_locale(), crate::i18n::UiLocale::En);
    assert!(!crate::pty_tap::is_osc52_enabled());
    assert_eq!(crate::pty_tap::notify_threshold_secs(), 25);
}

fn assert_swarm_palette_and_escape_wiring() {
    shortcuts::execute_palette_action(crate::palette::PaletteAction::ToggleSwarmDashboard);
    pump_gtk_events();
    let stack = APP.with(|c| c.borrow().as_ref().unwrap().stack.cast());
    assert!(crate::swarm_view::is_swarm_active_in_stack(stack));
    assert!(shortcuts::route_swarm_escape(0xff1b));
    pump_gtk_events();
    assert!(!crate::swarm_view::is_swarm_active_in_stack(stack));
}

fn assert_pane_attention_jump_across_split(sid: usize) {
    session_ops::split_pane_in_session(Some(sid), ptr::null_mut(), GTK_ORIENTATION_HORIZONTAL);
    pump_gtk_events();
    let pane2_vte = APP.with(|c| {
        let mut borrow = c.borrow_mut();
        let s = borrow.as_mut().unwrap().sessions.iter_mut().find(|s| s.id == sid).unwrap();
        let vte2 = s.panes.extra[0].vte;
        s.pane_registry.by_vte_mut(vte2).unwrap().state = crate::session::WorkState::NeedsApproval;
        vte2
    });
    shortcuts::jump_next_attention();
    let focused = APP.with(|c| c.borrow().as_ref().unwrap().sessions[0].active_vte());
    assert_eq!(focused, pane2_vte);
}

fn verify_v1_end_to_end_wiring() {
    let prev_loc = crate::i18n::configured_locale();
    let prev_osc = crate::pty_tap::is_osc52_enabled();
    assert!(crate::about::check_version_arg(&["akhterm".into(), "--version".into()])
        .is_some_and(|s| s.starts_with("akhterm 1.0.0 (")));
    let (win, _) = install_test_window();
    let sid = session_ops::add_tab("S1", "", "/tmp", SessionCategory::Project, &["sh"], Tab::local("shell"));
    assert_live_prefs_and_locale_wiring();
    assert_swarm_palette_and_escape_wiring();
    assert_pane_attention_jump_across_split(sid);
    cleanup_test_window(win, &[sid]);
    crate::pty_tap::set_osc52_enabled(prev_osc);
    crate::i18n::set_locale(prev_loc);
}

fn make_folder_drag_repos() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let base = temp_test_path("akhterm_sf_e2e");
    let (repo_a, wt_a, repo_b) = (base.join("AlphaRepo"), base.join(".AlphaRepo-wt/feat-55"), base.join("BetaRepo"));
    let wt_gitdir = repo_a.join(".git/worktrees/feat-55");
    std::fs::create_dir_all(&wt_gitdir).unwrap();
    std::fs::create_dir_all(&wt_a).unwrap();
    std::fs::create_dir_all(repo_b.join(".git")).unwrap();
    std::fs::write(repo_a.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(wt_gitdir.join("HEAD"), "ref: refs/heads/feat-55\n").unwrap();
    std::fs::write(wt_a.join(".git"), format!("gitdir: {}\n", wt_gitdir.display())).unwrap();
    std::fs::write(repo_b.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    (base.clone(), repo_a, wt_a, base.join("tabs-v2.tsv"))
}

fn assert_saved_tab_order(state_path: &Path, first_cwd: &Path, second_cwd: &Path) {
    let saved = std::fs::read_to_string(state_path).unwrap_or_default();
    let fields: Vec<&str> = saved.split('\0').collect();
    let p1 = fields.iter().position(|&f| f == first_cwd.to_string_lossy()).expect("first cwd saved");
    let p2 = fields.iter().position(|&f| f == second_cwd.to_string_lossy()).expect("second cwd saved");
    assert!(saved.starts_with("akhterm-tabs-v2\0") && p1 < p2, "unexpected tabs-v2 order: {saved:?}");
}

fn assert_sidebar_click_suppression(row1: *mut GtkWidget, sid1: usize, sid2: usize) {
    app_state::with_app_mut(|a| a.active_id = sid2);
    (session_ops::select_sidebar_row_cb())(row1, sid1 as *mut c_void);
    assert_eq!(app_state::active_session_id(), sid2, "sidebar post-drag click suppressed");
    (session_ops::select_sidebar_row_cb())(row1, sid1 as *mut c_void);
    assert_eq!(app_state::active_session_id(), sid1, "sidebar plain click selects");
}

fn assert_sidebar_drag_reorders(row1: *mut GtkWidget, sid1: usize, sid2: usize) {
    unsafe {
        let (press, motion) = (gdk_event_new(GDK_BUTTON_PRESS), gdk_event_new(GDK_MOTION_NOTIFY));
        set_raw_btn_coords(press, 10.0, 12.0, 1);
        (session_ops::sidebar_button_press_cb())(row1, press, sid1 as *mut c_void);
        set_raw_btn_coords(motion, 10.0, 90.0, 0);
        assert_eq!((session_ops::sidebar_motion_cb())(row1, motion, sid1 as *mut c_void), 1);
        assert!(has_css_class(row1, "sidebar-row-dragging"));
        assert_alt_slot_and_order(sid1, sid2);
        (session_ops::sidebar_button_release_cb())(row1, press, sid1 as *mut c_void);
        assert!(!has_css_class(row1, "sidebar-row-dragging"));
        gdk_event_free(press);
        gdk_event_free(motion);
    }
}

fn assert_alt_slot_and_order(sid1: usize, sid2: usize) {
    assert_eq!(&current_session_ids()[..2], &[sid2, sid1]);
    let alt1 = tab_order::alt_tab_index(b'1' as u32, GDK_MOD1_MASK).unwrap();
    let alt2 = tab_order::alt_tab_index(b'2' as u32, GDK_MOD1_MASK).unwrap();
    APP.with(|c| {
        let a = c.borrow();
        let sessions = &a.as_ref().unwrap().sessions;
        assert_eq!((sessions[alt1].id, sessions[alt2].id), (sid2, sid1));
    });
}

fn apply_test_sidebar_filter(query: &str) {
    APP.with(|c| {
        let a = c.borrow();
        let app = a.as_ref().unwrap();
        app.sidebar.filter(query, &app.sessions, std::iter::empty());
    });
}

fn assert_folder_collapse_and_search(toggle_a: *mut GtkWidget, rows_a: *mut GtkWidget, root_b: *mut GtkWidget) {
    unsafe { gtk_button_clicked(toggle_a) };
    pump_gtk_events();
    assert_eq!(unsafe { gtk_widget_get_visible(rows_a) }, 0, "folder A should collapse");
    apply_test_sidebar_filter("feat-55");
    assert_eq!(unsafe { gtk_widget_get_visible(rows_a) }, 1, "search auto-expands matching folder");
    assert_eq!(unsafe { gtk_widget_get_visible(root_b) }, 0, "search hides 0-match folder");
    apply_test_sidebar_filter("");
    assert_eq!(unsafe { gtk_widget_get_visible(rows_a) }, 0, "collapsed state restored after search");
}

fn verify_folder_grouping_collapse_add_and_search(repo_a: &Path, repo_b: &Path) -> usize {
    let (key_a, key_b) = (repo_a.to_string_lossy().into_owned(), repo_b.to_string_lossy().into_owned());
    assert_eq!(APP.with(|c| c.borrow().as_ref().unwrap().sidebar.folder_keys()), vec![key_a.clone(), key_b.clone()]);
    let (_, toggle_a, add_a, rows_a) = APP.with(|c| c.borrow().as_ref().unwrap().sidebar.folder_widgets(&key_a).unwrap());
    let (root_b, _, _, _) = APP.with(|c| c.borrow().as_ref().unwrap().sidebar.folder_widgets(&key_b).unwrap());
    assert_folder_collapse_and_search(toggle_a, rows_a, root_b);
    unsafe { gtk_button_clicked(add_a) };
    pump_gtk_events();
    assert_eq!(unsafe { gtk_widget_get_visible(rows_a) }, 1, "+ button expands folder");
    APP.with(|c| {
        let a = c.borrow();
        let last = a.as_ref().unwrap().sessions.last().unwrap();
        assert_eq!(last.cwd, key_a);
        last.id
    })
}

fn setup_sidebar_drag_sessions(repo_a: &Path, wt_a: &Path, repo_b: &Path, state_path: &Path) -> (*mut GtkWidget, usize, usize, usize) {
    let (win, _) = install_test_window();
    unsafe { gtk_window_set_default_size(win, 640, 480) };
    APP.with(|c| c.borrow_mut().as_mut().unwrap().tab_store = Some(TabStore::new_for_test(state_path.to_path_buf())));
    let sid1 = session_ops::add_tab("Alpha", "", &repo_a.to_string_lossy(), SessionCategory::Project, &["sh"], Tab::local("shell"));
    let sid2 = session_ops::add_tab("Alpha WT", "", &wt_a.to_string_lossy(), SessionCategory::Project, &["sh"], Tab::local("shell"));
    let sid3 = session_ops::add_tab("Beta", "", &repo_b.to_string_lossy(), SessionCategory::Project, &["sh"], Tab::local("shell"));
    topbar::refresh_ui();
    unsafe { gtk_widget_show_all(win) };
    thread::sleep(Duration::from_millis(25));
    pump_gtk_events();
    (win, sid1, sid2, sid3)
}

fn verify_sidebar_drag_and_folder_grouping() {
    let (base, repo_a, wt_a, state_path) = make_folder_drag_repos();
    let repo_b = base.join("BetaRepo");
    let (win, sid1, sid2, sid3) = setup_sidebar_drag_sessions(&repo_a, &wt_a, &repo_b, &state_path);
    let row1 = APP.with(|c| c.borrow().as_ref().unwrap().sessions[0].sidebar_row.row_widget());
    let detail2 = APP.with(|c| c.borrow().as_ref().unwrap().sessions[1].sidebar_row.detail_text_for_test());
    assert!(detail2.contains("[wt]"), "linked worktree row must preserve [wt] detail, got {detail2:?}");
    assert_sidebar_drag_reorders(row1, sid1, sid2);
    assert_sidebar_click_suppression(row1, sid1, sid2);
    assert_saved_tab_order(&state_path, &wt_a, &repo_a);
    let sid4 = verify_folder_grouping_collapse_add_and_search(&repo_a, &repo_b);
    cleanup_test_window(win, &[sid1, sid2, sid3, sid4]);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn headless_broadway_sidebar_drag_and_folder_grouping() {
    with_headless_gtk(verify_sidebar_drag_and_folder_grouping);
}

#[test]
fn headless_broadway_v1_e2e_wiring() {
    crate::i18n::with_locale_lock(|| with_headless_gtk(verify_v1_end_to_end_wiring));
}

#[test]
fn k8_full_stack_maintainability_index_targets() {
    let main_lines = std::fs::read_to_string("src/main.rs").expect("read main.rs").lines().count();
    let mut failures = Vec::new();
    if main_lines >= 900 {
        failures.push(format!("src/main.rs has {main_lines} lines (must be < 900)"));
    }
    for (name, sloc, cc, score, grade) in collect_src_mi_rows() {
        println!("{name:<19} | SLOC {sloc:>4} | CC {cc:>2} | MI {score:>6.2} | Grade {grade}");
        check_module_mi_gates(&name, score, &mut failures);
    }
    assert!(failures.is_empty(), "MI gate failures:\n{}", failures.join("\n"));
}
