//! AkhTerm binary entry point and GTK application bootstrap.

mod about;
mod app_state;
#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
mod approval;
mod config;
mod context_menu;
mod ffi;
mod file_drop;
mod git_status;
mod git_wt;
mod handoff;
mod i18n;
mod mi;
mod mi_watch;
mod osc_parser;
mod palette;
mod pane_actions;
mod pane_header;
mod pane_radar;
mod pane_registry;
mod paste_route;
mod paths;
mod platform;
mod proc_radar;
mod pty_tap;
mod remote_telemetry;
mod search_bar;
mod session;
mod session_ops;
mod settings_dialog;
mod shortcuts;
mod sidebar;
mod split_pane;
mod ssh_detect;
mod ssh_ui;
mod tab_order;
mod tab_strip;
mod tabs;
mod telemetry;
mod telemetry_agy;
mod theme;
mod theme_mode;
mod topbar;
mod ui_font;
mod ui_text;

use std::cell::Cell;
use std::ffi::{c_int, c_void, CString};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use app_state::{focus_active_vte, AppState, APP};
use config::WorkspaceConfig;
use ffi::*;
use git_status::{GitMenu, GitStatusScheduler};
use handoff::HandoffMenu;
use mi_watch::MiScheduler;
use proc_radar::{load_ssh_hosts, query_ssh_servers, BoxTelemetry};
use remote_telemetry::RemoteTelemetryScheduler;
use session::{register_link_cwd_lookup, register_vte_menu_hook};

pub use app_state::{connect_click, connect_entry_changed};
#[cfg(test)]
pub(crate) use pane_actions::find_live_drop_vte;
pub(crate) use pane_actions::install_file_drop_hooks;
pub use session_ops::select_session;
pub use topbar::{adjust_zoom, refresh_ui, set_zoom};

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if let Some(version_line) = about::check_version_arg(&args) {
        println!("{version_line}");
        return std::process::ExitCode::SUCCESS;
    }
    let _workspace = match tabs::lock_workspace() {
        Ok(file) => file,
        Err(error) => {
            eprintln!("AkhTerm: cannot lock workspace: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    run_app();
    std::process::ExitCode::SUCCESS
}

fn init_app_hooks() {
    platform::init_process_identity();
    let _ui_font = ui_font::load().inspect_err(|e| eprintln!("AkhTerm: {e}"));
    register_link_cwd_lookup(session_ops::lookup_vte_link_cwd);
    install_file_drop_hooks();
    register_vte_menu_hook(pane_actions::open_vte_context_menu);
    pane_header::register_pane_close_hook(pane_actions::close_pane_by_vte);
}

fn init_gtk_theme(initial_theme: theme_mode::ThemeMode) {
    unsafe {
        gtk_init(ptr::null_mut(), ptr::null_mut());
        theme::apply_gtk_theme_mode(initial_theme);
        const SIGTERM: c_int = 15;
        g_unix_signal_add(SIGTERM, session_ops::terminate_cb(), ptr::null_mut());
    }
}

fn create_main_window() -> (*mut GtkWidget, *mut GtkWidget) {
    let backend = platform::detect_display_backend();
    let desktop = platform::detect_desktop_environment();
    unsafe {
        let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        gtk_window_set_title(win, ui_text::APP_NAME.as_ptr());
        platform::configure_window_identity(win, backend, desktop);
        configure_window_chrome(win)
    }
}

fn configure_window_chrome(win: *mut GtkWidget) -> (*mut GtkWidget, *mut GtkWidget) {
    unsafe {
        gtk_window_set_decorated(win, 1);
        gtk_window_set_resizable(win, 1);
        gtk_window_set_default_size(win, 1480, 900);
        add_class(win, "main-bg");
        connect_window_signals(win);
        let root_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_container_add(win, root_vbox);
        topbar::build_header_bar(win);
        (win, root_vbox)
    }
}

fn connect_window_signals(win: *mut GtkWidget) {
    let sig_destroy = CString::new("destroy").unwrap();
    let sig_key = CString::new("key-press-event").unwrap();
    let on_destroy = session_ops::window_destroy_cb() as *const c_void;
    let on_key = shortcuts::window_key_press_cb() as *const c_void;
    unsafe {
        g_signal_connect_data(win, sig_destroy.as_ptr(), on_destroy, ptr::null_mut(), ptr::null(), 0);
        g_signal_connect_data(win, sig_key.as_ptr(), on_key, ptr::null_mut(), ptr::null(), 0);
    }
}

fn build_workspace_area(root_vbox: *mut GtkWidget) -> (sidebar::Sidebar, search_bar::SearchBar, *mut GtkWidget) {
    unsafe {
        let hbox = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
        gtk_box_pack_start(root_vbox, hbox, 1, 1, 0);
        let sidebar = sidebar::Sidebar::new(hbox, session_ops::sidebar_search_cb());
        let term_vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_box_pack_start(hbox, term_vbox, 1, 1, 0);
        let search_bar = search_bar::SearchBar::new(shortcuts::active_search_vte, shortcuts::collect_close_search_vtes);
        gtk_box_pack_start(term_vbox, search_bar.widget(), 0, 1, 0);
        let stack = gtk_stack_new();
        gtk_box_pack_start(term_vbox, stack, 1, 1, 0);
        (sidebar, search_bar, stack)
    }
}

fn build_footer_bar(root_vbox: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let footer_box = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        add_class(footer_box, "footer-bar");
        let lbl = gtk_label_new(CString::new("Запуск ядра VTE-2.91...").unwrap().as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        gtk_box_pack_start(footer_box, lbl, 1, 1, 0);
        gtk_box_pack_start(root_vbox, footer_box, 0, 1, 0);
        lbl
    }
}

struct Schedulers {
    mi: MiScheduler,
    git: GitStatusScheduler,
    remote: RemoteTelemetryScheduler,
    box_state: Arc<Mutex<BoxTelemetry>>,
    enabled_hosts: Arc<Mutex<Vec<String>>>,
}

fn make_schedulers(config: &WorkspaceConfig, ssh_hosts: &[proc_radar::SshHostEntry]) -> Schedulers {
    let enabled: Vec<String> = ssh_hosts.iter().filter(|h| h.enabled).map(|h| h.alias.clone()).collect();
    let mi = MiScheduler::default();
    mi.sync_targets(&config.mi_directories(), &[], &paths::home_dir());
    Schedulers {
        mi, git: GitStatusScheduler::default(), remote: RemoteTelemetryScheduler::default(),
        box_state: Arc::new(Mutex::new(BoxTelemetry::default())), enabled_hosts: Arc::new(Mutex::new(enabled)),
    }
}

fn spawn_mi_worker(mi_bg: MiScheduler) {
    thread::spawn(move || loop {
        if mi_bg.refresh_due(Instant::now()) > 0 { unsafe { g_idle_add(topbar::mi_ready_cb(), ptr::null_mut()) }; }
        mi_bg.wait_for_turn(Duration::from_secs(5));
    });
}

fn spawn_git_worker(git_bg: GitStatusScheduler) {
    thread::spawn(move || loop {
        if git_bg.refresh_due(Instant::now()) > 0 { unsafe { g_idle_add(pane_actions::git_status_ready_cb(), ptr::null_mut()) }; }
        git_bg.wait_for_turn(Duration::from_secs(4));
    });
}

fn spawn_remote_worker(remote_bg: RemoteTelemetryScheduler) {
    thread::spawn(move || loop {
        if remote_bg.refresh_due(Instant::now()) > 0 { unsafe { g_idle_add(session_ops::remote_telemetry_cb(), ptr::null_mut()) }; }
        remote_bg.wait_for_turn(Duration::from_secs(2));
    });
}

fn spawn_ssh_worker(box_bg: Arc<Mutex<BoxTelemetry>>, hosts_bg: Arc<Mutex<Vec<String>>>) {
    thread::spawn(move || loop {
        let hosts = hosts_bg.lock().ok().map(|g| g.clone()).unwrap_or_default();
        let telem = query_ssh_servers(&hosts);
        if let Ok(mut g) = box_bg.lock() { *g = telem; }
        thread::sleep(Duration::from_secs(3));
    });
}

fn spawn_background_workers(sched: &Schedulers) {
    spawn_mi_worker(sched.mi.clone());
    spawn_git_worker(sched.git.clone());
    spawn_remote_worker(sched.remote.clone());
    spawn_ssh_worker(Arc::clone(&sched.box_state), Arc::clone(&sched.enabled_hosts));
}

struct AppAssembly {
    config: WorkspaceConfig,
    win: *mut GtkWidget,
    initial_theme: theme_mode::ThemeMode,
    tb: topbar::TopbarWidgets,
    settings_pop: ssh_ui::SettingsPopoverWidgets,
    box_pop: ssh_ui::BoxPopoverWidgets,
    sidebar: sidebar::Sidebar,
    search_bar: search_bar::SearchBar,
    stack: *mut GtkWidget,
    footer_status: *mut GtkWidget,
    ssh_hosts: Vec<proc_radar::SshHostEntry>,
    sched: Schedulers,
}

fn install_app_state(a: AppAssembly) {
    let git_menu = GitMenu::new(a.tb.git_btn, pane_actions::git_open_cb(), pane_actions::git_key_cb());
    let handoff = HandoffMenu::new(a.tb.handoff_btn, pane_actions::handoff_open_cb());
    let palette = palette::CommandPalette::new(a.tb.topbar, shortcuts::execute_palette_action, focus_active_vte);
    let pin_pop = tab_order::TabPinPopover::new(a.tb.topbar, session_ops::apply_pinned_tab_title, focus_active_vte);
    APP.with(|cell| *cell.borrow_mut() = Some(build_app_state(a, git_menu, handoff, palette, pin_pop)));
}

fn build_app_state(
    a: AppAssembly,
    git_menu: GitMenu,
    handoff: HandoffMenu,
    palette: palette::CommandPalette,
    pin_pop: tab_order::TabPinPopover,
) -> AppState {
    AppState {
        config: a.config, win: a.win, is_fullscreen: false, sidebar: a.sidebar,
        sidebar_visible: true, theme_mode: a.initial_theme, theme_btn: a.tb.theme_btn,
        font_scale: 1.0, zoom_btn: a.tb.zoom_btn, reconnect_btn: a.tb.reconnect_btn,
        split_vert_btn: a.tb.split_vert_btn, split_horiz_btn: a.tb.split_horiz_btn,
        next_id: 1, active_id: 1, sessions: Vec::new(), tab_store: None, stack: a.stack,
        tab_strip: a.tb.tab_strip, mi_btn: a.tb.mi_btn, mi_pop: a.tb.mi_pop,
        mi_pop_label: a.tb.mi_pop_label, mi_refactor_btn: a.tb.mi_refactor_btn,
        tokens_menu: a.tb.tokens_menu, agents_pop: a.tb.agents_pop, box_pop: a.box_pop.box_pop,
        box_gpu_label: a.box_pop.box_gpu_label, box_tmux_pop_box: a.box_pop.box_tmux_pop_box,
        settings_pop: a.settings_pop.settings_pop, ssh_count_label: a.settings_pop.ssh_count_label,
        git_menu, handoff, footer_status: a.footer_status, footer_hold_until: Cell::new(None),
        ssh_hosts: a.ssh_hosts, ssh_toggle_btns: a.settings_pop.ssh_toggle_btns,
        ssh_rows: a.settings_pop.ssh_rows, box_ssh_rows: a.box_pop.box_ssh_rows,
        sidebar_filter: String::new(), box_filter: String::new(),
        enabled_hosts_shared: a.sched.enabled_hosts, tmux_jump_btns: Vec::new(),
        last_tmux_keys: Vec::new(), mi_scheduler: a.sched.mi, git_scheduler: a.sched.git,
        remote_telemetry: a.sched.remote, box_state: a.sched.box_state,
        palette, search_bar: a.search_bar, pin_pop,
    }
}

fn assemble_and_install(config: WorkspaceConfig, initial_theme: theme_mode::ThemeMode) {
    let (win, root_vbox) = create_main_window();
    let tb = topbar::build_topbar_widgets(root_vbox, initial_theme);
    let ssh_hosts = load_ssh_hosts(&config.watch, &config.startup);
    let settings_pop = ssh_ui::build_settings_popover(tb.settings_btn, &ssh_hosts);
    let box_pop = ssh_ui::build_box_popover(tb.box_btn, &config, &ssh_hosts);
    let (sidebar, search_bar, stack) = build_workspace_area(root_vbox);
    let footer_status = build_footer_bar(root_vbox);
    let sched = make_schedulers(&config, &ssh_hosts);
    spawn_background_workers(&sched);
    install_app_state(AppAssembly {
        config, win, initial_theme, tb, settings_pop, box_pop, sidebar,
        search_bar, stack, footer_status, ssh_hosts, sched,
    });
    finish_startup(win);
}

fn finish_startup(win: *mut GtkWidget) {
    session_ops::restore_tabs();
    unsafe {
        gtk_widget_show_all(win);
        g_timeout_add(900, session_ops::radar_tick_cb(), ptr::null_mut());
        gtk_main();
    }
}

fn run_app() {
    init_app_hooks();
    let config = WorkspaceConfig::load();
    let initial_theme = theme_mode::load_initial_theme(config.theme);
    init_gtk_theme(initial_theme);
    assemble_and_install(config, initial_theme);
}
