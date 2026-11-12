//! Static interface labels and bilingual (`RU` / `EN`) `UiText` registry for AkhTerm.

#![allow(dead_code)]

use std::ffi::CStr;

#[allow(unused_imports)]
pub use crate::i18n::{
    configured_locale, current_locale, detect_system_locale_from_env, set_locale, UiLocale, UiText,
};

pub const APP_NAME: &CStr = c"AkhTerm";
pub const PROFILES: &CStr = c"Профили / SSH ▾";
pub const SERVERS: &CStr = c"Серверы ▾";
pub const AGENTS: &CStr = c"Агенты ▾";
pub const HANDOFF: &CStr = c"Handoff ▾";
pub const MAINTAINABILITY: &CStr = c"MI ▾";
pub const MI_EMPTY: &CStr = c"Нет данных MI для открытой вкладки.\nПерейдите в каталог проекта (.git, Cargo.toml, pyproject.toml, package.json, go.mod, Makefile).";
pub const MI_REFACTOR: &CStr = c"Поручить рефакторинг агенту";
pub const MI_REFACTOR_TIP: &CStr = c"Отправить в активный терминал промпт на рефакторинг файлов с низким MI";
pub const TOGGLE_SIDEBAR: &CStr = c"◧";
pub const TOGGLE_SIDEBAR_TIP: &CStr = c"Показать или скрыть боковую панель";
pub const THEME_DARK_ICON: &CStr = c"☾";
pub const THEME_LIGHT_ICON: &CStr = c"☀";
pub const THEME_TO_LIGHT_TIP: &CStr = c"Переключить на светлую тему (Ctrl+Shift+L)";
pub const THEME_TO_DARK_TIP: &CStr = c"Переключить на тёмную тему (Ctrl+Shift+L)";
pub const SPLIT_VERT: &CStr = c"◫";
pub const SPLIT_VERT_TIP: &CStr = c"Разделить вкладку по вертикали (Ctrl+Shift+D)";
pub const SPLIT_HORIZ: &CStr = c"⬒";
pub const SPLIT_HORIZ_TIP: &CStr = c"Разделить вкладку по горизонтали (Ctrl+Shift+E)";
pub const RECONNECT: &CStr = c"Переподключить";
pub const RECONNECT_TIP: &CStr = c"Переподключить активную вкладку SSH или tmux (Ctrl+Shift+R)";
pub const INTERRUPT: &CStr = c"SIGINT";
pub const INTERRUPT_TIP: &CStr = c"Прервать активный процесс (SIGINT)";
pub const PASTE_IMAGE: &CStr = c"Скриншот";
pub const PASTE_IMAGE_TIP: &CStr = c"Сохранить изображение из буфера в активной сессии и вставить путь";
pub const ZOOM_IN: &CStr = c"+";
pub const ZOOM_IN_TIP: &CStr = c"Увеличить шрифт терминала (Ctrl++)";
pub const ZOOM_RESET: &CStr = c"100%";
pub const ZOOM_RESET_TIP: &CStr = c"Сбросить масштаб терминала (Ctrl+0)";
pub const ZOOM_OUT: &CStr = c"−";
pub const ZOOM_OUT_TIP: &CStr = c"Уменьшить шрифт терминала (Ctrl+−)";
pub const NEW_TAB: &CStr = c"+";
pub const NEW_TAB_TIP: &CStr = c"Новая вкладка (Ctrl+Shift+T)";
pub const CLOSE_TAB: &CStr = c"×";
pub const CLOSE_TAB_TIP: &CStr = c"Закрыть вкладку";
pub const PIN_TAB_TITLE: &CStr = c"Название вкладки";
pub const PIN_TAB_PLACEHOLDER: &CStr = c"Закреплённое имя (пусто — авто)";
pub const PIN_TAB_SAVE: &CStr = c"Закрепить";
pub const PIN_TAB_RESET: &CStr = c"Сбросить";
pub const STATUS_APPROVAL: &CStr = c"Ждёт апрува";
pub const APPROVAL_TAB_TIP: &CStr = c"Ждёт апрува (Ctrl+Shift+U — перейти)";
pub const NOTIFY_APPROVAL_TITLE: &CStr = c"AkhTerm · Ждёт апрува";
pub const NOTIFY_READY_TITLE: &CStr = c"AkhTerm · Готово";
pub const TMUX_MAIN: &CStr = c"tmux:main";
pub const AGENTS_WORKTREE_HEADER: &CStr = c"НОВЫЙ GIT WORKTREE · ОТДЕЛЬНАЯ ВКЛАДКА";
pub const AGENT_WORKTREE_TIP: &CStr =
    c"Создать изолированный Git worktree и запустить агента в новой вкладке";
pub const WORKTREE_LOCAL_ONLY: &CStr =
    c"⚠ Git worktree доступен только для локальной вкладки репозитория";
pub const PALETTE_TITLE: &CStr = c"Палитра команд";
pub const PALETTE_HINT: &CStr = c"↑↓ выбор · Enter выполнить · Esc закрыть";
pub const PALETTE_PLACEHOLDER: &CStr = c"Вкладка, папка, сессия tmux, SSH, агент или Handoff...";
pub const PALETTE_EMPTY: &CStr = c"Ничего не найдено";
pub const PALETTE_BADGE_TAB: &CStr = c"Вкладка";
pub const PALETTE_BADGE_TMUX: &CStr = c"tmux";
pub const PALETTE_BADGE_PROFILE: &CStr = c"Профиль";
pub const PALETTE_BADGE_SSH: &CStr = c"SSH";
pub const PALETTE_BADGE_SSH_TMUX: &CStr = c"SSH · tmux";
pub const PALETTE_BADGE_AGENT: &CStr = c"Агент";
pub const PALETTE_BADGE_HANDOFF: &CStr = c"Handoff";
pub const PALETTE_BADGE_THEME: &CStr = c"Тема";
pub const PALETTE_BADGE_SEARCH: &CStr = c"Поиск";
pub const PALETTE_BADGE_GIT: &CStr = c"Git";
pub const PALETTE_SEARCH_TITLE: &CStr = c"Поиск по выводу";
pub const PALETTE_SEARCH_DETAIL: &CStr =
    c"Найти текст или regex в активном терминале · Ctrl+Shift+F";

pub const SEARCH_PLACEHOLDER: &CStr = c"Поиск по выводу терминала...";
pub const SEARCH_PREV: &CStr = c"↑";
pub const SEARCH_PREV_TIP: &CStr = c"Предыдущее совпадение (Shift+Enter)";
pub const SEARCH_NEXT: &CStr = c"↓";
pub const SEARCH_NEXT_TIP: &CStr = c"Следующее совпадение (Enter)";
pub const SEARCH_CASE: &CStr = c"Aa";
pub const SEARCH_CASE_TIP: &CStr = c"Учитывать регистр";
pub const SEARCH_REGEX: &CStr = c".*";
pub const SEARCH_REGEX_TIP: &CStr = c"Регулярное выражение (PCRE2)";
pub const SEARCH_CLOSE_TIP: &CStr = c"Закрыть поиск (Esc)";
pub const SEARCH_FOUND: &CStr = c"Найдено";
pub const SEARCH_NOT_FOUND: &CStr = c"Нет совпадений";
pub const SEARCH_REGEX_ERR: &CStr = c"Ошибка regex";

pub const GIT_MENU: &CStr = c"Git ▾";
pub const GIT_MENU_TIP: &CStr =
    c"Изменённые файлы Git, быстрый diff и очистка worktree (Ctrl+Shift+G)";
pub const GIT_LOCAL_ONLY: &CStr = c"⚠ Меню Git доступно только для локальной вкладки";
pub const GIT_HEADER: &CStr = c"GIT · ЛОКАЛЬНАЯ ВКЛАДКА";
pub const GIT_NOT_REPO: &CStr = c"Текущий каталог не является Git-репозиторием";
pub const GIT_FILES_HEADER: &CStr = c"ИЗМЕНЁННЫЕ И НОВЫЕ ФАЙЛЫ (КЛИК — DIFF / ОТКРЫТЬ)";
pub const GIT_CLEAN: &CStr = c"Рабочее дерево чистое — незакоммиченных изменений нет";
pub const GIT_LOADING: &CStr = c"Обновление статуса Git в фоне…";
pub const GIT_REMOVE_WT_BTN: &CStr = c"Удалить чистый worktree";
pub const GIT_REMOVE_WT_TIP: &CStr =
    c"Проверить отсутствие изменений и удалить изолированный Git worktree";

pub const AGENT_ACTIONS: [(&CStr, &str); 5] = [
    (c"agy · Antigravity CLI", "agy\n"),
    (c"codex · OpenAI Codex CLI", "codex\n"),
    (c"claude · Claude Code", "claude\n"),
    (c"dsh · DSH Agent", "dsh\n"),
    (c"kimi · Kimi Code CLI", "kimi\n"),
];

pub const HANDOFF_TIP: &CStr = c"Снять лог активной вкладки или передать .md-бриф в сессию";
pub const HANDOFF_CAPTURE_BTN: &CStr = c"Сохранить вывод активной вкладки (.md)";
pub const HANDOFF_CAPTURE_TIP: &CStr = c"Сохранить последние строки активного терминала в Markdown-файл (0600)";
pub const HANDOFF_FORWARD_HEADER: &CStr = c"ПЕРЕДАТЬ ВЫВОД АКТИВНОЙ ВКЛАДКИ В СЕССИЮ";
pub const HANDOFF_BRIEFS_HEADER: &CStr = c"БРИФЫ И СНИМКИ (.MD) — В АКТИВНУЮ ВКЛАДКУ";
pub const HANDOFF_EMPTY: &CStr = c"Нет сохранённых .md-файлов — снимите вывод выше";
pub const DROP_SCP_START: &CStr = c"⬆ Загрузка по SCP на";
pub const DROP_SCP_DONE: &CStr = c"📎 Загружено по SCP на";
pub const DROP_SCP_ERROR: &CStr = c"⚠ Ошибка SCP при загрузке на";
pub const DROP_SCP_NO_FILE: &CStr = c"⚠ Ошибка SCP: локальный файл недоступен";

pub const TOKENS: &CStr = c"Токены ▾";
pub const TOKENS_TIP: &CStr = c"Расход токенов, контекст и таймер хода агентов по вкладкам";
pub const TOKENS_HEADER: &CStr = c"ТОКЕНЫ · КОНТЕКСТ И ДЛИТЕЛЬНОСТЬ ХОДА";
pub const TOKENS_EMPTY: &CStr =
    c"Нет данных о токенах для открытых вкладок.\nЗапустите агента (AGY, Claude Code, Codex CLI) или сессию с индикатором контекста.";

pub const CTX_COPY: &CStr = c"Копировать";
pub const CTX_PASTE: &CStr = c"Вставить";
pub const CTX_OPEN_LINK: &CStr = c"Открыть ссылку";
pub const CTX_COPY_LINK: &CStr = c"Скопировать ссылку";
pub const CTX_OPEN_FOLDER: &CStr = c"Открыть папку";
pub const CTX_SEND_TO_TAB: &CStr = c"Отправить в вкладку ▸";
pub const CTX_SPLIT_RIGHT: &CStr = c"Разделить вправо";
pub const CTX_SPLIT_DOWN: &CStr = c"Разделить вниз";
pub const CTX_CLOSE_PANE: &CStr = c"Закрыть панель";
pub const CTX_CLEAR_BUFFER: &CStr = c"Очистить буфер";

pub const CTX_RENAME_TAB: &CStr = c"Переименовать вкладку…";
pub const CTX_DUPLICATE_TAB: &CStr = c"Дублировать вкладку";
pub const CTX_COPY_PATH: &CStr = c"Скопировать путь";
pub const CTX_CLOSE_TAB: &CStr = c"Закрыть вкладку";
pub const CTX_CLOSE_OTHER_TABS: &CStr = c"Закрыть остальные";
pub const RENAME_PLACEHOLDER: &CStr = c"Имя вкладки (пусто — авто)";
pub const RENAME_SAVE: &CStr = c"Сохранить";

pub mod i18n_text {
    use super::*;

    pub const APP_NAME: UiText = UiText::new(super::APP_NAME, c"AkhTerm");
    pub fn app_name() -> &'static CStr { APP_NAME.cstr() }
    pub const PROFILES: UiText = UiText::new(super::PROFILES, c"Profiles / SSH ▾");
    pub fn profiles() -> &'static CStr { PROFILES.cstr() }
    pub fn profiles_str() -> &'static str { PROFILES.as_str() }
    pub const SERVERS: UiText = UiText::new(super::SERVERS, c"Servers ▾");
    pub fn servers() -> &'static CStr { SERVERS.cstr() }
    pub const AGENTS: UiText = UiText::new(super::AGENTS, c"Agents ▾");
    pub fn agents() -> &'static CStr { AGENTS.cstr() }
    pub const HANDOFF: UiText = UiText::new(super::HANDOFF, c"Handoff ▾");
    pub fn handoff() -> &'static CStr { HANDOFF.cstr() }
    pub const MAINTAINABILITY: UiText = UiText::new(super::MAINTAINABILITY, c"MI ▾");
    pub fn maintainability() -> &'static CStr { MAINTAINABILITY.cstr() }
    pub const MI_EMPTY: UiText = UiText::new(super::MI_EMPTY, c"No MI data for the active tab.\nOpen a project directory (.git, Cargo.toml, pyproject.toml, package.json, go.mod, Makefile).");
    pub fn mi_empty() -> &'static CStr { MI_EMPTY.cstr() }
    pub const MI_REFACTOR: UiText = UiText::new(super::MI_REFACTOR, c"Ask agent to refactor");
    pub fn mi_refactor() -> &'static CStr { MI_REFACTOR.cstr() }
    pub const MI_REFACTOR_TIP: UiText = UiText::new(super::MI_REFACTOR_TIP, c"Send a prompt to the active terminal to refactor low-MI files");
    pub fn mi_refactor_tip() -> &'static CStr { MI_REFACTOR_TIP.cstr() }
    pub const TOGGLE_SIDEBAR: UiText = UiText::new(super::TOGGLE_SIDEBAR, c"◧");
    pub fn toggle_sidebar() -> &'static CStr { TOGGLE_SIDEBAR.cstr() }
    pub const TOGGLE_SIDEBAR_TIP: UiText = UiText::new(super::TOGGLE_SIDEBAR_TIP, c"Show or hide sidebar");
    pub fn toggle_sidebar_tip() -> &'static CStr { TOGGLE_SIDEBAR_TIP.cstr() }
    pub const THEME_DARK_ICON: UiText = UiText::new(super::THEME_DARK_ICON, c"☾");
    pub fn theme_dark_icon() -> &'static CStr { THEME_DARK_ICON.cstr() }
    pub const THEME_LIGHT_ICON: UiText = UiText::new(super::THEME_LIGHT_ICON, c"☀");
    pub fn theme_light_icon() -> &'static CStr { THEME_LIGHT_ICON.cstr() }
    pub const THEME_TO_LIGHT_TIP: UiText = UiText::new(super::THEME_TO_LIGHT_TIP, c"Switch to light theme (Ctrl+Shift+L)");
    pub fn theme_to_light_tip() -> &'static CStr { THEME_TO_LIGHT_TIP.cstr() }
    pub const THEME_TO_DARK_TIP: UiText = UiText::new(super::THEME_TO_DARK_TIP, c"Switch to dark theme (Ctrl+Shift+L)");
    pub fn theme_to_dark_tip() -> &'static CStr { THEME_TO_DARK_TIP.cstr() }
    pub const SPLIT_VERT: UiText = UiText::new(super::SPLIT_VERT, c"◫");
    pub fn split_vert() -> &'static CStr { SPLIT_VERT.cstr() }
    pub const SPLIT_VERT_TIP: UiText = UiText::new(super::SPLIT_VERT_TIP, c"Split tab vertically (Ctrl+Shift+D)");
    pub fn split_vert_tip() -> &'static CStr { SPLIT_VERT_TIP.cstr() }
    pub const SPLIT_HORIZ: UiText = UiText::new(super::SPLIT_HORIZ, c"⬒");
    pub fn split_horiz() -> &'static CStr { SPLIT_HORIZ.cstr() }
    pub const SPLIT_HORIZ_TIP: UiText = UiText::new(super::SPLIT_HORIZ_TIP, c"Split tab horizontally (Ctrl+Shift+E)");
    pub fn split_horiz_tip() -> &'static CStr { SPLIT_HORIZ_TIP.cstr() }
    pub const RECONNECT: UiText = UiText::new(super::RECONNECT, c"Reconnect");
    pub fn reconnect() -> &'static CStr { RECONNECT.cstr() }
    pub const RECONNECT_TIP: UiText = UiText::new(super::RECONNECT_TIP, c"Reconnect active SSH or tmux tab (Ctrl+Shift+R)");
    pub fn reconnect_tip() -> &'static CStr { RECONNECT_TIP.cstr() }
    pub const INTERRUPT: UiText = UiText::new(super::INTERRUPT, c"SIGINT");
    pub fn interrupt() -> &'static CStr { INTERRUPT.cstr() }
    pub const INTERRUPT_TIP: UiText = UiText::new(super::INTERRUPT_TIP, c"Interrupt active process (SIGINT)");
    pub fn interrupt_tip() -> &'static CStr { INTERRUPT_TIP.cstr() }
    pub const PASTE_IMAGE: UiText = UiText::new(super::PASTE_IMAGE, c"Screenshot");
    pub fn paste_image() -> &'static CStr { PASTE_IMAGE.cstr() }
    pub const PASTE_IMAGE_TIP: UiText = UiText::new(super::PASTE_IMAGE_TIP, c"Save clipboard image into active session and paste path");
    pub fn paste_image_tip() -> &'static CStr { PASTE_IMAGE_TIP.cstr() }
    pub const ZOOM_IN: UiText = UiText::new(super::ZOOM_IN, c"+");
    pub fn zoom_in() -> &'static CStr { ZOOM_IN.cstr() }
    pub const ZOOM_IN_TIP: UiText = UiText::new(super::ZOOM_IN_TIP, c"Increase terminal font size (Ctrl++)");
    pub fn zoom_in_tip() -> &'static CStr { ZOOM_IN_TIP.cstr() }
    pub const ZOOM_RESET: UiText = UiText::new(super::ZOOM_RESET, c"100%");
    pub fn zoom_reset() -> &'static CStr { ZOOM_RESET.cstr() }
    pub const ZOOM_RESET_TIP: UiText = UiText::new(super::ZOOM_RESET_TIP, c"Reset terminal zoom (Ctrl+0)");
    pub fn zoom_reset_tip() -> &'static CStr { ZOOM_RESET_TIP.cstr() }
    pub const ZOOM_OUT: UiText = UiText::new(super::ZOOM_OUT, c"−");
    pub fn zoom_out() -> &'static CStr { ZOOM_OUT.cstr() }
    pub const ZOOM_OUT_TIP: UiText = UiText::new(super::ZOOM_OUT_TIP, c"Decrease terminal font size (Ctrl+-)");
    pub fn zoom_out_tip() -> &'static CStr { ZOOM_OUT_TIP.cstr() }
    pub const NEW_TAB: UiText = UiText::new(super::NEW_TAB, c"+");
    pub fn new_tab() -> &'static CStr { NEW_TAB.cstr() }
    pub const NEW_TAB_TIP: UiText = UiText::new(super::NEW_TAB_TIP, c"New tab (Ctrl+Shift+T)");
    pub fn new_tab_tip() -> &'static CStr { NEW_TAB_TIP.cstr() }
    pub const CLOSE_TAB: UiText = UiText::new(super::CLOSE_TAB, c"×");
    pub fn close_tab() -> &'static CStr { CLOSE_TAB.cstr() }
    pub const CLOSE_TAB_TIP: UiText = UiText::new(super::CLOSE_TAB_TIP, c"Close tab");
    pub fn close_tab_tip() -> &'static CStr { CLOSE_TAB_TIP.cstr() }
    pub const PIN_TAB_TITLE: UiText = UiText::new(super::PIN_TAB_TITLE, c"Tab Title");
    pub fn pin_tab_title() -> &'static CStr { PIN_TAB_TITLE.cstr() }
    pub const PIN_TAB_PLACEHOLDER: UiText = UiText::new(super::PIN_TAB_PLACEHOLDER, c"Pinned title (empty — auto)");
    pub fn pin_tab_placeholder() -> &'static CStr { PIN_TAB_PLACEHOLDER.cstr() }
    pub const PIN_TAB_SAVE: UiText = UiText::new(super::PIN_TAB_SAVE, c"Pin");
    pub fn pin_tab_save() -> &'static CStr { PIN_TAB_SAVE.cstr() }
    pub const PIN_TAB_RESET: UiText = UiText::new(super::PIN_TAB_RESET, c"Reset");
    pub fn pin_tab_reset() -> &'static CStr { PIN_TAB_RESET.cstr() }
    pub const STATUS_APPROVAL: UiText = UiText::new(super::STATUS_APPROVAL, c"Needs approval");
    pub fn status_approval() -> &'static CStr { STATUS_APPROVAL.cstr() }
    pub const APPROVAL_TAB_TIP: UiText = UiText::new(super::APPROVAL_TAB_TIP, c"Needs approval (Ctrl+Shift+U to jump)");
    pub fn approval_tab_tip() -> &'static CStr { APPROVAL_TAB_TIP.cstr() }
    pub const NOTIFY_APPROVAL_TITLE: UiText = UiText::new(super::NOTIFY_APPROVAL_TITLE, c"AkhTerm · Needs Approval");
    pub fn notify_approval_title() -> &'static CStr { NOTIFY_APPROVAL_TITLE.cstr() }
    pub const NOTIFY_READY_TITLE: UiText = UiText::new(super::NOTIFY_READY_TITLE, c"AkhTerm · Ready");
    pub fn notify_ready_title() -> &'static CStr { NOTIFY_READY_TITLE.cstr() }
    pub const TMUX_MAIN: UiText = UiText::new(super::TMUX_MAIN, c"tmux:main");
    pub fn tmux_main() -> &'static CStr { TMUX_MAIN.cstr() }
    pub const AGENTS_WORKTREE_HEADER: UiText = UiText::new(super::AGENTS_WORKTREE_HEADER, c"NEW GIT WORKTREE · SEPARATE TAB");
    pub fn agents_worktree_header() -> &'static CStr { AGENTS_WORKTREE_HEADER.cstr() }
    pub const AGENT_WORKTREE_TIP: UiText = UiText::new(super::AGENT_WORKTREE_TIP, c"Create an isolated Git worktree and launch agent in a new tab");
    pub fn agent_worktree_tip() -> &'static CStr { AGENT_WORKTREE_TIP.cstr() }
    pub const WORKTREE_LOCAL_ONLY: UiText = UiText::new(super::WORKTREE_LOCAL_ONLY, c"⚠ Git worktree is only available for a local repository tab");
    pub fn worktree_local_only() -> &'static CStr { WORKTREE_LOCAL_ONLY.cstr() }
    pub const PALETTE_TITLE: UiText = UiText::new(super::PALETTE_TITLE, c"Command Palette");
    pub fn palette_title() -> &'static CStr { PALETTE_TITLE.cstr() }
    pub const PALETTE_HINT: UiText = UiText::new(super::PALETTE_HINT, c"↑↓ select · Enter run · Esc close");
    pub fn palette_hint() -> &'static CStr { PALETTE_HINT.cstr() }
    pub const PALETTE_PLACEHOLDER: UiText = UiText::new(super::PALETTE_PLACEHOLDER, c"Tab, folder, tmux session, SSH, agent, or Handoff...");
    pub fn palette_placeholder() -> &'static CStr { PALETTE_PLACEHOLDER.cstr() }
    pub const PALETTE_EMPTY: UiText = UiText::new(super::PALETTE_EMPTY, c"Nothing found");
    pub fn palette_empty() -> &'static CStr { PALETTE_EMPTY.cstr() }
    pub const PALETTE_BADGE_TAB: UiText = UiText::new(super::PALETTE_BADGE_TAB, c"Tab");
    pub fn palette_badge_tab() -> &'static CStr { PALETTE_BADGE_TAB.cstr() }
    pub const PALETTE_BADGE_TMUX: UiText = UiText::new(super::PALETTE_BADGE_TMUX, c"tmux");
    pub fn palette_badge_tmux() -> &'static CStr { PALETTE_BADGE_TMUX.cstr() }
    pub const PALETTE_BADGE_PROFILE: UiText = UiText::new(super::PALETTE_BADGE_PROFILE, c"Profile");
    pub fn palette_badge_profile() -> &'static CStr { PALETTE_BADGE_PROFILE.cstr() }
    pub const PALETTE_BADGE_SSH: UiText = UiText::new(super::PALETTE_BADGE_SSH, c"SSH");
    pub fn palette_badge_ssh() -> &'static CStr { PALETTE_BADGE_SSH.cstr() }
    pub const PALETTE_BADGE_SSH_TMUX: UiText = UiText::new(super::PALETTE_BADGE_SSH_TMUX, c"SSH · tmux");
    pub fn palette_badge_ssh_tmux() -> &'static CStr { PALETTE_BADGE_SSH_TMUX.cstr() }
    pub const PALETTE_BADGE_AGENT: UiText = UiText::new(super::PALETTE_BADGE_AGENT, c"Agent");
    pub fn palette_badge_agent() -> &'static CStr { PALETTE_BADGE_AGENT.cstr() }
    pub const PALETTE_BADGE_HANDOFF: UiText = UiText::new(super::PALETTE_BADGE_HANDOFF, c"Handoff");
    pub fn palette_badge_handoff() -> &'static CStr { PALETTE_BADGE_HANDOFF.cstr() }
    pub const PALETTE_BADGE_THEME: UiText = UiText::new(super::PALETTE_BADGE_THEME, c"Theme");
    pub fn palette_badge_theme() -> &'static CStr { PALETTE_BADGE_THEME.cstr() }
    pub const PALETTE_BADGE_SEARCH: UiText = UiText::new(super::PALETTE_BADGE_SEARCH, c"Search");
    pub fn palette_badge_search() -> &'static CStr { PALETTE_BADGE_SEARCH.cstr() }
    pub const PALETTE_BADGE_GIT: UiText = UiText::new(super::PALETTE_BADGE_GIT, c"Git");
    pub fn palette_badge_git() -> &'static CStr { PALETTE_BADGE_GIT.cstr() }
    pub const PALETTE_BADGE_SETTINGS: UiText = UiText::new(c"Настройки", c"Settings");
    pub fn palette_badge_settings() -> &'static CStr { PALETTE_BADGE_SETTINGS.cstr() }
    pub const PALETTE_SEARCH_TITLE: UiText = UiText::new(super::PALETTE_SEARCH_TITLE, c"Search Output");
    pub fn palette_search_title() -> &'static CStr { PALETTE_SEARCH_TITLE.cstr() }
    pub const PALETTE_SEARCH_DETAIL: UiText = UiText::new(super::PALETTE_SEARCH_DETAIL, c"Find text or regex in active terminal · Ctrl+Shift+F");
    pub fn palette_search_detail() -> &'static CStr { PALETTE_SEARCH_DETAIL.cstr() }
    pub const PALETTE_SETTINGS_TITLE: UiText = UiText::new(c"Настройки AkhTerm", c"AkhTerm Settings");
    pub fn palette_settings_title() -> &'static CStr { PALETTE_SETTINGS_TITLE.cstr() }
    pub const PALETTE_SETTINGS_DETAIL: UiText = UiText::new(c"Тема, язык, шрифт, scrollback, OSC 52 · Ctrl+,", c"Theme, language, font, scrollback, OSC 52 · Ctrl+,");
    pub fn palette_settings_detail() -> &'static CStr { PALETTE_SETTINGS_DETAIL.cstr() }
    pub const PALETTE_ABOUT_TITLE: UiText = UiText::new(c"О программе AkhTerm", c"About AkhTerm");
    pub fn palette_about_title() -> &'static CStr { PALETTE_ABOUT_TITLE.cstr() }
    pub const PALETTE_ABOUT_DETAIL: UiText = UiText::new(c"Версия, git SHA, GTK/VTE, Wayland/X11 и пути конфига", c"Version, git SHA, GTK/VTE, Wayland/X11, and config paths");
    pub fn palette_about_detail() -> &'static CStr { PALETTE_ABOUT_DETAIL.cstr() }

    pub const SEARCH_PLACEHOLDER: UiText = UiText::new(super::SEARCH_PLACEHOLDER, c"Search terminal output...");
    pub fn search_placeholder() -> &'static CStr { SEARCH_PLACEHOLDER.cstr() }
    pub const SEARCH_PREV: UiText = UiText::new(super::SEARCH_PREV, c"↑");
    pub fn search_prev() -> &'static CStr { SEARCH_PREV.cstr() }
    pub const SEARCH_PREV_TIP: UiText = UiText::new(super::SEARCH_PREV_TIP, c"Previous match (Shift+Enter)");
    pub fn search_prev_tip() -> &'static CStr { SEARCH_PREV_TIP.cstr() }
    pub const SEARCH_NEXT: UiText = UiText::new(super::SEARCH_NEXT, c"↓");
    pub fn search_next() -> &'static CStr { SEARCH_NEXT.cstr() }
    pub const SEARCH_NEXT_TIP: UiText = UiText::new(super::SEARCH_NEXT_TIP, c"Next match (Enter)");
    pub fn search_next_tip() -> &'static CStr { SEARCH_NEXT_TIP.cstr() }
    pub const SEARCH_CASE: UiText = UiText::new(super::SEARCH_CASE, c"Aa");
    pub fn search_case() -> &'static CStr { SEARCH_CASE.cstr() }
    pub const SEARCH_CASE_TIP: UiText = UiText::new(super::SEARCH_CASE_TIP, c"Match case");
    pub fn search_case_tip() -> &'static CStr { SEARCH_CASE_TIP.cstr() }
    pub const SEARCH_REGEX: UiText = UiText::new(super::SEARCH_REGEX, c".*");
    pub fn search_regex() -> &'static CStr { SEARCH_REGEX.cstr() }
    pub const SEARCH_REGEX_TIP: UiText = UiText::new(super::SEARCH_REGEX_TIP, c"Regular expression (PCRE2)");
    pub fn search_regex_tip() -> &'static CStr { SEARCH_REGEX_TIP.cstr() }
    pub const SEARCH_CLOSE_TIP: UiText = UiText::new(super::SEARCH_CLOSE_TIP, c"Close search (Esc)");
    pub fn search_close_tip() -> &'static CStr { SEARCH_CLOSE_TIP.cstr() }
    pub const SEARCH_FOUND: UiText = UiText::new(super::SEARCH_FOUND, c"Found");
    pub fn search_found() -> &'static CStr { SEARCH_FOUND.cstr() }
    pub const SEARCH_NOT_FOUND: UiText = UiText::new(super::SEARCH_NOT_FOUND, c"No matches");
    pub fn search_not_found() -> &'static CStr { SEARCH_NOT_FOUND.cstr() }
    pub const SEARCH_REGEX_ERR: UiText = UiText::new(super::SEARCH_REGEX_ERR, c"Regex error");
    pub fn search_regex_err() -> &'static CStr { SEARCH_REGEX_ERR.cstr() }

    pub const GIT_MENU: UiText = UiText::new(super::GIT_MENU, c"Git ▾");
    pub fn git_menu() -> &'static CStr { GIT_MENU.cstr() }
    pub const GIT_MENU_TIP: UiText = UiText::new(super::GIT_MENU_TIP, c"Git modified files, quick diff, and worktree cleanup (Ctrl+Shift+G)");
    pub fn git_menu_tip() -> &'static CStr { GIT_MENU_TIP.cstr() }
    pub const GIT_LOCAL_ONLY: UiText = UiText::new(super::GIT_LOCAL_ONLY, c"⚠ Git menu is only available for a local tab");
    pub fn git_local_only() -> &'static CStr { GIT_LOCAL_ONLY.cstr() }
    pub const GIT_HEADER: UiText = UiText::new(super::GIT_HEADER, c"GIT · LOCAL TAB");
    pub fn git_header() -> &'static CStr { GIT_HEADER.cstr() }
    pub const GIT_NOT_REPO: UiText = UiText::new(super::GIT_NOT_REPO, c"Current directory is not a Git repository");
    pub fn git_not_repo() -> &'static CStr { GIT_NOT_REPO.cstr() }
    pub const GIT_FILES_HEADER: UiText = UiText::new(super::GIT_FILES_HEADER, c"MODIFIED & UNTRACKED FILES (CLICK TO DIFF / OPEN)");
    pub fn git_files_header() -> &'static CStr { GIT_FILES_HEADER.cstr() }
    pub const GIT_CLEAN: UiText = UiText::new(super::GIT_CLEAN, c"Working tree clean — no uncommitted changes");
    pub fn git_clean() -> &'static CStr { GIT_CLEAN.cstr() }
    pub const GIT_LOADING: UiText = UiText::new(super::GIT_LOADING, c"Refreshing Git status in background…");
    pub fn git_loading() -> &'static CStr { GIT_LOADING.cstr() }
    pub const GIT_REMOVE_WT_BTN: UiText = UiText::new(super::GIT_REMOVE_WT_BTN, c"Remove clean worktree");
    pub fn git_remove_wt_btn() -> &'static CStr { GIT_REMOVE_WT_BTN.cstr() }
    pub const GIT_REMOVE_WT_TIP: UiText = UiText::new(super::GIT_REMOVE_WT_TIP, c"Verify no changes remain and remove isolated Git worktree");
    pub fn git_remove_wt_tip() -> &'static CStr { GIT_REMOVE_WT_TIP.cstr() }

    pub const HANDOFF_TIP: UiText = UiText::new(super::HANDOFF_TIP, c"Capture active tab log or forward an .md brief to a session");
    pub fn handoff_tip() -> &'static CStr { HANDOFF_TIP.cstr() }
    pub const HANDOFF_CAPTURE_BTN: UiText = UiText::new(super::HANDOFF_CAPTURE_BTN, c"Save active tab output (.md)");
    pub fn handoff_capture_btn() -> &'static CStr { HANDOFF_CAPTURE_BTN.cstr() }
    pub const HANDOFF_CAPTURE_TIP: UiText = UiText::new(super::HANDOFF_CAPTURE_TIP, c"Save recent terminal lines to a Markdown file (0600)");
    pub fn handoff_capture_tip() -> &'static CStr { HANDOFF_CAPTURE_TIP.cstr() }
    pub const HANDOFF_FORWARD_HEADER: UiText = UiText::new(super::HANDOFF_FORWARD_HEADER, c"FORWARD ACTIVE TAB OUTPUT TO SESSION");
    pub fn handoff_forward_header() -> &'static CStr { HANDOFF_FORWARD_HEADER.cstr() }
    pub const HANDOFF_BRIEFS_HEADER: UiText = UiText::new(super::HANDOFF_BRIEFS_HEADER, c"BRIEFS & CAPTURES (.MD) — INTO ACTIVE TAB");
    pub fn handoff_briefs_header() -> &'static CStr { HANDOFF_BRIEFS_HEADER.cstr() }
    pub const HANDOFF_EMPTY: UiText = UiText::new(super::HANDOFF_EMPTY, c"No saved .md files — capture output above");
    pub fn handoff_empty() -> &'static CStr { HANDOFF_EMPTY.cstr() }
    pub const DROP_SCP_START: UiText = UiText::new(super::DROP_SCP_START, c"⬆ Uploading via SCP to");
    pub fn drop_scp_start() -> &'static CStr { DROP_SCP_START.cstr() }
    pub const DROP_SCP_DONE: UiText = UiText::new(super::DROP_SCP_DONE, c"📎 Uploaded via SCP to");
    pub fn drop_scp_done() -> &'static CStr { DROP_SCP_DONE.cstr() }
    pub const DROP_SCP_ERROR: UiText = UiText::new(super::DROP_SCP_ERROR, c"⚠ SCP upload error to");
    pub fn drop_scp_error() -> &'static CStr { DROP_SCP_ERROR.cstr() }
    pub const DROP_SCP_NO_FILE: UiText = UiText::new(super::DROP_SCP_NO_FILE, c"⚠ SCP error: local file unavailable");
    pub fn drop_scp_no_file() -> &'static CStr { DROP_SCP_NO_FILE.cstr() }

    pub const TOKENS: UiText = UiText::new(super::TOKENS, c"Tokens ▾");
    pub fn tokens() -> &'static CStr { TOKENS.cstr() }
    pub const TOKENS_TIP: UiText = UiText::new(super::TOKENS_TIP, c"Token usage, context window, and agent turn timer per tab");
    pub fn tokens_tip() -> &'static CStr { TOKENS_TIP.cstr() }
    pub const TOKENS_HEADER: UiText = UiText::new(super::TOKENS_HEADER, c"TOKENS · CONTEXT & TURN DURATION");
    pub fn tokens_header() -> &'static CStr { TOKENS_HEADER.cstr() }
    pub const TOKENS_EMPTY: UiText = UiText::new(super::TOKENS_EMPTY, c"No token telemetry for open tabs.\nStart an agent (AGY, Claude Code, Codex CLI) or a session with context usage.");
    pub fn tokens_empty() -> &'static CStr { TOKENS_EMPTY.cstr() }

    pub const CTX_COPY: UiText = UiText::new(super::CTX_COPY, c"Copy");
    pub fn ctx_copy() -> &'static CStr { CTX_COPY.cstr() }
    pub const CTX_PASTE: UiText = UiText::new(super::CTX_PASTE, c"Paste");
    pub fn ctx_paste() -> &'static CStr { CTX_PASTE.cstr() }
    pub const CTX_OPEN_LINK: UiText = UiText::new(super::CTX_OPEN_LINK, c"Open Link");
    pub fn ctx_open_link() -> &'static CStr { CTX_OPEN_LINK.cstr() }
    pub const CTX_COPY_LINK: UiText = UiText::new(super::CTX_COPY_LINK, c"Copy Link");
    pub fn ctx_copy_link() -> &'static CStr { CTX_COPY_LINK.cstr() }
    pub const CTX_OPEN_FOLDER: UiText = UiText::new(super::CTX_OPEN_FOLDER, c"Open Containing Folder");
    pub fn ctx_open_folder() -> &'static CStr { CTX_OPEN_FOLDER.cstr() }
    pub fn ctx_open_folder_str() -> &'static str { CTX_OPEN_FOLDER.as_str() }
    pub const CTX_SEND_TO_TAB: UiText = UiText::new(super::CTX_SEND_TO_TAB, c"Send to Tab ▸");
    pub fn ctx_send_to_tab() -> &'static CStr { CTX_SEND_TO_TAB.cstr() }
    pub const CTX_SPLIT_RIGHT: UiText = UiText::new(super::CTX_SPLIT_RIGHT, c"Split Right");
    pub fn ctx_split_right() -> &'static CStr { CTX_SPLIT_RIGHT.cstr() }
    pub const CTX_SPLIT_DOWN: UiText = UiText::new(super::CTX_SPLIT_DOWN, c"Split Down");
    pub fn ctx_split_down() -> &'static CStr { CTX_SPLIT_DOWN.cstr() }
    pub const CTX_CLOSE_PANE: UiText = UiText::new(super::CTX_CLOSE_PANE, c"Close Pane");
    pub fn ctx_close_pane() -> &'static CStr { CTX_CLOSE_PANE.cstr() }
    pub const CTX_CLEAR_BUFFER: UiText = UiText::new(super::CTX_CLEAR_BUFFER, c"Clear Buffer");
    pub fn ctx_clear_buffer() -> &'static CStr { CTX_CLEAR_BUFFER.cstr() }

    pub const CTX_RENAME_TAB: UiText = UiText::new(super::CTX_RENAME_TAB, c"Rename Tab…");
    pub fn ctx_rename_tab() -> &'static CStr { CTX_RENAME_TAB.cstr() }
    pub const CTX_DUPLICATE_TAB: UiText = UiText::new(super::CTX_DUPLICATE_TAB, c"Duplicate Tab");
    pub fn ctx_duplicate_tab() -> &'static CStr { CTX_DUPLICATE_TAB.cstr() }
    pub const CTX_COPY_PATH: UiText = UiText::new(super::CTX_COPY_PATH, c"Copy Path");
    pub fn ctx_copy_path() -> &'static CStr { CTX_COPY_PATH.cstr() }
    pub const CTX_CLOSE_TAB: UiText = UiText::new(super::CTX_CLOSE_TAB, c"Close Tab");
    pub fn ctx_close_tab() -> &'static CStr { CTX_CLOSE_TAB.cstr() }
    pub const CTX_CLOSE_OTHER_TABS: UiText = UiText::new(super::CTX_CLOSE_OTHER_TABS, c"Close Other Tabs");
    pub fn ctx_close_other_tabs() -> &'static CStr { CTX_CLOSE_OTHER_TABS.cstr() }
    pub const CTX_CURRENT_SESSION: UiText = UiText::new(c"(текущая)", c"(current)");
    pub fn ctx_current_session_str() -> &'static str { CTX_CURRENT_SESSION.as_str() }
    pub const RENAME_PLACEHOLDER: UiText = UiText::new(super::RENAME_PLACEHOLDER, c"Tab name (empty — auto)");
    pub fn rename_placeholder() -> &'static CStr { RENAME_PLACEHOLDER.cstr() }
    pub const RENAME_SAVE: UiText = UiText::new(super::RENAME_SAVE, c"Save");
    pub fn rename_save() -> &'static CStr { RENAME_SAVE.cstr() }

    pub const SETTINGS_BTN: UiText = UiText::new(c"Настройки", c"Settings");
    pub fn settings_btn() -> &'static CStr { SETTINGS_BTN.cstr() }
    pub const SETTINGS_TIP: UiText = UiText::new(c"Настройки AkhTerm (Ctrl+,)", c"AkhTerm Settings (Ctrl+,)");
    pub fn settings_tip() -> &'static CStr { SETTINGS_TIP.cstr() }
    pub const SETTINGS_TITLE: UiText = UiText::new(c"Настройки AkhTerm", c"AkhTerm Settings");
    pub fn settings_title() -> &'static CStr { SETTINGS_TITLE.cstr() }
    pub fn settings_title_str() -> &'static str { SETTINGS_TITLE.as_str() }
    pub const SETTINGS_SUBTITLE: UiText = UiText::new(c"Параметры применяются сразу и сохраняются в config.conf", c"Changes apply live and are saved to config.conf");
    pub fn settings_subtitle() -> &'static CStr { SETTINGS_SUBTITLE.cstr() }
    pub const SETTINGS_THEME: UiText = UiText::new(c"Тема оформления", c"Appearance Theme");
    pub fn settings_theme() -> &'static CStr { SETTINGS_THEME.cstr() }
    pub const SETTINGS_THEME_DARK: UiText = UiText::new(c"Тёмная (Dark Zinc)", c"Dark (Dark Zinc)");
    pub fn settings_theme_dark() -> &'static CStr { SETTINGS_THEME_DARK.cstr() }
    pub const SETTINGS_THEME_LIGHT: UiText = UiText::new(c"Светлая (Light Zinc)", c"Light (Light Zinc)");
    pub fn settings_theme_light() -> &'static CStr { SETTINGS_THEME_LIGHT.cstr() }
    pub const SETTINGS_LOCALE: UiText = UiText::new(c"Язык интерфейса", c"Interface Language");
    pub fn settings_locale() -> &'static CStr { SETTINGS_LOCALE.cstr() }
    pub const SETTINGS_LOCALE_AUTO: UiText = UiText::new(c"Авто (по системе)", c"Auto (System)");
    pub fn settings_locale_auto() -> &'static CStr { SETTINGS_LOCALE_AUTO.cstr() }
    pub const SETTINGS_LOCALE_RU: UiText = UiText::new(c"Русский", c"Русский");
    pub fn settings_locale_ru() -> &'static CStr { SETTINGS_LOCALE_RU.cstr() }
    pub const SETTINGS_LOCALE_EN: UiText = UiText::new(c"English", c"English");
    pub fn settings_locale_en() -> &'static CStr { SETTINGS_LOCALE_EN.cstr() }
    pub const SETTINGS_FONT: UiText = UiText::new(c"Шрифт и кегль", c"Terminal Font");
    pub fn settings_font() -> &'static CStr { SETTINGS_FONT.cstr() }
    pub const SETTINGS_FONT_PLACEHOLDER: UiText = UiText::new(c"Например: DejaVu Sans Mono 10.5", c"e.g. DejaVu Sans Mono 10.5");
    pub fn settings_font_placeholder() -> &'static CStr { SETTINGS_FONT_PLACEHOLDER.cstr() }
    pub const SETTINGS_SCROLLBACK: UiText = UiText::new(c"Глубина прокрутки (строк)", c"Scrollback Lines");
    pub fn settings_scrollback() -> &'static CStr { SETTINGS_SCROLLBACK.cstr() }
    pub const SETTINGS_OSC52: UiText = UiText::new(c"Буфер обмена OSC 52", c"OSC 52 Clipboard");
    pub fn settings_osc52() -> &'static CStr { SETTINGS_OSC52.cstr() }
    pub const SETTINGS_OSC52_HINT: UiText = UiText::new(c"Разрешить запись из терминала (чтение заблокировано)", c"Allow write-only terminal clipboard copy (reads blocked)");
    pub fn settings_osc52_hint() -> &'static CStr { SETTINGS_OSC52_HINT.cstr() }
    pub const SETTINGS_NOTIFY_SECS: UiText = UiText::new(c"Порог уведомлений (сек)", c"Long Command Notify (sec)");
    pub fn settings_notify_secs() -> &'static CStr { SETTINGS_NOTIFY_SECS.cstr() }
    pub const SETTINGS_NOTIFY_HINT: UiText = UiText::new(c"Для команд дольше N сек (0 — выключено)", c"Notify after N sec (0 — disabled)");
    pub fn settings_notify_hint() -> &'static CStr { SETTINGS_NOTIFY_HINT.cstr() }
    pub const SETTINGS_ABOUT_BTN: UiText = UiText::new(c"О программе…", c"About AkhTerm…");
    pub fn settings_about_btn() -> &'static CStr { SETTINGS_ABOUT_BTN.cstr() }
    pub const SETTINGS_CLOSE_BTN: UiText = UiText::new(c"Закрыть", c"Close");
    pub fn settings_close_btn() -> &'static CStr { SETTINGS_CLOSE_BTN.cstr() }

    pub const ABOUT_TITLE: UiText = UiText::new(c"О программе — AkhTerm", c"About AkhTerm");
    pub fn about_title() -> &'static CStr { ABOUT_TITLE.cstr() }
    pub const ABOUT_SUBTITLE: UiText = UiText::new(c"Нативный Linux-терминал для агентных потоков, SSH и tmux", c"Native Linux terminal for agent workflows, SSH, and tmux");
    pub fn about_subtitle() -> &'static CStr { ABOUT_SUBTITLE.cstr() }
    pub const ABOUT_BUILD_LBL: UiText = UiText::new(c"Сборка", c"Build");
    pub fn about_build_lbl() -> &'static CStr { ABOUT_BUILD_LBL.cstr() }
    pub const ABOUT_STACK_LBL: UiText = UiText::new(c"Стек", c"Runtime Stack");
    pub fn about_stack_lbl() -> &'static CStr { ABOUT_STACK_LBL.cstr() }
    pub const ABOUT_DISPLAY_LBL: UiText = UiText::new(c"Графика и DE", c"Display & DE");
    pub fn about_display_lbl() -> &'static CStr { ABOUT_DISPLAY_LBL.cstr() }
    pub const ABOUT_CONFIG_LBL: UiText = UiText::new(c"Конфиг", c"Config File");
    pub fn about_config_lbl() -> &'static CStr { ABOUT_CONFIG_LBL.cstr() }
    pub const ABOUT_STATE_LBL: UiText = UiText::new(c"Состояние", c"State Dir");
    pub fn about_state_lbl() -> &'static CStr { ABOUT_STATE_LBL.cstr() }
    pub const ABOUT_LICENSE_LBL: UiText = UiText::new(c"Лицензия", c"License");
    pub fn about_license_lbl() -> &'static CStr { ABOUT_LICENSE_LBL.cstr() }
    pub const ABOUT_LICENSE_VAL: UiText = UiText::new(c"MIT License", c"MIT License");
    pub fn about_license_val() -> &'static CStr { ABOUT_LICENSE_VAL.cstr() }
    pub const ABOUT_COPY_BTN: UiText = UiText::new(c"Скопировать данные сборки", c"Copy Build Info");
    pub fn about_copy_btn() -> &'static CStr { ABOUT_COPY_BTN.cstr() }
    pub fn about_copy_btn_str() -> &'static str { ABOUT_COPY_BTN.as_str() }
    pub const ABOUT_COPIED_BTN: UiText = UiText::new(c"Скопировано ✓", c"Copied ✓");
    pub fn about_copied_btn() -> &'static CStr { ABOUT_COPIED_BTN.cstr() }
    pub const ABOUT_CLOSE_BTN: UiText = UiText::new(c"Закрыть", c"Close");
    pub fn about_close_btn() -> &'static CStr { ABOUT_CLOSE_BTN.cstr() }

    pub fn theme_palette_title(is_light: bool) -> &'static str {
        if is_light {
            UiText::new(c"☾ Тёмная тема (Dark Zinc)", c"☾ Dark Theme (Dark Zinc)").as_str()
        } else {
            UiText::new(c"☀ Светлая тема (Light Zinc)", c"☀ Light Theme (Light Zinc)").as_str()
        }
    }

    pub fn theme_palette_detail(is_light: bool) -> &'static str {
        if is_light {
            UiText::new(c"Сейчас: Light Zinc · Ctrl+Shift+L", c"Active: Light Zinc · Ctrl+Shift+L").as_str()
        } else {
            UiText::new(c"Сейчас: Dark Zinc · Ctrl+Shift+L", c"Active: Dark Zinc · Ctrl+Shift+L").as_str()
        }
    }
}

pub use i18n_text::*;

pub fn theme_toggle_icon(is_light: bool) -> &'static CStr {
    if is_light { THEME_DARK_ICON } else { THEME_LIGHT_ICON }
}

pub fn theme_toggle_tooltip(is_light: bool) -> &'static CStr {
    if is_light {
        i18n_text::theme_to_dark_tip()
    } else {
        i18n_text::theme_to_light_tip()
    }
}

pub fn cstr_text(value: &'static CStr) -> &'static str {
    value.to_str().unwrap_or("")
}

pub fn tr(entry: UiText) -> &'static CStr {
    entry.cstr()
}

pub fn tr_str(entry: UiText) -> &'static str {
    entry.as_str()
}

pub fn locale_is_ru() -> bool {
    current_locale() == UiLocale::Ru
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::with_locale_lock;

    fn assert_pair_valid(entry: UiText) {
        let ru = entry.for_locale(UiLocale::Ru).to_str().expect("utf8 ru");
        let en = entry.for_locale(UiLocale::En).to_str().expect("utf8 en");
        assert!(!ru.trim().is_empty(), "empty RU text");
        assert!(!en.trim().is_empty(), "empty EN text");
    }

    fn verify_core_and_palette_pairs() {
        for entry in [
            i18n_text::APP_NAME, i18n_text::PROFILES, i18n_text::SERVERS, i18n_text::AGENTS,
            i18n_text::HANDOFF, i18n_text::MAINTAINABILITY, i18n_text::MI_EMPTY,
            i18n_text::MI_REFACTOR, i18n_text::MI_REFACTOR_TIP, i18n_text::TOGGLE_SIDEBAR,
            i18n_text::TOGGLE_SIDEBAR_TIP, i18n_text::THEME_TO_LIGHT_TIP, i18n_text::THEME_TO_DARK_TIP,
            i18n_text::SPLIT_VERT_TIP, i18n_text::SPLIT_HORIZ_TIP, i18n_text::RECONNECT,
            i18n_text::RECONNECT_TIP, i18n_text::INTERRUPT, i18n_text::INTERRUPT_TIP,
            i18n_text::PASTE_IMAGE, i18n_text::PASTE_IMAGE_TIP, i18n_text::ZOOM_IN_TIP,
            i18n_text::ZOOM_RESET_TIP, i18n_text::ZOOM_OUT_TIP, i18n_text::NEW_TAB_TIP,
            i18n_text::CLOSE_TAB_TIP, i18n_text::PIN_TAB_TITLE, i18n_text::PIN_TAB_PLACEHOLDER,
            i18n_text::PIN_TAB_SAVE, i18n_text::PIN_TAB_RESET, i18n_text::STATUS_APPROVAL,
            i18n_text::APPROVAL_TAB_TIP, i18n_text::NOTIFY_APPROVAL_TITLE, i18n_text::NOTIFY_READY_TITLE,
            i18n_text::AGENTS_WORKTREE_HEADER, i18n_text::AGENT_WORKTREE_TIP, i18n_text::WORKTREE_LOCAL_ONLY,
            i18n_text::PALETTE_TITLE, i18n_text::PALETTE_HINT, i18n_text::PALETTE_PLACEHOLDER,
            i18n_text::PALETTE_EMPTY, i18n_text::PALETTE_BADGE_TAB, i18n_text::PALETTE_BADGE_PROFILE,
            i18n_text::PALETTE_BADGE_AGENT, i18n_text::PALETTE_BADGE_THEME, i18n_text::PALETTE_BADGE_SEARCH,
            i18n_text::PALETTE_BADGE_SETTINGS, i18n_text::PALETTE_SEARCH_TITLE, i18n_text::PALETTE_SEARCH_DETAIL,
            i18n_text::PALETTE_SETTINGS_TITLE, i18n_text::PALETTE_ABOUT_TITLE,
        ] {
            assert_pair_valid(entry);
        }
    }

    fn verify_tools_context_and_dialog_pairs() {
        for entry in [
            i18n_text::SEARCH_PLACEHOLDER, i18n_text::SEARCH_PREV_TIP, i18n_text::SEARCH_NEXT_TIP,
            i18n_text::SEARCH_CASE_TIP, i18n_text::SEARCH_REGEX_TIP, i18n_text::SEARCH_CLOSE_TIP,
            i18n_text::SEARCH_FOUND, i18n_text::SEARCH_NOT_FOUND, i18n_text::SEARCH_REGEX_ERR,
            i18n_text::GIT_MENU, i18n_text::GIT_MENU_TIP, i18n_text::GIT_LOCAL_ONLY,
            i18n_text::GIT_HEADER, i18n_text::GIT_NOT_REPO, i18n_text::GIT_FILES_HEADER,
            i18n_text::GIT_CLEAN, i18n_text::GIT_LOADING, i18n_text::GIT_REMOVE_WT_BTN,
            i18n_text::GIT_REMOVE_WT_TIP, i18n_text::HANDOFF_TIP, i18n_text::HANDOFF_CAPTURE_BTN,
            i18n_text::HANDOFF_CAPTURE_TIP, i18n_text::HANDOFF_FORWARD_HEADER, i18n_text::HANDOFF_BRIEFS_HEADER,
            i18n_text::HANDOFF_EMPTY, i18n_text::DROP_SCP_START, i18n_text::DROP_SCP_DONE,
            i18n_text::DROP_SCP_ERROR, i18n_text::DROP_SCP_NO_FILE, i18n_text::TOKENS,
            i18n_text::TOKENS_TIP, i18n_text::TOKENS_HEADER, i18n_text::TOKENS_EMPTY,
            i18n_text::CTX_COPY, i18n_text::CTX_PASTE, i18n_text::CTX_OPEN_LINK,
            i18n_text::CTX_COPY_LINK, i18n_text::CTX_OPEN_FOLDER, i18n_text::CTX_SEND_TO_TAB,
            i18n_text::CTX_SPLIT_RIGHT, i18n_text::CTX_SPLIT_DOWN, i18n_text::CTX_CLOSE_PANE,
            i18n_text::CTX_CLEAR_BUFFER, i18n_text::CTX_RENAME_TAB, i18n_text::CTX_DUPLICATE_TAB,
            i18n_text::CTX_COPY_PATH, i18n_text::CTX_CLOSE_TAB, i18n_text::CTX_CLOSE_OTHER_TABS,
            i18n_text::CTX_CURRENT_SESSION, i18n_text::RENAME_PLACEHOLDER, i18n_text::RENAME_SAVE,
            i18n_text::SETTINGS_BTN, i18n_text::SETTINGS_TIP, i18n_text::SETTINGS_TITLE,
            i18n_text::SETTINGS_SUBTITLE, i18n_text::SETTINGS_THEME, i18n_text::SETTINGS_LOCALE,
            i18n_text::SETTINGS_FONT, i18n_text::SETTINGS_SCROLLBACK, i18n_text::SETTINGS_OSC52,
            i18n_text::SETTINGS_NOTIFY_SECS, i18n_text::SETTINGS_ABOUT_BTN, i18n_text::SETTINGS_CLOSE_BTN,
            i18n_text::ABOUT_TITLE, i18n_text::ABOUT_SUBTITLE, i18n_text::ABOUT_COPY_BTN,
            i18n_text::ABOUT_COPIED_BTN, i18n_text::ABOUT_CLOSE_BTN,
        ] {
            assert_pair_valid(entry);
        }
    }

    #[test]
    fn every_ui_text_entry_has_valid_non_empty_ru_and_en_strings() {
        verify_core_and_palette_pairs();
        verify_tools_context_and_dialog_pairs();
    }

    #[test]
    fn live_getters_switch_between_russian_and_english() {
        with_locale_lock(|| {
            set_locale(UiLocale::Ru);
            assert!(locale_is_ru());
            assert_eq!(profiles_str(), "Профили / SSH ▾");
            assert_eq!(ctx_open_folder_str(), "Открыть папку");
            assert_eq!(settings_title_str(), "Настройки AkhTerm");
            assert_eq!(about_copy_btn_str(), "Скопировать данные сборки");
            assert_eq!(theme_toggle_tooltip(false), THEME_TO_LIGHT_TIP);

            set_locale(UiLocale::En);
            assert!(!locale_is_ru());
            assert_eq!(profiles_str(), "Profiles / SSH ▾");
            assert_eq!(ctx_open_folder_str(), "Open Containing Folder");
            assert_eq!(settings_title_str(), "AkhTerm Settings");
            assert_eq!(about_copy_btn_str(), "Copy Build Info");
            assert_eq!(theme_toggle_tooltip(false), c"Switch to light theme (Ctrl+Shift+L)");
        });
    }
}
