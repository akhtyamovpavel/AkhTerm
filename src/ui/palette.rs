//! Keyboard command palette for tabs, remote tmux sessions, profiles, SSH, agents, and handoffs.

use std::cell::RefCell;
use std::ffi::{c_int, c_uint, c_void, CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;

use crate::config::LocalProfile;
use crate::ffi::*;
use crate::git_status::{GitChangedFile, GitRepoStatus, MAX_GIT_FILES};
use crate::i18n::{current_locale, UiLocale, UiText};
use crate::paths::{shorten_home, state_dir};
use crate::proc_radar::{RemoteTmuxItem, SshHostEntry};
use crate::session::TermSession;
use crate::theme::current_theme_mode;
use crate::theme_mode::ThemeMode;
use crate::ui_text as text;

const MAX_VISIBLE_ROWS: usize = 48;
const MAX_HANDOFF_FILES: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum PaletteKind {
    Tab = 0,
    RemoteTmux = 1,
    Profile = 2,
    SshPlain = 3,
    SshTmux = 4,
    Agent = 5,
    Handoff = 6,
    Theme = 7,
    Search = 8,
    Git = 9,
    Dispatch = 10,
    Swarm = 11,
    Settings = 12,
    About = 13,
    Locale = 14,
}

const KIND_BADGES: [UiText; 15] = [
    UiText::new(text::PALETTE_BADGE_TAB, c"Tab"),
    UiText::new(text::PALETTE_BADGE_TMUX, c"tmux"),
    UiText::new(text::PALETTE_BADGE_PROFILE, c"Profile"),
    UiText::new(text::PALETTE_BADGE_SSH, c"SSH"),
    UiText::new(text::PALETTE_BADGE_SSH_TMUX, c"SSH · tmux"),
    UiText::new(text::PALETTE_BADGE_AGENT, c"Agent"),
    UiText::new(text::PALETTE_BADGE_HANDOFF, c"Handoff"),
    UiText::new(text::PALETTE_BADGE_THEME, c"Theme"),
    UiText::new(text::PALETTE_BADGE_SEARCH, c"Search"),
    UiText::new(text::PALETTE_BADGE_GIT, c"Git"),
    UiText::new(c"Задача", c"Dispatch"),
    UiText::new(c"Рой", c"Swarm"),
    UiText::new(c"Настройки", c"Settings"),
    UiText::new(c"Справка", c"About"),
    UiText::new(c"Язык", c"Language"),
];

const KIND_CLASSES: [&str; 15] = [
    "palette-badge-tab", "palette-badge-tmux", "palette-badge", "palette-badge-ssh",
    "palette-badge-ssh", "palette-badge-agent", "palette-badge-handoff", "palette-badge",
    "palette-badge", "palette-badge-git", "palette-badge-agent", "palette-badge-tmux",
    "palette-badge", "palette-badge", "palette-badge-tab",
];

impl PaletteKind {
    pub fn badge_text(self) -> UiText {
        KIND_BADGES[self as usize]
    }

    pub fn badge(self) -> &'static CStr {
        self.badge_text().cstr()
    }

    pub fn badge_class(self) -> &'static str {
        KIND_CLASSES[self as usize]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    SelectTab(usize),
    JumpTmux { host: String, session: String },
    OpenProfile(String),
    OpenSsh { host: String, tmux_session: Option<String> },
    RunAgent(usize),
    InjectHandoff(PathBuf),
    ToggleTheme,
    OpenSearch,
    OpenGitMenu,
    GitFile { repo_root: String, path: String, untracked: bool },
    RemoveCleanWorktree,
    OpenTaskDispatch,
    ToggleSwarmDashboard,
    OpenSettings,
    OpenAbout,
    SwitchLocale,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteItem {
    pub kind: PaletteKind,
    pub title: String,
    pub detail: String,
    pub keywords: String,
    pub action: PaletteAction,
}

fn new_item(
    kind: PaletteKind,
    title: String,
    detail: String,
    keywords: String,
    action: PaletteAction,
) -> PaletteItem {
    PaletteItem { kind, title, detail, keywords, action }
}

pub struct PaletteSource<'a> {
    pub sessions: &'a [TermSession],
    pub tmux_items: &'a [RemoteTmuxItem],
    pub profiles: &'a [LocalProfile],
    pub ssh_hosts: &'a [SshHostEntry],
    pub briefs_dir: Option<&'a Path>,
    pub extra_handoffs: &'a [PathBuf],
    pub active_git: Option<&'a GitRepoStatus>,
    pub active_is_worktree: bool,
}

fn is_ctrl_shift_p(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    let shift = (state & GDK_SHIFT_MASK) != 0;
    let is_p = matches!(keyval, 0x50 | 0x70 | 0x6da | 0x6fa) || keycode == 33;
    shift && is_p
}

fn is_plain_ctrl_k(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    let no_shift = (state & GDK_SHIFT_MASK) == 0;
    let is_k = matches!(keyval, 0x4b | 0x6b | 0x6cc | 0x6ec) || keycode == 45;
    no_shift && is_k
}

pub fn is_palette_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if (state & GDK_CONTROL_MASK) == 0 || (state & GDK_MOD1_MASK) != 0 {
        return false;
    }
    is_ctrl_shift_p(keyval, state, keycode) || is_plain_ctrl_k(keyval, state, keycode)
}

#[cfg(test)]
fn handoff_prompt(path: &Path) -> String {
    format!("Прочитай файл {} и действуй по нему.", path.display())
}

fn collect_md_in_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().is_some_and(|ext| ext == "md") && !out.contains(&path) {
            out.push(path);
        }
    }
}

pub fn discover_handoff_files(briefs_dir: Option<&Path>, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(dir) = briefs_dir {
        collect_md_in_dir(dir, &mut files);
    }
    collect_md_in_dir(&state_dir().join("captures"), &mut files);
    for path in extra {
        if !files.contains(path) {
            files.push(path.clone());
        }
    }
    files.sort_by(|a, b| b.cmp(a));
    files.truncate(MAX_HANDOFF_FILES);
    files
}

fn session_host(s: &TermSession) -> &str {
    s.remote().map_or(s.proc_info.ssh_host.as_str(), |r| r.host.as_str())
}

fn session_cwd(s: &TermSession) -> &str {
    if s.proc_info.cwd.is_empty() {
        s.cwd.as_str()
    } else {
        s.proc_info.cwd.as_str()
    }
}

fn session_location_label(s: &TermSession) -> String {
    if let Some((sess, win, _, path)) = &s.remote_tmux {
        format!("tmux:{sess}/{win} · {}", shorten_home(path))
    } else if let Some(cwd) = s.active_local_cwd() {
        shorten_home(&cwd)
    } else {
        shorten_home(session_cwd(s))
    }
}

fn format_session_detail(idx: usize, s: &TermSession) -> String {
    let mut parts = Vec::new();
    let host = session_host(s);
    if !host.is_empty() {
        parts.push(format!("ssh {host}"));
    }
    parts.push(session_location_label(s));
    if !s.proc_info.agent.is_empty() {
        parts.push(s.proc_info.agent.clone());
    }
    if idx < 9 {
        parts.push(format!("Alt+{}", idx + 1));
    }
    parts.join(" · ")
}

fn push_session_items(sessions: &[TermSession], out: &mut Vec<PaletteItem>) {
    for (idx, s) in sessions.iter().enumerate() {
        let detail = format_session_detail(idx, s);
        let host = session_host(s);
        let live_cwd = s.active_local_cwd().unwrap_or_default();
        let kw = format!(
            "вкладка открытая tab open {} {} {} {} {}",
            s.cwd, s.proc_info.cwd, live_cwd, host, s.proc_info.agent
        );
        let act = PaletteAction::SelectTab(s.id);
        out.push(new_item(PaletteKind::Tab, s.title.clone(), detail, kw, act));
    }
}

fn push_tmux_items(items: &[RemoteTmuxItem], out: &mut Vec<PaletteItem>) {
    for item in items {
        let title = format!("{} · tmux:{}", item.host, item.session_name);
        let kw = format!(
            "tmux сессия удаленная удалённая сервер remote session {} {}",
            item.host, item.session_name
        );
        let act = PaletteAction::JumpTmux {
            host: item.host.clone(),
            session: item.session_name.clone(),
        };
        out.push(new_item(PaletteKind::RemoteTmux, title, item.display_line.clone(), kw, act));
    }
}

fn push_profile_items(profiles: &[LocalProfile], out: &mut Vec<PaletteItem>) {
    for p in profiles {
        let title = format!("{} {}", p.icon, p.title);
        let detail = format!("{} · {}", p.key, shorten_home(&p.directory));
        let kw = format!("профиль локальный новая вкладка local profile {} {}", p.key, p.directory);
        let act = PaletteAction::OpenProfile(p.key.clone());
        out.push(new_item(PaletteKind::Profile, title, detail, kw, act));
    }
}

fn format_ssh_endpoint(h: &SshHostEntry) -> String {
    match (h.user.is_empty(), h.hostname.is_empty()) {
        (false, false) => format!("{}@{}", h.user, h.hostname),
        (true, false) => h.hostname.clone(),
        _ => h.alias.clone(),
    }
}

fn make_ssh_plain_item(h: &SshHostEntry, ep: &str, kw: &str) -> PaletteItem {
    let act = PaletteAction::OpenSsh { host: h.alias.clone(), tmux_session: None };
    new_item(PaletteKind::SshPlain, format!("ssh {}", h.alias), ep.to_owned(), kw.to_owned(), act)
}

fn make_ssh_tmux_item(h: &SshHostEntry, ep: &str, kw: &str) -> PaletteItem {
    let act = PaletteAction::OpenSsh {
        host: h.alias.clone(),
        tmux_session: Some("main".to_owned()),
    };
    let title = format!("{} · tmux:main", h.alias);
    let detail = format!("tmux:main · {ep}");
    new_item(PaletteKind::SshTmux, title, detail, format!("tmux:main tmux сессия {kw}"), act)
}

fn push_ssh_items(hosts: &[SshHostEntry], out: &mut Vec<PaletteItem>) {
    for h in hosts {
        let ep = format_ssh_endpoint(h);
        let kw = format!("ssh сервер хост подключение remote host {} {} {}", h.alias, h.hostname, h.user);
        out.push(make_ssh_plain_item(h, &ep, &kw));
        out.push(make_ssh_tmux_item(h, &ep, &kw));
    }
}

fn push_agent_items(out: &mut Vec<PaletteItem>) {
    for (idx, (label, cmd)) in text::AGENT_ACTIONS.iter().enumerate() {
        let title = label.to_string_lossy().into_owned();
        let trimmed = cmd.trim();
        let detail = format!("Команда: {trimmed}");
        let kw = format!("агент запуск ии agent cli {trimmed}");
        out.push(new_item(PaletteKind::Agent, title, detail, kw, PaletteAction::RunAgent(idx)));
    }
}

fn push_handoff_items(files: &[PathBuf], out: &mut Vec<PaletteItem>) {
    for path in files {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let display = path.to_string_lossy();
        let detail = shorten_home(&display);
        let kw = format!("handoff бриф контекст передать файл промпт md {display}");
        let act = PaletteAction::InjectHandoff(path.clone());
        out.push(new_item(PaletteKind::Handoff, name.to_owned(), detail, kw, act));
    }
}

pub fn make_theme_palette_item(mode: ThemeMode) -> PaletteItem {
    let kw = "тема светлая тёмная темная переключить вид цвет theme light dark zinc ctrl+shift+l";
    new_item(
        PaletteKind::Theme,
        mode.palette_title().to_owned(),
        mode.palette_detail().to_owned(),
        kw.to_owned(),
        PaletteAction::ToggleTheme,
    )
}

fn push_theme_item(out: &mut Vec<PaletteItem>) {
    out.push(make_theme_palette_item(current_theme_mode()));
}

fn push_search_item(out: &mut Vec<PaletteItem>) {
    let title = text::PALETTE_SEARCH_TITLE.to_string_lossy().into_owned();
    let detail = text::PALETTE_SEARCH_DETAIL.to_string_lossy().into_owned();
    let kw = "поиск найти выводу скроллбэк лог search find regex ctrl+shift+f".to_owned();
    out.push(new_item(PaletteKind::Search, title, detail, kw, PaletteAction::OpenSearch));
}

fn make_git_menu_item(active_git: Option<&GitRepoStatus>) -> PaletteItem {
    let badge = active_git.map_or_else(String::new, GitRepoStatus::dirty_badge);
    let detail = if badge.is_empty() {
        "Ctrl+Shift+G · статус репозитория и diff".to_owned()
    } else {
        format!("Ctrl+Shift+G · {badge}")
    };
    let kw = "git diff статус изменения файлы репозиторий ctrl+shift+g".to_owned();
    new_item(PaletteKind::Git, "Git · Изменённые файлы".to_owned(), detail, kw, PaletteAction::OpenGitMenu)
}

fn make_git_file_item(root: &str, file: &GitChangedFile) -> PaletteItem {
    let untracked = file.kind.is_untracked();
    let mode = if untracked { "открыть файл" } else { "git diff в сплите" };
    let title = format!("[{}] {}", file.kind.tag(), file.path);
    let detail = format!("{} · {mode}", shorten_home(root));
    let kw = format!("git diff статус файл изменённый {} {}", file.path, root);
    let act = PaletteAction::GitFile {
        repo_root: root.to_owned(),
        path: file.path.clone(),
        untracked,
    };
    new_item(PaletteKind::Git, title, detail, kw, act)
}

fn make_git_wt_item() -> PaletteItem {
    let kw = "git worktree удалить очистить чистый ветка remove clean".to_owned();
    let detail = "Проверить отсутствие изменений и удалить worktree".to_owned();
    new_item(PaletteKind::Git, "Удалить чистый worktree".to_owned(), detail, kw, PaletteAction::RemoveCleanWorktree)
}

fn push_git_items(src: &PaletteSource<'_>, out: &mut Vec<PaletteItem>) {
    out.push(make_git_menu_item(src.active_git));
    if src.active_is_worktree {
        out.push(make_git_wt_item());
    }
    if let Some(st) = src.active_git {
        for file in st.files.iter().take(MAX_GIT_FILES) {
            out.push(make_git_file_item(&st.root, file));
        }
    }
}

fn ui_str(text: UiText, locale: UiLocale) -> String {
    text.for_locale(locale).to_string_lossy().into_owned()
}

fn new_ui_item(
    kind: PaletteKind,
    title: UiText,
    detail: UiText,
    kw: &str,
    action: PaletteAction,
    locale: UiLocale,
) -> PaletteItem {
    new_item(kind, ui_str(title, locale), ui_str(detail, locale), kw.to_owned(), action)
}

fn make_dispatch_item(locale: UiLocale) -> PaletteItem {
    let title = UiText::new(c"Диспетчер задач агентов (Ctrl+Shift+N)", c"Task Dispatch (Ctrl+Shift+N)");
    let detail = UiText::new(
        c"Запустить задачу по брифу или Issue в одном или нескольких worktree",
        c"Launch a brief or Issue across one or more isolated worktrees",
    );
    let kw = "task dispatch задача worktree бриф issue ctrl+shift+n агент рой";
    new_ui_item(PaletteKind::Dispatch, title, detail, kw, PaletteAction::OpenTaskDispatch, locale)
}

fn make_swarm_item(locale: UiLocale) -> PaletteItem {
    let title = UiText::new(c"Панель роя (Ctrl+Shift+S)", c"Swarm Dashboard (Ctrl+Shift+S)");
    let detail = UiText::new(
        c"Карточки активных агентов, быстрый апрув и рассылка промптов",
        c"Live agent cards, quick approve, and multi-pane prompt broadcast",
    );
    let kw = "swarm dashboard рой панели карточки апрув broadcast ctrl+shift+s";
    new_ui_item(PaletteKind::Swarm, title, detail, kw, PaletteAction::ToggleSwarmDashboard, locale)
}

fn make_settings_item(locale: UiLocale) -> PaletteItem {
    let title = UiText::new(c"Настройки (Ctrl+,)", c"Settings (Ctrl+,)");
    let detail = UiText::new(
        c"Тема, язык RU/EN, шрифт, прокрутка и перехват OSC 52",
        c"Theme, RU/EN language, font, scrollback, and OSC 52 settings",
    );
    let kw = "settings настройки конфиг шрифт язык тема osc52 ctrl+,";
    new_ui_item(PaletteKind::Settings, title, detail, kw, PaletteAction::OpenSettings, locale)
}

fn make_about_item(locale: UiLocale) -> PaletteItem {
    let title = UiText::new(c"О программе AkhTerm", c"About AkhTerm");
    let detail = UiText::new(
        c"Версия, git SHA сборки, GTK/VTE и пути конфигурации",
        c"Version, build git SHA, GTK/VTE info, and config paths",
    );
    let kw = "about akhterm о программе версия сборка build info sha";
    new_ui_item(PaletteKind::About, title, detail, kw, PaletteAction::OpenAbout, locale)
}

fn make_locale_item(locale: UiLocale) -> PaletteItem {
    let title = UiText::new(c"Переключить язык (RU / EN)", c"Switch Language (RU / EN)");
    let detail = UiText::new(
        c"Текущий язык: Русский · переключить интерфейс на English",
        c"Current language: English · switch interface to Русский",
    );
    let kw = "language язык locale ru en русский english switch переключить";
    new_ui_item(PaletteKind::Locale, title, detail, kw, PaletteAction::SwitchLocale, locale)
}

fn push_v1_command_items(out: &mut Vec<PaletteItem>, locale: UiLocale) {
    out.push(make_dispatch_item(locale));
    out.push(make_swarm_item(locale));
    out.push(make_settings_item(locale));
    out.push(make_about_item(locale));
    out.push(make_locale_item(locale));
}

pub fn collect_items_for_locale(src: &PaletteSource<'_>, locale: UiLocale) -> Vec<PaletteItem> {
    let mut out = Vec::new();
    push_session_items(src.sessions, &mut out);
    push_v1_command_items(&mut out, locale);
    push_search_item(&mut out);
    push_tmux_items(src.tmux_items, &mut out);
    push_profile_items(src.profiles, &mut out);
    push_ssh_items(src.ssh_hosts, &mut out);
    push_agent_items(&mut out);
    push_theme_item(&mut out);
    push_git_items(src, &mut out);
    let handoffs = discover_handoff_files(src.briefs_dir, src.extra_handoffs);
    push_handoff_items(&handoffs, &mut out);
    out
}

pub fn collect_items(src: &PaletteSource<'_>) -> Vec<PaletteItem> {
    collect_items_for_locale(src, current_locale())
}

fn word_prefix_match(text: &str, tok: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(tok))
}

fn single_token_score(tok: &str, title: &str, badge: &str, detail: &str, kw: &str) -> Option<u32> {
    if word_prefix_match(title, tok) {
        Some(85)
    } else if title.contains(tok) {
        Some(55)
    } else if badge.contains(tok) {
        Some(35)
    } else if detail.contains(tok) {
        Some(22)
    } else if kw.contains(tok) {
        Some(10)
    } else {
        None
    }
}

fn title_phrase_bonus(title: &str, full: &str) -> u32 {
    if title == full {
        300
    } else if title.starts_with(full) {
        180
    } else if title.contains(full) {
        90
    } else {
        0
    }
}

pub fn score_query(item: &PaletteItem, query: &str) -> Option<u32> {
    let full = query.trim().to_lowercase();
    if full.is_empty() {
        return Some(100);
    }
    let title = item.title.to_lowercase();
    let badge = item.kind.badge().to_string_lossy().to_lowercase();
    let detail = item.detail.to_lowercase();
    let kw = item.keywords.to_lowercase();
    let mut total = 100 + title_phrase_bonus(&title, &full);
    for tok in full.split_whitespace() {
        total += single_token_score(tok, &title, &badge, &detail, &kw)?;
    }
    Some(total)
}

pub fn filter_items(items: &[PaletteItem], query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, u32)> = items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| score_query(item, query).map(|sc| (idx, sc)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().map(|(idx, _)| idx).collect()
}

struct PaletteState {
    items: Vec<PaletteItem>,
    filtered: Vec<usize>,
    rows: Vec<*mut GtkWidget>,
    selected: usize,
    open: bool,
    suppress_cancel: bool,
}

fn new_palette_state() -> PaletteState {
    PaletteState {
        items: Vec::new(),
        filtered: Vec::new(),
        rows: Vec::new(),
        selected: 0,
        open: false,
        suppress_cancel: false,
    }
}

struct PaletteCtx {
    popover: *mut GtkWidget,
    entry: *mut GtkWidget,
    scroll: *mut GtkWidget,
    list_box: *mut GtkWidget,
    empty_label: *mut GtkWidget,
    state: RefCell<PaletteState>,
    on_execute: Box<dyn Fn(PaletteAction)>,
    on_cancel: Box<dyn Fn()>,
}

#[derive(Clone)]
pub struct CommandPalette {
    ctx: Rc<PaletteCtx>,
}

fn make_styled_label(text_c: &CStr, cls: &str) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(text_c.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, cls);
        lbl
    }
}

fn build_header_row() -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        add_class(row, "palette-header");
        let title = make_styled_label(text::PALETTE_TITLE, "palette-title");
        let hint = make_styled_label(text::PALETTE_HINT, "palette-hint");
        gtk_box_pack_start(row, title, 1, 1, 0);
        gtk_box_pack_end(row, hint, 0, 0, 0);
        row
    }
}

fn build_scroll_area() -> (*mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, 520);
        gtk_scrolled_window_set_min_content_height(scroll, 220);
        gtk_scrolled_window_set_max_content_height(scroll, 360);
        let list_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 3);
        add_class(list_box, "palette-list");
        gtk_container_add(scroll, list_box);
        (scroll, list_box)
    }
}

fn build_row_labels(item: &PaletteItem) -> *mut GtkWidget {
    unsafe {
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
        let t_lbl = make_styled_label(c"", "palette-item-title");
        gtk_label_set_ellipsize(t_lbl, PANGO_ELLIPSIZE_END);
        gtk_label_set_max_width_chars(t_lbl, 52);
        set_label_text(t_lbl, &item.title);
        let d_lbl = make_styled_label(c"", "palette-item-detail");
        gtk_label_set_ellipsize(d_lbl, PANGO_ELLIPSIZE_MIDDLE);
        gtk_label_set_max_width_chars(d_lbl, 58);
        set_label_text(d_lbl, &item.detail);
        gtk_box_pack_start(vbox, t_lbl, 0, 1, 0);
        gtk_box_pack_start(vbox, d_lbl, 0, 1, 0);
        vbox
    }
}

fn build_row_button(item: &PaletteItem, active: bool, ctx_ptr: *mut c_void) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new();
        gtk_widget_set_can_focus(btn, 0);
        add_class(btn, "palette-row");
        set_class(btn, "palette-row-active", active);
        let hbox = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
        let badge = make_styled_label(item.kind.badge(), "palette-badge");
        add_class(badge, item.kind.badge_class());
        gtk_widget_set_valign(badge, GTK_ALIGN_CENTER);
        gtk_box_pack_start(hbox, build_row_labels(item), 1, 1, 0);
        gtk_box_pack_end(hbox, badge, 0, 0, 0);
        gtk_container_add(btn, hbox);
        set_tooltip(btn, &format!("{}\n{}", item.title, item.detail));
        g_signal_connect_data(btn, c"clicked".as_ptr(), on_row_clicked as *const c_void, ctx_ptr, ptr::null(), 0);
        gtk_widget_show_all(btn);
        btn
    }
}

fn reveal_selected_row(ctx: &PaletteCtx, row: *mut GtkWidget) {
    let mut y: c_int = 0;
    unsafe {
        if gtk_widget_translate_coordinates(row, ctx.list_box, 0, 0, ptr::null_mut(), &mut y) != 0 {
            let h = gtk_widget_get_allocated_height(row).max(32);
            let vadj = gtk_scrolled_window_get_vadjustment(ctx.scroll);
            if !vadj.is_null() {
                gtk_adjustment_clamp_page(vadj, f64::from(y), f64::from(y + h));
            }
        }
    }
}

fn take_filtered_snapshot(ctx: &PaletteCtx, query: &str) -> (Vec<*mut GtkWidget>, Vec<PaletteItem>) {
    let mut st = ctx.state.borrow_mut();
    st.filtered = filter_items(&st.items, query);
    st.filtered.truncate(MAX_VISIBLE_ROWS);
    st.selected = 0;
    let vis = st.filtered.iter().map(|&i| st.items[i].clone()).collect();
    (std::mem::take(&mut st.rows), vis)
}

fn refresh_rows(ctx: &PaletteCtx, query: &str) {
    let (old_rows, visible_items) = take_filtered_snapshot(ctx, query);
    for w in old_rows {
        unsafe { gtk_widget_destroy(w) };
    }
    let ctx_ptr = (ctx as *const PaletteCtx) as *mut c_void;
    let mut new_rows = Vec::with_capacity(visible_items.len());
    for (pos, item) in visible_items.iter().enumerate() {
        let btn = build_row_button(item, pos == 0, ctx_ptr);
        unsafe { gtk_box_pack_start(ctx.list_box, btn, 0, 1, 0) };
        new_rows.push(btn);
    }
    let is_empty = new_rows.is_empty();
    unsafe {
        gtk_widget_set_visible(ctx.scroll, i32::from(!is_empty));
        gtk_widget_set_visible(ctx.empty_label, i32::from(is_empty));
    }
    ctx.state.borrow_mut().rows = new_rows;
}

fn move_selection(ctx: &PaletteCtx, delta: isize) {
    let pair = {
        let mut st = ctx.state.borrow_mut();
        if st.rows.is_empty() {
            return;
        }
        let old = st.selected;
        let next = (old as isize + delta).rem_euclid(st.rows.len() as isize) as usize;
        st.selected = next;
        (st.rows[old], st.rows[next])
    };
    set_class(pair.0, "palette-row-active", false);
    set_class(pair.1, "palette-row-active", true);
    reveal_selected_row(ctx, pair.1);
}

fn execute_at(ctx: &PaletteCtx, row_pos: usize) {
    let action = {
        let mut st = ctx.state.borrow_mut();
        let Some(&item_idx) = st.filtered.get(row_pos) else {
            return;
        };
        st.open = false;
        st.suppress_cancel = true;
        st.items[item_idx].action.clone()
    };
    unsafe { gtk_popover_popdown(ctx.popover) };
    (ctx.on_execute)(action);
}

fn cancel_and_close(ctx: &PaletteCtx) {
    let was_open = {
        let mut st = ctx.state.borrow_mut();
        let prev = st.open;
        st.open = false;
        st.suppress_cancel = true;
        prev
    };
    unsafe { gtk_popover_popdown(ctx.popover) };
    if was_open {
        (ctx.on_cancel)();
    }
}

fn handle_palette_key(ctx: &PaletteCtx, keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    if is_palette_shortcut(keyval, state, keycode) {
        cancel_and_close(ctx);
        return true;
    }
    match keyval {
        0xff54 | 0xff99 => move_selection(ctx, 1),
        0xff52 | 0xff97 => move_selection(ctx, -1),
        0xff0d | 0xff8d | 0xfe34 => {
            let sel = ctx.state.borrow().selected;
            execute_at(ctx, sel);
        }
        0xff1b => cancel_and_close(ctx),
        _ => return false,
    }
    true
}

fn read_entry_query(entry: *mut GtkWidget) -> String {
    unsafe {
        let ptr = gtk_entry_get_text(entry);
        if ptr.is_null() {
            String::new()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}

extern "C" fn on_entry_changed(entry: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const PaletteCtx) };
    refresh_rows(ctx, &read_entry_query(entry));
}

extern "C" fn on_entry_key_press(
    _entry: *mut GtkWidget,
    event: *const GdkEvent,
    data: *mut c_void,
) -> c_int {
    let mut keyval: c_uint = 0;
    let mut state: c_uint = 0;
    let mut keycode: u16 = 0;
    unsafe {
        if gdk_event_get_keyval(event, &mut keyval) == 0 {
            return 0;
        }
        gdk_event_get_state(event, &mut state);
        gdk_event_get_keycode(event, &mut keycode);
    }
    let ctx = unsafe { &*(data as *const PaletteCtx) };
    i32::from(handle_palette_key(ctx, keyval, state, keycode))
}

extern "C" fn on_row_clicked(btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const PaletteCtx) };
    let pos = ctx.state.borrow().rows.iter().position(|&w| w == btn);
    if let Some(pos) = pos {
        execute_at(ctx, pos);
    }
}

extern "C" fn on_popover_closed(_pop: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const PaletteCtx) };
    let should_restore = {
        let mut st = ctx.state.borrow_mut();
        let restore = st.open && !st.suppress_cancel;
        st.open = false;
        st.suppress_cancel = false;
        restore
    };
    if should_restore {
        (ctx.on_cancel)();
    }
}

fn wire_palette_signals(ctx: &Rc<PaletteCtx>) {
    let ptr = Rc::as_ptr(ctx) as *mut c_void;
    unsafe {
        g_signal_connect_data(ctx.entry, c"changed".as_ptr(), on_entry_changed as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(ctx.entry, c"key-press-event".as_ptr(), on_entry_key_press as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(ctx.popover, c"closed".as_ptr(), on_popover_closed as *const c_void, ptr, ptr::null(), 0);
    }
}

fn build_search_input() -> *mut GtkWidget {
    unsafe {
        let entry = gtk_search_entry_new();
        add_class(entry, "palette-entry");
        gtk_entry_set_placeholder_text(entry, text::PALETTE_PLACEHOLDER.as_ptr());
        entry
    }
}

fn build_popover_shell(
    anchor: *mut GtkWidget,
) -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let popover = gtk_popover_new(anchor);
        add_class(popover, "palette-popover");
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
        add_class(vbox, "palette-box");
        let entry = build_search_input();
        let (scroll, list_box) = build_scroll_area();
        let empty = make_styled_label(text::PALETTE_EMPTY, "palette-empty");
        gtk_widget_set_no_show_all(empty, 1);
        gtk_box_pack_start(vbox, build_header_row(), 0, 1, 0);
        gtk_box_pack_start(vbox, entry, 0, 1, 0);
        gtk_box_pack_start(vbox, scroll, 1, 1, 0);
        gtk_box_pack_start(vbox, empty, 0, 1, 0);
        gtk_container_add(popover, vbox);
        gtk_widget_show_all(vbox);
        (popover, entry, scroll, list_box, empty)
    }
}

impl CommandPalette {
    pub fn new(
        anchor: *mut GtkWidget,
        on_execute: impl Fn(PaletteAction) + 'static,
        on_cancel: impl Fn() + 'static,
    ) -> Self {
        let (popover, entry, scroll, list_box, empty_label) = build_popover_shell(anchor);
        let ctx = Rc::new(PaletteCtx {
            popover,
            entry,
            scroll,
            list_box,
            empty_label,
            state: RefCell::new(new_palette_state()),
            on_execute: Box::new(on_execute),
            on_cancel: Box::new(on_cancel),
        });
        wire_palette_signals(&ctx);
        Self { ctx }
    }

    pub fn is_open(&self) -> bool {
        self.ctx.state.borrow().open
    }

    pub fn open(&self, items: Vec<PaletteItem>) {
        {
            let mut st = self.ctx.state.borrow_mut();
            st.items = items;
            st.open = true;
            st.suppress_cancel = false;
        }
        if read_entry_query(self.ctx.entry).is_empty() {
            refresh_rows(&self.ctx, "");
        } else {
            unsafe { gtk_entry_set_text(self.ctx.entry, c"".as_ptr()) };
        }
        unsafe {
            gtk_popover_popup(self.ctx.popover);
            gtk_widget_grab_focus(self.ctx.entry);
        }
    }

    pub fn close(&self) {
        cancel_and_close(&self.ctx);
    }

    #[allow(dead_code)]
    pub fn handle_key(&self, keyval: c_uint, state: c_uint, keycode: u16) -> bool {
        handle_palette_key(&self.ctx, keyval, state, keycode)
    }

    #[allow(dead_code)]
    pub fn set_search_text(&self, query: &str) {
        if let Ok(c) = CString::new(query) {
            unsafe { gtk_entry_set_text(self.ctx.entry, c.as_ptr()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    fn test_item(k: PaletteKind, t: &str, d: &str, kw: &str, a: PaletteAction) -> PaletteItem {
        new_item(k, t.to_owned(), d.to_owned(), kw.to_owned(), a)
    }

    fn sample_items() -> Vec<PaletteItem> {
        let tmux_act = PaletteAction::JumpTmux { host: "gpu".into(), session: "train".into() };
        let ssh_act = PaletteAction::OpenSsh { host: "devbox".into(), tmux_session: Some("main".into()) };
        let ho_act = PaletteAction::InjectHandoff(PathBuf::from("/tmp/передача_контекста.md"));
        vec![
            test_item(PaletteKind::Tab, "Курс ЦУ · Claude", "~/Учёба/Курс ЦУ · Alt+1", "вкладка tab open claude", PaletteAction::SelectTab(1)),
            test_item(PaletteKind::RemoteTmux, "gpu · tmux:train", "[gpu] train:0*", "tmux сессия удаленная удалённая gpu train", tmux_act),
            test_item(PaletteKind::Profile, "⌂ Терминал", "shell · ~", "профиль локальный shell", PaletteAction::OpenProfile("shell".into())),
            test_item(PaletteKind::SshTmux, "devbox · tmux:main", "user@10.0.0.5", "tmux:main ssh сервер devbox", ssh_act),
            test_item(PaletteKind::Agent, "agy · Antigravity CLI", "Команда: agy", "агент запуск agent agy", PaletteAction::RunAgent(0)),
            test_item(PaletteKind::Handoff, "передача_контекста.md", "~/briefs/передача_контекста.md", "handoff бриф контекст md", ho_act),
        ]
    }

    #[test]
    fn recognizes_palette_shortcuts_in_latin_cyrillic_and_keycodes() {
        let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
        assert!(is_palette_shortcut(b'P' as u32, cs, 0));
        assert!(is_palette_shortcut(b'p' as u32, cs, 0));
        assert!(is_palette_shortcut(0x6da, cs, 33));
        assert!(is_palette_shortcut(b'k' as u32, GDK_CONTROL_MASK, 0));
        assert!(is_palette_shortcut(0x6cc, GDK_CONTROL_MASK, 45));
        assert!(!is_palette_shortcut(b'K' as u32, cs, 45));
        assert!(!is_palette_shortcut(b'p' as u32, GDK_CONTROL_MASK, 33));
        assert!(!is_palette_shortcut(b'k' as u32, GDK_CONTROL_MASK | GDK_MOD1_MASK, 45));
    }

    #[test]
    fn filters_and_ranks_cyrillic_and_latin_multi_token_queries() {
        let items = sample_items();
        assert_eq!(filter_items(&items, "").len(), items.len());
        assert_eq!(filter_items(&items, "КУРС claude"), vec![0]);
        assert_eq!(filter_items(&items, "удалённая train"), vec![1]);
        assert_eq!(filter_items(&items, "агент agy"), vec![4]);
        assert_eq!(filter_items(&items, "бриф контекста"), vec![5]);
        let ranked = filter_items(&items, "tmux");
        assert_eq!(items[ranked[0]].kind, PaletteKind::RemoteTmux);
        assert!(filter_items(&items, "несуществующий_запрос").is_empty());
    }

    fn sample_source<'a>(
        tmux_items: &'a [RemoteTmuxItem],
        profiles: &'a [LocalProfile],
        hosts: &'a [SshHostEntry],
        handoffs: &'a [PathBuf],
    ) -> PaletteSource<'a> {
        PaletteSource {
            sessions: &[],
            tmux_items,
            profiles,
            ssh_hosts: hosts,
            briefs_dir: None,
            extra_handoffs: handoffs,
            active_git: None,
            active_is_worktree: false,
        }
    }

    fn sample_git_status() -> GitRepoStatus {
        GitRepoStatus {
            root: "/tmp/репо".into(),
            head: "wt/agy-1".into(),
            is_linked_worktree: true,
            changed_count: 1,
            insertions: 5,
            deletions: 1,
            files: vec![GitChangedFile {
                path: "src/git_status.rs".into(),
                kind: crate::git_status::GitFileKind::Modified,
            }],
        }
    }

    #[test]
    fn indexes_git_files_and_clean_worktree_removal_action() {
        let git_st = sample_git_status();
        let mut src = sample_source(&[], &[], &[], &[]);
        src.active_git = Some(&git_st);
        src.active_is_worktree = true;
        let items = collect_items(&src);
        assert!(items.iter().any(|i| i.action == PaletteAction::OpenGitMenu));
        assert!(items.iter().any(|i| i.action == PaletteAction::RemoveCleanWorktree));
        let expected_file = PaletteAction::GitFile {
            repo_root: "/tmp/репо".into(),
            path: "src/git_status.rs".into(),
            untracked: false,
        };
        assert!(items.iter().any(|i| i.action == expected_file));
    }

    #[test]
    fn indexes_tmux_profiles_ssh_modes_agents_and_handoff_files() {
        let tmux = [RemoteTmuxItem {
            host: "gpu1".into(),
            session_name: "exp".into(),
            display_line: "[gpu1] exp:0*".into(),
            is_active: true,
        }];
        let prof = [LocalProfile {
            key: "work".into(),
            title: "Рабочая".into(),
            icon: "◇".into(),
            directory: "/tmp".into(),
            monitor: false,
        }];
        let hosts = [SshHostEntry {
            alias: "prod".into(),
            hostname: "10.1.1.2".into(),
            user: "ubuntu".into(),
            enabled: true,
        }];
        let handoffs = [PathBuf::from("/tmp/plan.md")];
        let all = collect_items(&sample_source(&tmux, &prof, &hosts, &handoffs));
        assert!(all.iter().any(|i| i.action == PaletteAction::JumpTmux { host: "gpu1".into(), session: "exp".into() }));
        assert!(all.iter().any(|i| i.action == PaletteAction::OpenProfile("work".into())));
        assert!(all.iter().any(|i| i.action == PaletteAction::OpenSsh { host: "prod".into(), tmux_session: None }));
        assert!(all.iter().any(|i| i.action == PaletteAction::OpenSsh { host: "prod".into(), tmux_session: Some("main".into()) }));
        assert!(all.iter().any(|i| matches!(i.action, PaletteAction::RunAgent(_))));
        assert!(all.iter().any(|i| i.action == PaletteAction::InjectHandoff(PathBuf::from("/tmp/plan.md"))));
        assert!(all.iter().any(|i| i.action == PaletteAction::OpenSearch && i.title == "Поиск по выводу"));
        let search_hits = filter_items(&all, "Поиск по выводу");
        assert_eq!(all[search_hits[0]].action, PaletteAction::OpenSearch);
        assert_eq!(handoff_prompt(Path::new("/tmp/plan.md")), "Прочитай файл /tmp/plan.md и действуй по нему.");
    }

    #[test]
    fn indexes_theme_toggle_action_in_command_palette() {
        let all = collect_items(&sample_source(&[], &[], &[], &[]));
        assert!(all.iter().any(|i| i.action == PaletteAction::ToggleTheme));
        assert!(score_query(&make_theme_palette_item(ThemeMode::Dark), "светлая тема").is_some());
        assert!(score_query(&make_theme_palette_item(ThemeMode::Light), "dark zinc").is_some());
    }

    #[test]
    fn indexes_v1_actions_with_bilingual_ru_and_en_labels_and_badges() {
        let src = sample_source(&[], &[], &[], &[]);
        let ru = collect_items_for_locale(&src, UiLocale::Ru);
        let en = collect_items_for_locale(&src, UiLocale::En);
        assert_eq!(PaletteKind::Dispatch.badge_text().for_locale(UiLocale::Ru), c"Задача");
        assert_eq!(PaletteKind::Dispatch.badge_text().for_locale(UiLocale::En), c"Dispatch");
        assert_eq!(PaletteKind::Swarm.badge_text().for_locale(UiLocale::Ru), c"Рой");
        assert_eq!(PaletteKind::Swarm.badge_text().for_locale(UiLocale::En), c"Swarm");
        assert!(ru.iter().any(|i| i.action == PaletteAction::OpenTaskDispatch && i.title.contains("Диспетчер")));
        assert!(en.iter().any(|i| i.action == PaletteAction::OpenTaskDispatch && i.title.contains("Task Dispatch")));
        assert!(ru.iter().any(|i| i.action == PaletteAction::ToggleSwarmDashboard && i.title.contains("Панель роя")));
        assert!(en.iter().any(|i| i.action == PaletteAction::ToggleSwarmDashboard && i.title.contains("Swarm Dashboard")));
        assert!(ru.iter().any(|i| i.action == PaletteAction::OpenSettings && i.title.contains("Настройки")));
        assert!(en.iter().any(|i| i.action == PaletteAction::OpenSettings && i.title.contains("Settings")));
        assert!(en.iter().any(|i| i.action == PaletteAction::OpenAbout && i.title.contains("About AkhTerm")));
        assert!(ru.iter().any(|i| i.action == PaletteAction::SwitchLocale && i.title.contains("RU / EN")));
        let hits = filter_items(&en, "swarm broadcast");
        assert_eq!(en[hits[0]].action, PaletteAction::ToggleSwarmDashboard);
    }

    #[test]
    fn palette_module_maintains_grade_b_mi_score() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("palette.rs"));
        assert!(mi >= 58.85, "expected palette.rs MI >= 58.85, got {mi:.2}");
    }

    fn pump_events() {
        for _ in 0..60 {
            if unsafe { gtk_events_pending() } == 0 {
                break;
            }
            unsafe { gtk_main_iteration_do(0) };
        }
    }

    fn spawn_isolated_broadway(dir: &Path) -> Option<Child> {
        let http_sock = dir.join("http.sock");
        let child = Command::new("broadwayd")
            .args(["-u", &http_sock.to_string_lossy(), ":16"])
            .env("XDG_RUNTIME_DIR", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let gdk_sock = dir.join("broadway17.socket");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !gdk_sock.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        std::env::set_var("XDG_RUNTIME_DIR", dir);
        std::env::set_var("GDK_BACKEND", "broadway");
        std::env::set_var("BROADWAY_DISPLAY", ":16");
        (unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } != 0).then_some(child)
    }

    fn create_test_window() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
            let topbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
            let vte = vte_terminal_new();
            gtk_box_pack_start(vbox, topbar, 0, 1, 0);
            gtk_box_pack_start(vbox, vte, 1, 1, 0);
            gtk_container_add(win, vbox);
            gtk_widget_show_all(win);
            gtk_widget_grab_focus(vte);
            (win, topbar, vte)
        }
    }

    fn verify_up_down_and_escape(palette: &CommandPalette, win: *mut GtkWidget, vte: *mut GtkWidget) {
        palette.open(sample_items());
        pump_events();
        assert!(palette.is_open());
        assert_eq!(palette.ctx.state.borrow().rows.len(), 6);
        assert!(palette.handle_key(0xff54, 0, 0));
        assert_eq!(palette.ctx.state.borrow().selected, 1);
        assert!(palette.handle_key(0xff52, 0, 0));
        assert_eq!(palette.ctx.state.borrow().selected, 0);
        assert!(palette.handle_key(0xff1b, 0, 0));
        pump_events();
        assert!(!palette.is_open());
        assert_eq!(unsafe { gtk_window_get_focus(win) }, vte);
    }

    fn verify_query_and_enter(palette: &CommandPalette, executed: &Rc<RefCell<Vec<PaletteAction>>>) {
        palette.open(sample_items());
        palette.set_search_text("КУРС");
        pump_events();
        assert_eq!(palette.ctx.state.borrow().rows.len(), 1);
        assert!(palette.handle_key(0xff0d, 0, 0));
        pump_events();
        assert!(!palette.is_open());
        assert_eq!(*executed.borrow(), vec![PaletteAction::SelectTab(1)]);
    }

    fn verify_reopen_resets_query(palette: &CommandPalette, win: *mut GtkWidget, vte: *mut GtkWidget) {
        palette.open(sample_items());
        pump_events();
        assert_eq!(read_entry_query(palette.ctx.entry), "");
        assert_eq!(palette.ctx.state.borrow().rows.len(), 6);
        assert!(palette.handle_key(b'k' as u32, GDK_CONTROL_MASK, 45));
        pump_events();
        assert!(!palette.is_open());
        assert_eq!(unsafe { gtk_window_get_focus(win) }, vte);
    }

    fn run_broadway_palette_checks() {
        let dir = std::env::temp_dir().join(format!("akhterm_pal_bway_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let Some(mut child) = spawn_isolated_broadway(&dir) else {
            let _ = fs::remove_dir_all(&dir);
            return;
        };
        unsafe { crate::theme::apply_gtk_theme() };
        let (win, topbar, vte) = create_test_window();
        pump_events();
        let executed: Rc<RefCell<Vec<PaletteAction>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&executed);
        let palette = CommandPalette::new(
            topbar,
            move |act| sink.borrow_mut().push(act),
            move || unsafe { gtk_widget_grab_focus(vte) },
        );
        verify_up_down_and_escape(&palette, win, vte);
        verify_query_and_enter(&palette, &executed);
        verify_reopen_resets_query(&palette, win, vte);
        unsafe { gtk_widget_destroy(win) };
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn headless_broadway_palette_widget_and_keyboard_navigation() {
        if std::env::var_os("AKHTERM_PALETTE_BWAY_CHILD").is_some() {
            run_broadway_palette_checks();
            return;
        }
        let exe = std::env::current_exe().expect("test binary path");
        let status = Command::new(exe)
            .args(["--exact", "ui::palette::tests::headless_broadway_palette_widget_and_keyboard_navigation"])
            .env("AKHTERM_PALETTE_BWAY_CHILD", "1")
            .status()
            .expect("spawn broadway palette test");
        assert!(status.success());
    }
}
