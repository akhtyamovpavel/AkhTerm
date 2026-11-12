//! Static interface labels and tooltips shared by the GTK views.

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

pub fn theme_toggle_icon(is_light: bool) -> &'static CStr {
    if is_light { THEME_DARK_ICON } else { THEME_LIGHT_ICON }
}

pub fn theme_toggle_tooltip(is_light: bool) -> &'static CStr {
    if is_light { THEME_TO_DARK_TIP } else { THEME_TO_LIGHT_TIP }
}

pub fn cstr_text(value: &'static CStr) -> &'static str {
    value.to_str().unwrap_or("")
}

#[allow(dead_code)]
pub fn tr(entry: UiText) -> &'static CStr {
    entry.cstr()
}

#[allow(dead_code)]
pub fn tr_str(entry: UiText) -> &'static str {
    entry.as_str()
}

#[allow(dead_code)]
pub fn locale_is_ru() -> bool {
    current_locale() == UiLocale::Ru
}
