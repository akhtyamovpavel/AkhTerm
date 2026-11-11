//! Static interface labels and tooltips shared by the GTK views.

use std::ffi::CStr;

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

pub fn theme_toggle_icon(is_light: bool) -> &'static CStr {
    if is_light { THEME_DARK_ICON } else { THEME_LIGHT_ICON }
}

pub fn theme_toggle_tooltip(is_light: bool) -> &'static CStr {
    if is_light { THEME_TO_DARK_TIP } else { THEME_TO_LIGHT_TIP }
}

pub fn cstr_text(value: &'static CStr) -> &'static str {
    value.to_str().unwrap_or("")
}
