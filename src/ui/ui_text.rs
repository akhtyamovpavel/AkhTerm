//! Static interface labels and bilingual (`RU` / `EN`) `UiText` registry for AkhTerm.

use std::ffi::CStr;

#[allow(unused_imports)]
pub use crate::i18n::{
    current_locale, detect_system_locale_from_env, set_locale, UiLocale, UiText,
};
#[cfg(test)]
#[allow(unused_imports)]
pub use crate::i18n::configured_locale;

pub const MD_PREVIEW: UiText = UiText::new(c"Открыть рядом", c"Open alongside");
pub const MD_RESULT: UiText = UiText::new(c"Последний Markdown в выводе", c"Last Markdown in output");
pub const MD_CODE: UiText = UiText::new(c"Копировать блок", c"Copy code block");
pub const MD_PASTE: UiText = UiText::new(c"Вставить выделение", c"Paste selection");
pub const MD_REFRESH: UiText = UiText::new(c"Обновить", c"Refresh");
pub const MD_ABSOLUTE: UiText = UiText::new(c"Введи полный путь и нажми Enter", c"Enter an absolute path and press Enter");
pub const MD_LOADING: UiText = UiText::new(c"Читаю по SSH…", c"Reading over SSH…");
pub const MD_CONNECTION: UiText = UiText::new(c"Не удалось определить параметры SSH-соединения", c"Could not identify SSH connection options");
pub const MD_QUOTE: UiText = UiText::new(c"Вставить цитату", c"Insert quote");
pub const MD_TARGET: UiText = UiText::new(c"Выбери агента", c"Choose an agent");
pub const MD_SELECT: UiText = UiText::new(c"Выдели текст в документе", c"Select text in the document");
pub const MD_STALE: UiText = UiText::new(c"Панель или агент изменились. Выбери адресата ещё раз.", c"The pane or agent changed. Choose a recipient again.");
pub const MD_SEARCH: UiText = UiText::new(c"Найти в документе", c"Find in document");
pub const MD_CLOSE: UiText = UiText::new(c"Закрыть просмотр", c"Close preview");
pub const MD_EMPTY: UiText = UiText::new(c"В выводе панели нет локального Markdown-файла", c"No local Markdown file found in pane output");
pub const MD_BLOCK_HINT: UiText = UiText::new(c"Поставь курсор внутрь блока кода", c"Place the cursor in a fenced code block");
pub const DIFF_OPEN_BTN: UiText = UiText::new(c"Ревью diff", c"Review Diff");
pub const DIFF_OPEN_TIP: UiText = UiText::new(
    c"Открыть панель ревью diff с комментариями для агента (Ctrl+Shift+B)",
    c"Open interactive Diff Review panel with comments for agent (Ctrl+Shift+B)",
);
pub const DIFF_MODE_WORKTREE: UiText = UiText::new(c"Рабочее дерево", c"Working tree");
pub const DIFF_MODE_STAGED: UiText = UiText::new(c"В индексе", c"Staged");
pub const DIFF_MODE_BRANCH: UiText = UiText::new(c"Ветка", c"Branch");
pub const DIFF_REFRESH: UiText = UiText::new(c"Обновить", c"Refresh");
pub const DIFF_CLOSE: UiText = UiText::new(c"Закрыть", c"Close");
pub const DIFF_FILES_HDR: UiText = UiText::new(c"ФАЙЛЫ", c"FILES");
pub const DIFF_REVIEWED: UiText = UiText::new(c"Просмотрено", c"Reviewed");
pub const DIFF_HUNK_PREV: UiText = UiText::new(c"▲", c"▲");
pub const DIFF_HUNK_NEXT: UiText = UiText::new(c"▼", c"▼");
pub const DIFF_HUNK_PREV_TIP: UiText = UiText::new(c"Предыдущий ханк", c"Previous hunk");
pub const DIFF_HUNK_NEXT_TIP: UiText = UiText::new(c"Следующий ханк", c"Next hunk");
pub const DIFF_STAGE_HUNK: UiText = UiText::new(c"В индекс", c"Stage hunk");
pub const DIFF_REVERT_HUNK: UiText = UiText::new(c"Откатить ханк", c"Revert hunk");
pub const DIFF_HUNK_LOCAL_ONLY: UiText = UiText::new(
    c"Действия с ханком доступны только для локального рабочего дерева",
    c"Hunk actions are available only for a local working tree",
);
pub const DIFF_COMMENT_PLACEHOLDER: UiText = UiText::new(
    c"Комментарий к выделенным строкам или активному ханку…",
    c"Comment on selected lines or active hunk…",
);
pub const DIFF_ADD_COMMENT: UiText = UiText::new(c"Добавить", c"Add comment");
pub const DIFF_CLEAR_BATCH: UiText = UiText::new(c"Очистить", c"Clear");
pub const DIFF_SEND_BATCH: UiText = UiText::new(c"Отправить агенту", c"Send batch to agent");
pub const DIFF_QUEUE_EMPTY: UiText = UiText::new(c"Очередь комментариев пуста", c"Comment queue is empty");
pub const DIFF_NEED_COMMENT: UiText = UiText::new(c"Введите текст комментария перед добавлением", c"Enter a comment before adding");
pub const DIFF_NO_CHANGES: UiText = UiText::new(c"Изменений нет", c"No changes");
pub const DIFF_LOADING: UiText = UiText::new(c"Загружаю diff…", c"Loading diff…");
pub const DIFF_TRUNCATED: UiText = UiText::new(c"Diff обрезан на лимите 2 МиБ", c"Diff truncated at 2 MiB safety limit");
pub const DIFF_SENT_OK: UiText = UiText::new(c"Пакет комментариев отправлен агенту", c"Review batch sent to agent");
pub const DIFF_HUNK_STAGED: UiText = UiText::new(c"Ханк добавлен в индекс", c"Hunk staged");
pub const DIFF_HUNK_REVERTED: UiText = UiText::new(c"Ханк откачен", c"Hunk reverted");
pub const DIFF_PALETTE_TITLE: UiText = UiText::new(c"Ревью Diff (Ctrl+Shift+B)", c"Review Diff (Ctrl+Shift+B)");
pub const DIFF_PALETTE_DETAIL: UiText = UiText::new(
    c"Локальный и SSH diff с инлайн-комментариями агенту и ханками",
    c"Local and SSH diff review with inline comments for agent and hunk actions",
);
pub const CP_SECTION_HEADER: UiText = UiText::new(c"ЧЕКПОЙНТЫ ХОДОВ", c"TURN CHECKPOINTS");
pub const CP_SAVE_BTN: UiText = UiText::new(c"Сохранить чекпойнт", c"Save Checkpoint");
pub const CP_SAVE_TIP: UiText = UiText::new(
    c"Сохранить снимок рабочего дерева и индекса без изменения HEAD",
    c"Snapshot working tree and staging index without mutating HEAD",
);
pub const CP_UNDO_BTN: UiText = UiText::new(c"Откатить", c"Undo");
pub const CP_UNDO_TIP: UiText = UiText::new(
    c"Откатить рабочее дерево к этому чекпойнту с резервным снимком",
    c"Restore working tree to this checkpoint with a pre-undo safety snapshot",
);
pub const CP_EMPTY: UiText = UiText::new(
    c"Нет чекпойнтов для этой панели",
    c"No checkpoints for this pane yet",
);
pub const CP_PALETTE_SAVE_TITLE: UiText = UiText::new(
    c"Git · Сохранить чекпойнт хода",
    c"Git · Save Turn Checkpoint",
);
pub const CP_PALETTE_SAVE_DETAIL: UiText = UiText::new(
    c"Сохранить снимок правок активной панели в refs/akhterm/checkpoints",
    c"Save working tree snapshot for active pane in refs/akhterm/checkpoints",
);
pub const CP_PALETTE_UNDO_TITLE: UiText = UiText::new(
    c"Git · Откатить последний ход агента",
    c"Git · Undo Last Agent Turn",
);
pub const CP_PALETTE_UNDO_DETAIL: UiText = UiText::new(
    c"Вернуть файлы активной панели к последнему сохранённому чекпойнту",
    c"Restore active pane files to the most recent saved checkpoint",
);
pub const CP_MSG_LOCAL_ONLY: UiText = UiText::new(
    c"Чекпойнты доступны только для локального Git-репозитория",
    c"Checkpoints are available for local Git repositories only",
);
pub const CP_MSG_CLEAN: UiText = UiText::new(
    c"Нет изменений для сохранения чекпойнта",
    c"No changes to save as a checkpoint",
);
pub const CP_MSG_DUPLICATE: UiText = UiText::new(
    c"Последний чекпойнт уже совпадает с текущими правками",
    c"Latest checkpoint already matches current changes",
);
pub const CP_MSG_NONE: UiText = UiText::new(
    c"Нет сохранённых чекпойнтов для отката",
    c"No saved checkpoints to restore",
);
pub const CP_MSG_SAVED: UiText = UiText::new(c"Сохранён чекпойнт", c"Saved checkpoint");
pub const CP_MSG_RESTORED: UiText = UiText::new(c"Откат выполнен к чекпойнту", c"Restored checkpoint");
pub const WT_SECTION_HDR: UiText = UiText::new(c"WORKTREES", c"WORKTREES");
pub const WT_OPEN_BTN: UiText = UiText::new(c"Открыть", c"Open");
pub const WT_REMOVE_TIP: UiText = UiText::new(c"Удалить чистый worktree", c"Remove clean worktree");
pub const WT_NEW_BTN: UiText = UiText::new(c"+ Новый worktree", c"+ New Worktree");
pub const WT_PRUNE_BTN: UiText = UiText::new(c"Убрать слитые", c"Prune Merged");
pub const WT_PRIMARY_TAG: UiText = UiText::new(c"основной", c"primary");
pub const WT_MERGED_TAG: UiText = UiText::new(c"слита", c"merged");
pub const WT_DIRTY_TAG: UiText = UiText::new(c"изменён", c"dirty");
pub const WT_DETACHED_TAG: UiText = UiText::new(c"detached", c"detached");
pub const WT_DLG_TITLE: UiText = UiText::new(c"Создать Git Worktree", c"New Git Worktree");
pub const WT_DLG_BRANCH_LBL: UiText = UiText::new(c"Ветка / slug", c"Branch / slug");
pub const WT_DLG_BRANCH_PH: UiText = UiText::new(c"например, feat-105", c"e.g. feat-105");
pub const WT_DLG_BASE_LBL: UiText = UiText::new(c"Базовая ветка / ref", c"Base ref");
pub const WT_DLG_AGENT_LBL: UiText = UiText::new(c"Запустить во вкладке", c"Launch in tab");
pub const WT_DLG_CREATE: UiText = UiText::new(c"Создать", c"Create");
pub const WT_DLG_CANCEL: UiText = UiText::new(c"Отмена", c"Cancel");
pub const WT_PALETTE_NEW_TITLE: UiText = UiText::new(c"Создать Git Worktree", c"New Git Worktree");
pub const WT_PALETTE_NEW_DETAIL: UiText = UiText::new(
    c"Создать новую ветку в соседнем worktree и открыть вкладку",
    c"Create a new branch in a sibling worktree and open a tab",
);
pub const WT_PALETTE_PRUNE_TITLE: UiText = UiText::new(c"Очистить слитые Worktree", c"Prune Merged Worktrees");
pub const WT_PALETTE_PRUNE_DETAIL: UiText = UiText::new(
    c"Удалить только чистые слитые worktree и выполнить git worktree prune",
    c"Remove only clean merged worktrees and run git worktree prune",
);
pub const WT_ERR_NOT_REPO: UiText = UiText::new(
    c"Текущий каталог не является Git-репозиторием",
    c"Current directory is not a Git repository",
);
pub const WT_ERR_EMPTY_SLUG: UiText = UiText::new(
    c"Введите имя ветки (slug)",
    c"Enter a branch slug",
);
pub const WT_ERR_BAD_SLUG: UiText = UiText::new(
    c"Недопустимое имя ветки: используйте латиницу, цифры, '.', '_' и '-' без '/', '..' и пробелов",
    c"Invalid branch slug: use ASCII letters, digits, '.', '_', '-' without '/', '..' or spaces",
);
pub const WT_ERR_BAD_BASE: UiText = UiText::new(
    c"Недопустимая базовая ссылка (base ref)",
    c"Invalid base ref",
);
pub const WT_ERR_PRIMARY_REMOVE: UiText = UiText::new(
    c"Нельзя удалить основной worktree",
    c"Cannot remove the primary worktree",
);
pub const WT_ERR_DIRTY_REMOVE: UiText = UiText::new(
    c"Нельзя удалить worktree с несохранёнными изменениями",
    c"Cannot remove a worktree with uncommitted changes",
);
pub const WT_ERR_NOT_FOUND: UiText = UiText::new(
    c"Указанный worktree не найден",
    c"Worktree not found",
);
pub const WT_ERR_PATH_EXISTS: UiText = UiText::new(
    c"Целевой каталог worktree уже существует",
    c"Target worktree directory already exists",
);
pub const WT_MSG_FOCUSED: UiText = UiText::new(c"Переключено на вкладку worktree:", c"Focused worktree tab:");
pub const WT_MSG_OPENED: UiText = UiText::new(c"Открыта вкладка worktree:", c"Opened worktree tab:");
pub const WT_MSG_CREATED: UiText = UiText::new(c"Создан worktree:", c"Created worktree:");
pub const WT_MSG_REMOVED: UiText = UiText::new(c"Удалён worktree:", c"Removed worktree:");
pub const WT_MSG_PRUNED_NONE: UiText = UiText::new(
    c"Нет чистых слитых worktree для удаления",
    c"No clean merged worktrees to prune",
);
pub const WT_MSG_PRUNED_DONE: UiText = UiText::new(
    c"Очищено слитых worktree:",
    c"Pruned merged worktrees:",
);
pub const WT_LOCAL_ONLY: UiText = UiText::new(
    c"Управление Git worktree доступно только для локального репозитория",
    c"Git worktree management is available only for local repositories",
);
pub const APP_NAME: &CStr = c"AkhTerm";
pub const PROFILES: &CStr = c"Профили / SSH ▾";
pub const SERVERS: &CStr = c"Серверы ▾";
pub const AGENTS: &CStr = c"Агенты ▾";
pub const HANDOFF: &CStr = c"Handoff ▾";
pub const MAINTAINABILITY: &CStr = c"MI ▾";
pub const MI_CALCULATING: UiText = UiText::new(c"MI · считаю… ▾", c"MI · calculating… ▾");
pub const MI_NO_FILES: UiText = UiText::new(c"MI · нет файлов ▾", c"MI · no files ▾");
pub const MI_SCAN_DETAIL: UiText = UiText::new(c"Считаю MI в каталоге", c"Calculating MI in");
pub const MI_NO_FILES_DETAIL: UiText = UiText::new(c"Нет подходящих исходных файлов в каталоге", c"No eligible source files in");
pub const MI_EMPTY: &CStr = c"Для расчёта MI открой локальный каталог с исходным кодом.";
pub const MI_EMPTY_MESSAGE: UiText = UiText::new(MI_EMPTY, c"Open a local source directory to calculate MI.");
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
pub const STATUS_ERROR: UiText = UiText::new(c"Ошибка", c"Error");
pub const NOTIFY_FOCUS_ACTION: UiText = UiText::new(c"Перейти", c"Focus");
pub const NOTIFY_ERROR_TITLE: UiText = UiText::new(c"AkhTerm · Ошибка", c"AkhTerm · Error");
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

pub const AGENT_ACTIONS: [(&CStr, &str); 6] = [
    (c"agy · Antigravity CLI", "agy\n"),
    (c"codex · OpenAI Codex CLI", "codex\n"),
    (c"claude · Claude Code", "claude\n"),
    (c"dsh · DSH Agent", "dsh\n"),
    (c"kimi · Kimi Code CLI", "kimi\n"),
    (c"opencode · OpenCode", "opencode\n"),
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
    use std::ffi::CStr;
    use crate::i18n::UiText;

    pub const PROFILES: UiText = UiText::new(super::PROFILES, c"Profiles / SSH ▾");
    pub fn profiles() -> &'static CStr { PROFILES.cstr() }
    pub const SERVERS: UiText = UiText::new(super::SERVERS, c"Servers ▾");
    pub fn servers() -> &'static CStr { SERVERS.cstr() }
    pub const AGENTS: UiText = UiText::new(super::AGENTS, c"Agents ▾");
    pub fn agents() -> &'static CStr { AGENTS.cstr() }
    pub const HANDOFF: UiText = UiText::new(super::HANDOFF, c"Handoff ▾");
    pub fn handoff() -> &'static CStr { HANDOFF.cstr() }
    pub const MAINTAINABILITY: UiText = UiText::new(super::MAINTAINABILITY, c"MI ▾");
    pub fn maintainability() -> &'static CStr { MAINTAINABILITY.cstr() }
    pub const THEME_TO_LIGHT_TIP: UiText = UiText::new(super::THEME_TO_LIGHT_TIP, c"Switch to light theme (Ctrl+Shift+L)");
    pub fn theme_to_light_tip() -> &'static CStr { THEME_TO_LIGHT_TIP.cstr() }
    pub const THEME_TO_DARK_TIP: UiText = UiText::new(super::THEME_TO_DARK_TIP, c"Switch to dark theme (Ctrl+Shift+L)");
    pub fn theme_to_dark_tip() -> &'static CStr { THEME_TO_DARK_TIP.cstr() }
    pub const RECONNECT: UiText = UiText::new(super::RECONNECT, c"Reconnect");
    pub fn reconnect() -> &'static CStr { RECONNECT.cstr() }
    pub const RECONNECT_TIP: UiText = UiText::new(super::RECONNECT_TIP, c"Reconnect active SSH or tmux tab (Ctrl+Shift+R)");
    pub fn reconnect_tip() -> &'static CStr { RECONNECT_TIP.cstr() }
    pub const GIT_MENU: UiText = UiText::new(super::GIT_MENU, c"Git ▾");
    pub fn git_menu() -> &'static CStr { GIT_MENU.cstr() }
    pub const GIT_MENU_TIP: UiText = UiText::new(super::GIT_MENU_TIP, c"Git modified files, quick diff, and worktree cleanup (Ctrl+Shift+G)");
    pub fn git_menu_tip() -> &'static CStr { GIT_MENU_TIP.cstr() }
    pub const HANDOFF_TIP: UiText = UiText::new(super::HANDOFF_TIP, c"Capture active tab log or forward an .md brief to a session");
    pub fn handoff_tip() -> &'static CStr { HANDOFF_TIP.cstr() }
    pub const TOKENS_TIP: UiText = UiText::new(super::TOKENS_TIP, c"Token usage, context window, and agent turn timer per tab");
    pub fn tokens_tip() -> &'static CStr { TOKENS_TIP.cstr() }
    pub const TOKENS_BTN: UiText = UiText::new(super::TOKENS, c"Tokens ▾");
    pub fn tokens_btn() -> &'static CStr { TOKENS_BTN.cstr() }
    pub const TOKENS_WORD: UiText = UiText::new(c"Токены", c"Tokens");
    pub fn tokens_word() -> &'static str { TOKENS_WORD.as_str() }
    pub const TOKENS_HEADER_TEXT: UiText = UiText::new(super::TOKENS_HEADER, c"TOKENS · CONTEXT & TURN DURATION");
    pub fn tokens_header() -> &'static CStr { TOKENS_HEADER_TEXT.cstr() }
    pub const TOKENS_EMPTY_TEXT: UiText = UiText::new(
        super::TOKENS_EMPTY,
        c"No token telemetry for open tabs.\nStart an agent (AGY, Claude Code, Codex CLI) or a session with context usage.",
    );
    pub fn tokens_empty() -> &'static CStr { TOKENS_EMPTY_TEXT.cstr() }
    pub const TOKENS_SUMMARY_PREFIX: UiText = UiText::new(c"Итого по окну", c"Window total");
    pub fn tokens_summary_prefix() -> &'static str { TOKENS_SUMMARY_PREFIX.as_str() }
    pub const TOKENS_WORKING: UiText = UiText::new(c"в работе", c"working");
    pub fn tokens_working() -> &'static str { TOKENS_WORKING.as_str() }
    pub const TOKENS_LAST_TURN: UiText = UiText::new(c"посл. ход", c"last turn");
    pub fn tokens_last_turn() -> &'static str { TOKENS_LAST_TURN.as_str() }
    pub const TOKENS_TUI_STATUS: UiText = UiText::new(c"статус TUI", c"TUI status");
    pub fn tokens_tui_status() -> &'static str { TOKENS_TUI_STATUS.as_str() }

    pub const TOGGLE_SIDEBAR_TIP_TEXT: UiText = UiText::new(super::TOGGLE_SIDEBAR_TIP, c"Show or hide sidebar");
    pub fn toggle_sidebar_tip() -> &'static CStr { TOGGLE_SIDEBAR_TIP_TEXT.cstr() }
    pub const SPLIT_VERT_TIP_TEXT: UiText = UiText::new(super::SPLIT_VERT_TIP, c"Split tab vertically (Ctrl+Shift+D)");
    pub fn split_vert_tip() -> &'static CStr { SPLIT_VERT_TIP_TEXT.cstr() }
    pub const SPLIT_HORIZ_TIP_TEXT: UiText = UiText::new(super::SPLIT_HORIZ_TIP, c"Split tab horizontally (Ctrl+Shift+E)");
    pub fn split_horiz_tip() -> &'static CStr { SPLIT_HORIZ_TIP_TEXT.cstr() }
    pub const INTERRUPT_TIP_TEXT: UiText = UiText::new(super::INTERRUPT_TIP, c"Interrupt active process (SIGINT)");
    pub fn interrupt_tip() -> &'static CStr { INTERRUPT_TIP_TEXT.cstr() }
    pub const PASTE_IMAGE_TEXT: UiText = UiText::new(super::PASTE_IMAGE, c"Screenshot");
    pub fn paste_image() -> &'static CStr { PASTE_IMAGE_TEXT.cstr() }
    pub const PASTE_IMAGE_TIP_TEXT: UiText = UiText::new(super::PASTE_IMAGE_TIP, c"Save clipboard image into active session and paste path");
    pub fn paste_image_tip() -> &'static CStr { PASTE_IMAGE_TIP_TEXT.cstr() }
    pub const ZOOM_IN_TIP_TEXT: UiText = UiText::new(super::ZOOM_IN_TIP, c"Increase terminal font size (Ctrl++)");
    pub fn zoom_in_tip() -> &'static CStr { ZOOM_IN_TIP_TEXT.cstr() }
    pub const ZOOM_RESET_TIP_TEXT: UiText = UiText::new(super::ZOOM_RESET_TIP, c"Reset terminal zoom (Ctrl+0)");
    pub fn zoom_reset_tip() -> &'static CStr { ZOOM_RESET_TIP_TEXT.cstr() }
    pub const ZOOM_OUT_TIP_TEXT: UiText = UiText::new(super::ZOOM_OUT_TIP, c"Decrease terminal font size (Ctrl+-)");
    pub fn zoom_out_tip() -> &'static CStr { ZOOM_OUT_TIP_TEXT.cstr() }
    pub const NEW_TAB_TIP_TEXT: UiText = UiText::new(super::NEW_TAB_TIP, c"New tab (Ctrl+Shift+T)");
    pub fn new_tab_tip() -> &'static CStr { NEW_TAB_TIP_TEXT.cstr() }
    pub const CLOSE_TAB_TIP_TEXT: UiText = UiText::new(super::CLOSE_TAB_TIP, c"Close tab");
    pub fn close_tab_tip() -> &'static CStr { CLOSE_TAB_TIP_TEXT.cstr() }
    pub const STATUS_APPROVAL_TEXT: UiText = UiText::new(super::STATUS_APPROVAL, c"Needs approval");
    pub fn status_approval() -> &'static CStr { STATUS_APPROVAL_TEXT.cstr() }
    pub const APPROVAL_TAB_TIP_TEXT: UiText = UiText::new(super::APPROVAL_TAB_TIP, c"Needs approval (Ctrl+Shift+U to jump)");
    pub fn approval_tab_tip() -> &'static CStr { APPROVAL_TAB_TIP_TEXT.cstr() }
    pub const MI_REFACTOR_TEXT: UiText = UiText::new(super::MI_REFACTOR, c"Ask agent to refactor");
    pub fn mi_refactor() -> &'static CStr { MI_REFACTOR_TEXT.cstr() }
    pub const MI_REFACTOR_TIP_TEXT: UiText = UiText::new(super::MI_REFACTOR_TIP, c"Send a prompt to the active terminal to refactor low-MI files");
    pub fn mi_refactor_tip() -> &'static CStr { MI_REFACTOR_TIP_TEXT.cstr() }
    pub const AGENTS_WORKTREE_HEADER_TEXT: UiText = UiText::new(super::AGENTS_WORKTREE_HEADER, c"NEW GIT WORKTREE · SEPARATE TAB");
    pub fn agents_worktree_header() -> &'static CStr { AGENTS_WORKTREE_HEADER_TEXT.cstr() }
    pub const AGENT_WORKTREE_TIP_TEXT: UiText = UiText::new(super::AGENT_WORKTREE_TIP, c"Create an isolated Git worktree and launch agent in a new tab");
    pub fn agent_worktree_tip() -> &'static CStr { AGENT_WORKTREE_TIP_TEXT.cstr() }
    pub const AGENTS_DISPATCH_BTN: UiText = UiText::new(c"Диспетчер задач (Ctrl+Shift+N)…", c"Task Dispatch (Ctrl+Shift+N)…");
    pub fn agents_dispatch_btn() -> &'static CStr { AGENTS_DISPATCH_BTN.cstr() }
    pub const SWARM_BTN: UiText = UiText::new(c"Рой ▾", c"Swarm ▾");
    pub fn swarm_btn() -> &'static CStr { SWARM_BTN.cstr() }
    pub const SWARM_TIP: UiText = UiText::new(c"Панель роя (Ctrl+Shift+S)", c"Swarm dashboard (Ctrl+Shift+S)");
    pub fn swarm_tip() -> &'static CStr { SWARM_TIP.cstr() }
    pub const BOOT_FOOTER: UiText = UiText::new(c"Запуск ядра VTE-2.91...", c"Starting VTE-2.91 core...");
    pub fn boot_footer() -> &'static CStr { BOOT_FOOTER.cstr() }

    #[cfg(test)]
    pub mod catalog {
        use crate::i18n::UiText;
        pub const APP_NAME: UiText = UiText::new(super::super::APP_NAME, c"AkhTerm");
        pub fn profiles_str() -> &'static str { super::PROFILES.as_str() }
        pub const MI_EMPTY: UiText = super::super::MI_EMPTY_MESSAGE;
        pub const MI_REFACTOR: UiText = UiText::new(super::super::MI_REFACTOR, c"Ask agent to refactor");
        pub const MI_REFACTOR_TIP: UiText = UiText::new(super::super::MI_REFACTOR_TIP, c"Send a prompt to the active terminal to refactor low-MI files");
        pub const TOGGLE_SIDEBAR: UiText = UiText::new(super::super::TOGGLE_SIDEBAR, c"◧");
        pub const TOGGLE_SIDEBAR_TIP: UiText = UiText::new(super::super::TOGGLE_SIDEBAR_TIP, c"Show or hide sidebar");
        pub const SPLIT_VERT_TIP: UiText = UiText::new(super::super::SPLIT_VERT_TIP, c"Split tab vertically (Ctrl+Shift+D)");
        pub const SPLIT_HORIZ_TIP: UiText = UiText::new(super::super::SPLIT_HORIZ_TIP, c"Split tab horizontally (Ctrl+Shift+E)");
        pub const INTERRUPT: UiText = UiText::new(super::super::INTERRUPT, c"SIGINT");
        pub const INTERRUPT_TIP: UiText = UiText::new(super::super::INTERRUPT_TIP, c"Interrupt active process (SIGINT)");
        pub const PASTE_IMAGE: UiText = UiText::new(super::super::PASTE_IMAGE, c"Screenshot");
        pub const PASTE_IMAGE_TIP: UiText = UiText::new(super::super::PASTE_IMAGE_TIP, c"Save clipboard image into active session and paste path");
        pub const ZOOM_IN_TIP: UiText = UiText::new(super::super::ZOOM_IN_TIP, c"Increase terminal font size (Ctrl++)");
        pub const ZOOM_RESET_TIP: UiText = UiText::new(super::super::ZOOM_RESET_TIP, c"Reset terminal zoom (Ctrl+0)");
        pub const ZOOM_OUT_TIP: UiText = UiText::new(super::super::ZOOM_OUT_TIP, c"Decrease terminal font size (Ctrl+-)");
        pub const NEW_TAB_TIP: UiText = UiText::new(super::super::NEW_TAB_TIP, c"New tab (Ctrl+Shift+T)");
        pub const CLOSE_TAB_TIP: UiText = UiText::new(super::super::CLOSE_TAB_TIP, c"Close tab");
        pub const PIN_TAB_TITLE: UiText = UiText::new(super::super::PIN_TAB_TITLE, c"Tab Title");
        pub const PIN_TAB_PLACEHOLDER: UiText = UiText::new(super::super::PIN_TAB_PLACEHOLDER, c"Pinned title (empty — auto)");
        pub const PIN_TAB_SAVE: UiText = UiText::new(super::super::PIN_TAB_SAVE, c"Pin");
        pub const PIN_TAB_RESET: UiText = UiText::new(super::super::PIN_TAB_RESET, c"Reset");
        pub const STATUS_APPROVAL: UiText = UiText::new(super::super::STATUS_APPROVAL, c"Needs approval");
        pub const APPROVAL_TAB_TIP: UiText = UiText::new(super::super::APPROVAL_TAB_TIP, c"Needs approval (Ctrl+Shift+U to jump)");
        pub const NOTIFY_APPROVAL_TITLE: UiText = UiText::new(super::super::NOTIFY_APPROVAL_TITLE, c"AkhTerm · Needs Approval");
        pub const NOTIFY_READY_TITLE: UiText = UiText::new(super::super::NOTIFY_READY_TITLE, c"AkhTerm · Ready");
        pub const AGENTS_WORKTREE_HEADER: UiText = UiText::new(super::super::AGENTS_WORKTREE_HEADER, c"NEW GIT WORKTREE · SEPARATE TAB");
        pub const AGENT_WORKTREE_TIP: UiText = UiText::new(super::super::AGENT_WORKTREE_TIP, c"Create an isolated Git worktree and launch agent in a new tab");
        pub const WORKTREE_LOCAL_ONLY: UiText = UiText::new(super::super::WORKTREE_LOCAL_ONLY, c"⚠ Git worktree is only available for a local repository tab");
        pub const PALETTE_TITLE: UiText = UiText::new(super::super::PALETTE_TITLE, c"Command Palette");
        pub const PALETTE_HINT: UiText = UiText::new(super::super::PALETTE_HINT, c"↑↓ select · Enter run · Esc close");
        pub const PALETTE_PLACEHOLDER: UiText = UiText::new(super::super::PALETTE_PLACEHOLDER, c"Tab, folder, tmux session, SSH, agent, or Handoff...");
        pub const PALETTE_EMPTY: UiText = UiText::new(super::super::PALETTE_EMPTY, c"Nothing found");
        pub const PALETTE_BADGE_TAB: UiText = UiText::new(super::super::PALETTE_BADGE_TAB, c"Tab");
        pub const PALETTE_BADGE_PROFILE: UiText = UiText::new(super::super::PALETTE_BADGE_PROFILE, c"Profile");
        pub const PALETTE_BADGE_AGENT: UiText = UiText::new(super::super::PALETTE_BADGE_AGENT, c"Agent");
        pub const PALETTE_BADGE_THEME: UiText = UiText::new(super::super::PALETTE_BADGE_THEME, c"Theme");
        pub const PALETTE_BADGE_SEARCH: UiText = UiText::new(super::super::PALETTE_BADGE_SEARCH, c"Search");
        pub const PALETTE_BADGE_SETTINGS: UiText = UiText::new(c"Настройки", c"Settings");
        pub const PALETTE_SEARCH_TITLE: UiText = UiText::new(super::super::PALETTE_SEARCH_TITLE, c"Search Output");
        pub const PALETTE_SEARCH_DETAIL: UiText = UiText::new(super::super::PALETTE_SEARCH_DETAIL, c"Find text or regex in active terminal · Ctrl+Shift+F");
        pub const PALETTE_SETTINGS_TITLE: UiText = UiText::new(c"Настройки AkhTerm", c"AkhTerm Settings");
        pub const PALETTE_ABOUT_TITLE: UiText = UiText::new(c"О программе AkhTerm", c"About AkhTerm");
        pub const SEARCH_PLACEHOLDER: UiText = UiText::new(super::super::SEARCH_PLACEHOLDER, c"Search terminal output...");
        pub const SEARCH_PREV_TIP: UiText = UiText::new(super::super::SEARCH_PREV_TIP, c"Previous match (Shift+Enter)");
        pub const SEARCH_NEXT_TIP: UiText = UiText::new(super::super::SEARCH_NEXT_TIP, c"Next match (Enter)");
        pub const SEARCH_CASE_TIP: UiText = UiText::new(super::super::SEARCH_CASE_TIP, c"Match case");
        pub const SEARCH_REGEX_TIP: UiText = UiText::new(super::super::SEARCH_REGEX_TIP, c"Regular expression (PCRE2)");
        pub const SEARCH_CLOSE_TIP: UiText = UiText::new(super::super::SEARCH_CLOSE_TIP, c"Close search (Esc)");
        pub const SEARCH_FOUND: UiText = UiText::new(super::super::SEARCH_FOUND, c"Found");
        pub const SEARCH_NOT_FOUND: UiText = UiText::new(super::super::SEARCH_NOT_FOUND, c"No matches");
        pub const SEARCH_REGEX_ERR: UiText = UiText::new(super::super::SEARCH_REGEX_ERR, c"Regex error");
        pub const GIT_LOCAL_ONLY: UiText = UiText::new(super::super::GIT_LOCAL_ONLY, c"⚠ Git menu is only available for a local tab");
        pub const GIT_HEADER: UiText = UiText::new(super::super::GIT_HEADER, c"GIT · LOCAL TAB");
        pub const GIT_NOT_REPO: UiText = UiText::new(super::super::GIT_NOT_REPO, c"Current directory is not a Git repository");
        pub const GIT_FILES_HEADER: UiText = UiText::new(super::super::GIT_FILES_HEADER, c"MODIFIED & UNTRACKED FILES (CLICK TO DIFF / OPEN)");
        pub const GIT_CLEAN: UiText = UiText::new(super::super::GIT_CLEAN, c"Working tree clean — no uncommitted changes");
        pub const GIT_LOADING: UiText = UiText::new(super::super::GIT_LOADING, c"Refreshing Git status in background…");
        pub const GIT_REMOVE_WT_BTN: UiText = UiText::new(super::super::GIT_REMOVE_WT_BTN, c"Remove clean worktree");
        pub const GIT_REMOVE_WT_TIP: UiText = UiText::new(super::super::GIT_REMOVE_WT_TIP, c"Verify no changes remain and remove isolated Git worktree");
        pub const HANDOFF_CAPTURE_BTN: UiText = UiText::new(super::super::HANDOFF_CAPTURE_BTN, c"Save active tab output (.md)");
        pub const HANDOFF_CAPTURE_TIP: UiText = UiText::new(super::super::HANDOFF_CAPTURE_TIP, c"Save recent terminal lines to a Markdown file (0600)");
        pub const HANDOFF_FORWARD_HEADER: UiText = UiText::new(super::super::HANDOFF_FORWARD_HEADER, c"FORWARD ACTIVE TAB OUTPUT TO SESSION");
        pub const HANDOFF_BRIEFS_HEADER: UiText = UiText::new(super::super::HANDOFF_BRIEFS_HEADER, c"BRIEFS & CAPTURES (.MD) — INTO ACTIVE TAB");
        pub const HANDOFF_EMPTY: UiText = UiText::new(super::super::HANDOFF_EMPTY, c"No saved .md files — capture output above");
        pub const DROP_SCP_START: UiText = UiText::new(super::super::DROP_SCP_START, c"⬆ Uploading via SCP to");
        pub const DROP_SCP_DONE: UiText = UiText::new(super::super::DROP_SCP_DONE, c"📎 Uploaded via SCP to");
        pub const DROP_SCP_ERROR: UiText = UiText::new(super::super::DROP_SCP_ERROR, c"⚠ SCP upload error to");
        pub const DROP_SCP_NO_FILE: UiText = UiText::new(super::super::DROP_SCP_NO_FILE, c"⚠ SCP error: local file unavailable");
        pub const TOKENS: UiText = UiText::new(super::super::TOKENS, c"Tokens ▾");
        pub const TOKENS_HEADER: UiText = UiText::new(super::super::TOKENS_HEADER, c"TOKENS · CONTEXT & TURN DURATION");
        pub const TOKENS_EMPTY: UiText = UiText::new(super::super::TOKENS_EMPTY, c"No token telemetry for open tabs.\nStart an agent (AGY, Claude Code, Codex CLI) or a session with context usage.");
    }
    #[cfg(test)]
    pub use catalog::*;

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
    #[cfg(test)]
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
    #[cfg(test)]
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
    pub const SETTINGS_HOOKS_TITLE: UiText = UiText::new(c"Хуки агентов", c"Agent Hooks");
    pub fn settings_hooks_title() -> &'static CStr { SETTINGS_HOOKS_TITLE.cstr() }
    pub const SETTINGS_HOOKS_SUBTITLE: UiText = UiText::new(
        c"Живые статусы и уведомления для Claude Code, Codex CLI и AGY",
        c"Live status and notifications for Claude Code, Codex CLI, and AGY",
    );
    pub fn settings_hooks_subtitle() -> &'static CStr { SETTINGS_HOOKS_SUBTITLE.cstr() }
    pub const SETTINGS_HOOKS_INSTALL: UiText = UiText::new(c"Установить", c"Install");
    pub fn settings_hooks_install() -> &'static CStr { SETTINGS_HOOKS_INSTALL.cstr() }
    pub const SETTINGS_HOOKS_UPDATE: UiText = UiText::new(c"Обновить", c"Update");
    pub fn settings_hooks_update() -> &'static CStr { SETTINGS_HOOKS_UPDATE.cstr() }
    pub const SETTINGS_HOOKS_REMOVE: UiText = UiText::new(c"Удалить", c"Remove");
    pub fn settings_hooks_remove() -> &'static CStr { SETTINGS_HOOKS_REMOVE.cstr() }
    pub const SETTINGS_HOOKS_INSTALL_ALL: UiText = UiText::new(c"Установить все", c"Install All");
    pub fn settings_hooks_install_all() -> &'static CStr { SETTINGS_HOOKS_INSTALL_ALL.cstr() }
    pub const SETTINGS_HOOKS_INSTALLED: UiText = UiText::new(c"Установлено", c"Installed");
    pub fn settings_hooks_installed() -> &'static CStr { SETTINGS_HOOKS_INSTALLED.cstr() }
    pub const SETTINGS_HOOKS_NOT_INSTALLED: UiText = UiText::new(c"Не установлено", c"Not installed");
    pub fn settings_hooks_not_installed() -> &'static CStr { SETTINGS_HOOKS_NOT_INSTALLED.cstr() }
    pub const SETTINGS_HOOKS_OUTDATED: UiText = UiText::new(c"Устарело", c"Outdated");
    pub fn settings_hooks_outdated() -> &'static CStr { SETTINGS_HOOKS_OUTDATED.cstr() }
    pub const PALETTE_HOOKS_TITLE: UiText = UiText::new(c"Установить хуки агентов", c"Install Agent Hooks");
    pub const PALETTE_HOOKS_DETAIL: UiText = UiText::new(
        c"Официальные хуки Claude Code, Codex CLI и AGY в один клик",
        c"One-click official hooks for Claude Code, Codex CLI, and AGY",
    );

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
    #[cfg(test)]
    pub fn about_copy_btn_str() -> &'static str { ABOUT_COPY_BTN.as_str() }
    pub const ABOUT_COPIED_BTN: UiText = UiText::new(c"Скопировано ✓", c"Copied ✓");
    pub fn about_copied_btn() -> &'static CStr { ABOUT_COPIED_BTN.cstr() }
    pub const ABOUT_CLOSE_BTN: UiText = UiText::new(c"Закрыть", c"Close");
    pub fn about_close_btn() -> &'static CStr { ABOUT_CLOSE_BTN.cstr() }
    pub const ABOUT_CHECK_UPDATES_BTN: UiText = UiText::new(c"Проверить обновления", c"Check for updates");
    pub fn about_check_updates_btn() -> &'static CStr { ABOUT_CHECK_UPDATES_BTN.cstr() }

    pub const UPDATE_ICON_TEXT: &CStr = c">_";
    pub const UPDATE_BADGE: UiText = UiText::new(c"ОБНОВЛЕНИЕ", c"UPDATE");
    pub fn update_badge() -> &'static CStr { UPDATE_BADGE.cstr() }
    pub const UPDATE_AUTHOR: UiText = UiText::new(c"by Pavel Akhtyamov", c"by Pavel Akhtyamov");
    pub fn update_author() -> &'static CStr { UPDATE_AUTHOR.cstr() }
    pub const UPDATE_CLOSE_BTN: &CStr = c"×";
    pub const UPDATE_CLOSE_TIP: UiText = UiText::new(c"Скрыть уведомление об обновлении", c"Dismiss update notice");
    pub fn update_close_tip() -> &'static CStr { UPDATE_CLOSE_TIP.cstr() }
    pub const UPDATE_STATUS_AVAILABLE: UiText = UiText::new(
        c"Доступна новая версия AkhTerm",
        c"A new version of AkhTerm is ready to install",
    );
    pub fn update_status_available() -> &'static CStr { UPDATE_STATUS_AVAILABLE.cstr() }
    pub const UPDATE_STATUS_DOWNLOADING: UiText = UiText::new(
        c"Скачивание и проверка SHA-256...",
        c"Downloading and verifying update...",
    );
    pub fn update_status_downloading() -> &'static CStr { UPDATE_STATUS_DOWNLOADING.cstr() }
    pub const UPDATE_STATUS_INSTALLING: UiText = UiText::new(
        c"Установка обновления...",
        c"Installing update...",
    );
    pub fn update_status_installing() -> &'static CStr { UPDATE_STATUS_INSTALLING.cstr() }
    pub const UPDATE_STATUS_READY: UiText = UiText::new(
        c"✓ Обновление установлено. Перезапустите для применения.",
        c"✓ Update installed. Restart AkhTerm to apply.",
    );
    pub fn update_status_ready() -> &'static CStr { UPDATE_STATUS_READY.cstr() }
    pub const UPDATE_WHATS_NEW: UiText = UiText::new(c"Что нового", c"What's new");
    pub fn update_whats_new() -> &'static CStr { UPDATE_WHATS_NEW.cstr() }
    pub const UPDATE_DISMISS: UiText = UiText::new(c"Позже", c"DISMISS");
    pub fn update_dismiss() -> &'static CStr { UPDATE_DISMISS.cstr() }
    pub const UPDATE_BTN_UPDATE: UiText = UiText::new(c"Обновить", c"UPDATE");
    pub fn update_btn_update() -> &'static CStr { UPDATE_BTN_UPDATE.cstr() }
    pub const UPDATE_BTN_RESTART: UiText = UiText::new(c"Перезапустить", c"RESTART");
    pub fn update_btn_restart() -> &'static CStr { UPDATE_BTN_RESTART.cstr() }
    pub const UPDATE_BTN_RETRY: UiText = UiText::new(c"Повторить", c"RETRY");
    pub fn update_btn_retry() -> &'static CStr { UPDATE_BTN_RETRY.cstr() }

    pub const SIDEBAR_TITLE: UiText = UiText::new(c"ПРОЕКТЫ И СЕССИИ", c"PROJECTS & SESSIONS");
    pub fn sidebar_title() -> &'static CStr { SIDEBAR_TITLE.cstr() }
    pub const SIDEBAR_SEARCH_PLACEHOLDER: UiText = UiText::new(c"Поиск вкладки, каталога, tmux...", c"Search tab, folder, tmux...");
    pub fn sidebar_search_placeholder() -> &'static CStr { SIDEBAR_SEARCH_PLACEHOLDER.cstr() }
    pub const SIDEBAR_GROUP_LOCAL: UiText = UiText::new(c"Локальные", c"Local");
    pub fn sidebar_group_local() -> &'static CStr { SIDEBAR_GROUP_LOCAL.cstr() }
    pub const SIDEBAR_GROUP_SSH: UiText = UiText::new(c"SSH", c"SSH");
    pub fn sidebar_group_ssh() -> &'static CStr { SIDEBAR_GROUP_SSH.cstr() }
    pub const SIDEBAR_GROUP_MONITOR: UiText = UiText::new(c"Мониторинг", c"Monitoring");
    pub fn sidebar_group_monitor() -> &'static CStr { SIDEBAR_GROUP_MONITOR.cstr() }
    pub const SIDEBAR_EMPTY_TABS: UiText = UiText::new(c"Нет открытых вкладок", c"No open tabs");
    pub fn sidebar_empty_tabs() -> &'static CStr { SIDEBAR_EMPTY_TABS.cstr() }
    pub const SIDEBAR_EMPTY_SEARCH: UiText = UiText::new(c"Ничего не найдено", c"Nothing found");
    pub fn sidebar_empty_search() -> &'static CStr { SIDEBAR_EMPTY_SEARCH.cstr() }
    pub const SIDEBAR_TMUX_PREFIX: UiText = UiText::new(c"tmux на сервере:", c"tmux on server:");
    pub fn sidebar_tmux_prefix() -> &'static str { SIDEBAR_TMUX_PREFIX.as_str() }
    pub const SIDEBAR_FOLDER_NEW_TAB_TIP: UiText = UiText::new(c"Новая локальная вкладка в этой папке", c"New local tab in this folder");
    pub fn sidebar_folder_new_tab_tip() -> &'static CStr { SIDEBAR_FOLDER_NEW_TAB_TIP.cstr() }
    pub const SIDEBAR_TMUX_INITIAL: UiText = UiText::new(c"Сессии tmux", c"tmux sessions");
    pub fn sidebar_tmux_initial() -> &'static CStr { SIDEBAR_TMUX_INITIAL.cstr() }
    pub const SIDEBAR_STATUS_OFFLINE: UiText = UiText::new(c"Нет связи", c"Disconnected");
    pub fn sidebar_status_offline() -> &'static CStr { SIDEBAR_STATUS_OFFLINE.cstr() }
    pub const SIDEBAR_STATUS_ENDED: UiText = UiText::new(c"Завершена", c"Ended");
    pub fn sidebar_status_ended() -> &'static CStr { SIDEBAR_STATUS_ENDED.cstr() }
    pub const SIDEBAR_STATUS_READY: UiText = UiText::new(c"Готово", c"Ready");
    pub fn sidebar_status_ready() -> &'static CStr { SIDEBAR_STATUS_READY.cstr() }
    pub const SIDEBAR_STATUS_ONLINE: UiText = UiText::new(c"Онлайн", c"Online");
    pub fn sidebar_status_online() -> &'static CStr { SIDEBAR_STATUS_ONLINE.cstr() }
    pub const SIDEBAR_STATUS_WORKING: UiText = UiText::new(c"В работе", c"Working");
    pub fn sidebar_status_working() -> &'static CStr { SIDEBAR_STATUS_WORKING.cstr() }

    pub const PORTS_BTN: UiText = UiText::new(c"Порты ▾", c"Ports ▾");
    pub fn ports_btn() -> &'static CStr { PORTS_BTN.cstr() }
    pub const PORTS_BTN_WORD: UiText = UiText::new(c"Порты", c"Ports");
    pub const PORTS_TIP: UiText = UiText::new(c"Открытые локальные и SSH-порты, управляемые туннели", c"Listening local and SSH ports, managed tunnels");
    pub fn ports_tip() -> &'static CStr { PORTS_TIP.cstr() }
    pub const PORTS_HEADER: UiText = UiText::new(c"ПОРТЫ И SSH-ТУННЕЛИ", c"PORTS & SSH TUNNELS");
    pub const PORTS_REFRESH_TIP: UiText = UiText::new(c"Обновить список портов", c"Refresh listening ports");
    pub const PORTS_SECTION_TUNNELS: UiText = UiText::new(c"АКТИВНЫЕ SSH-ТУННЕЛИ", c"ACTIVE SSH TUNNELS");
    pub const PORTS_SECTION_LOCAL: UiText = UiText::new(c"ЛОКАЛЬНЫЕ ПОРТЫ ВКЛАДКИ", c"LOCAL TAB PORTS");
    pub const PORTS_SECTION_REMOTE: UiText = UiText::new(c"УДАЛЁННЫЕ ПОРТЫ", c"REMOTE PORTS");
    pub const PORTS_OPEN: UiText = UiText::new(c"Открыть", c"Open");
    pub const PORTS_COPY: UiText = UiText::new(c"Копировать", c"Copy");
    pub const PORTS_FORWARD_OPEN: UiText = UiText::new(c"Пробросить и открыть", c"Forward & Open");
    pub const PORTS_STOP_TIP: UiText = UiText::new(c"Остановить SSH-туннель", c"Stop SSH tunnel");
    pub const PORTS_TO_WORD: UiText = UiText::new(c"на", c"to");
    pub const PORTS_EMPTY_LOCAL: UiText = UiText::new(c"На этой вкладке нет открытых локальных портов.", c"No listening local ports detected in this tab.");
    pub const PORTS_EMPTY_REMOTE: UiText = UiText::new(c"Открытые порты на удалённом хосте не найдены. Запустите сервер или обновите список.", c"No listening remote ports detected on this host. Start a server or refresh.");
    pub const PORTS_COPIED: UiText = UiText::new(c"Скопирован URL:", c"Copied URL:");
    pub const PORTS_TUNNEL_READY: UiText = UiText::new(c"SSH-туннель:", c"SSH tunnel:");
    pub const PORTS_TUNNEL_STOPPED: UiText = UiText::new(c"SSH-туннель остановлен:", c"SSH tunnel stopped:");
    pub const PORTS_TUNNEL_FAILED: UiText = UiText::new(c"Не удалось открыть SSH-туннель:", c"Failed to open SSH tunnel:");
    pub const PORTS_TUNNEL_DISCONNECTED: UiText = UiText::new(c"Разорван", c"Disconnected");
    pub const PORTS_RECONNECT: UiText = UiText::new(c"Переподключить", c"Reconnect");
    pub const PORTS_PIN: UiText = UiText::new(c"Закрепить", c"Pin");
    pub const PORTS_UNPIN: UiText = UiText::new(c"Открепить", c"Unpin");
    pub const PORTS_PIN_TIP: UiText = UiText::new(c"Закрепить туннель для быстрого восстановления", c"Pin tunnel for quick restore");
    pub const PORTS_UNPIN_TIP: UiText = UiText::new(c"Убрать туннель из закреплённых", c"Remove tunnel from pinned list");
    pub const PORTS_RESTORE_PINNED: UiText = UiText::new(c"Восстановить закреплённые", c"Restore Pinned");
    pub const PORTS_RESTORE_PINNED_TIP: UiText = UiText::new(c"Восстановить все закреплённые туннели этого хоста", c"Restore all pinned tunnels for this host");
    pub const PORTS_SECTION_PINNED: UiText = UiText::new(c"ЗАКРЕПЛЁННЫЕ ТУННЕЛИ", c"PINNED TUNNELS");
    pub const PORTS_TUNNEL_RECONNECTED: UiText = UiText::new(c"SSH-туннель переподключён:", c"SSH tunnel reconnected:");
    pub const PORTS_TUNNEL_PINNED: UiText = UiText::new(c"Туннель закреплён:", c"Tunnel pinned:");
    pub const PORTS_TUNNEL_UNPINNED: UiText = UiText::new(c"Туннель откреплён:", c"Tunnel unpinned:");
    pub const PORTS_TUNNELS_RESTORED: UiText = UiText::new(c"Восстановлено туннелей:", c"Restored tunnels:");
    pub const PORTS_TUNNELS_NONE_TO_RESTORE: UiText = UiText::new(c"Все закреплённые туннели уже активны", c"All pinned tunnels are already active");

    pub const SYS_MON_TIP: UiText = UiText::new(c"Системный монитор: CPU, RAM и питание", c"System monitor: CPU, RAM and power");
    pub fn sys_mon_tip() -> &'static CStr { SYS_MON_TIP.cstr() }
    pub const SYS_MON_TITLE: UiText = UiText::new(c"Системный монитор", c"System Monitor");
    pub fn sys_mon_title() -> &'static CStr { SYS_MON_TITLE.cstr() }
    pub const SYS_MON_CPU: UiText = UiText::new(c"CPU", c"CPU");
    pub fn sys_mon_cpu() -> &'static CStr { SYS_MON_CPU.cstr() }
    pub const SYS_MON_CPU_DETAIL: UiText = UiText::new(c"Профиль", c"Breakdown");
    pub fn sys_mon_cpu_detail() -> &'static CStr { SYS_MON_CPU_DETAIL.cstr() }
    pub const SYS_MON_LOAD: UiText = UiText::new(c"Нагрузка (1/5/15м)", c"Load avg (1/5/15m)");
    pub fn sys_mon_load() -> &'static CStr { SYS_MON_LOAD.cstr() }
    pub const SYS_MON_RAM: UiText = UiText::new(c"RAM", c"RAM");
    pub fn sys_mon_ram() -> &'static CStr { SYS_MON_RAM.cstr() }
    pub const SYS_MON_SWAP: UiText = UiText::new(c"Swap", c"Swap");
    pub fn sys_mon_swap() -> &'static CStr { SYS_MON_SWAP.cstr() }
    pub const SYS_MON_SWAP_OFF: UiText = UiText::new(c"Отключён", c"Off");
    pub fn sys_mon_swap_off() -> &'static str { SYS_MON_SWAP_OFF.as_str() }
    pub const SYS_MON_TAB_RSS: UiText = UiText::new(c"Память вкладки", c"Tab RSS");
    pub fn sys_mon_tab_rss() -> &'static CStr { SYS_MON_TAB_RSS.cstr() }
    pub const SYS_MON_BATTERY: UiText = UiText::new(c"Батарея", c"Battery");
    pub fn sys_mon_battery() -> &'static CStr { SYS_MON_BATTERY.cstr() }
    pub const SYS_MON_POWER: UiText = UiText::new(c"Мощность", c"Power draw");
    pub fn sys_mon_power() -> &'static CStr { SYS_MON_POWER.cstr() }
    pub const SYS_MON_ETA_LEFT: UiText = UiText::new(c"Осталось", c"Remaining");
    pub fn sys_mon_eta_left() -> &'static CStr { SYS_MON_ETA_LEFT.cstr() }
    pub const SYS_MON_ETA_FULL: UiText = UiText::new(c"До заряда", c"To full");
    pub fn sys_mon_eta_full() -> &'static CStr { SYS_MON_ETA_FULL.cstr() }
    pub const SYS_MON_HEALTH: UiText = UiText::new(c"Здоровье", c"Health");
    pub fn sys_mon_health() -> &'static CStr { SYS_MON_HEALTH.cstr() }
    pub const SYS_MON_AC_ONLY: UiText = UiText::new(c"Сеть / нет батареи", c"AC / No battery");
    pub fn sys_mon_ac_only() -> &'static str { SYS_MON_AC_ONLY.as_str() }
    pub const SYS_MON_BAT_DISCHARGING: UiText = UiText::new(c"Разрядка", c"Discharging");
    pub fn sys_mon_bat_discharging() -> &'static str { SYS_MON_BAT_DISCHARGING.as_str() }
    pub const SYS_MON_BAT_CHARGING: UiText = UiText::new(c"Зарядка", c"Charging");
    pub fn sys_mon_bat_charging() -> &'static str { SYS_MON_BAT_CHARGING.as_str() }
    pub const SYS_MON_BAT_FULL: UiText = UiText::new(c"Заряжена", c"Full");
    pub fn sys_mon_bat_full() -> &'static str { SYS_MON_BAT_FULL.as_str() }
    pub const SYS_MON_BAT_NOT_CHARGING: UiText = UiText::new(c"От сети", c"Not charging");
    pub fn sys_mon_bat_not_charging() -> &'static str { SYS_MON_BAT_NOT_CHARGING.as_str() }
    pub const SYS_MON_BAT_UNKNOWN: UiText = UiText::new(c"Нет данных", c"Unknown");
    pub fn sys_mon_bat_unknown() -> &'static str { SYS_MON_BAT_UNKNOWN.as_str() }

    pub const CTX_PREV_COMMAND: UiText = UiText::new(c"Предыдущая команда", c"Previous Command");
    pub fn ctx_prev_command() -> &'static CStr { CTX_PREV_COMMAND.cstr() }
    pub const CTX_NEXT_COMMAND: UiText = UiText::new(c"Следующая команда", c"Next Command");
    pub fn ctx_next_command() -> &'static CStr { CTX_NEXT_COMMAND.cstr() }
    pub const CTX_COPY_CMD_OUTPUT: UiText = UiText::new(c"Скопировать вывод команды", c"Copy Command Output");
    pub fn ctx_copy_cmd_output() -> &'static CStr { CTX_COPY_CMD_OUTPUT.cstr() }
    pub const CTX_SEND_FAILED_AGENT: UiText = UiText::new(c"Отправить ошибку в агента", c"Send Failed Output to Agent");
    pub fn ctx_send_failed_agent() -> &'static CStr { CTX_SEND_FAILED_AGENT.cstr() }
    pub const CMD_PREV_DETAIL: UiText = UiText::new(
        c"Прокрутить к предыдущей команде OSC 133 (Ctrl+Shift+Up)",
        c"Scroll to previous OSC 133 command prompt (Ctrl+Shift+Up)",
    );
    pub const CMD_NEXT_DETAIL: UiText = UiText::new(
        c"Прокрутить к следующей команде OSC 133 (Ctrl+Shift+Down)",
        c"Scroll to next OSC 133 command prompt (Ctrl+Shift+Down)",
    );
    pub const CMD_COPY_OUTPUT_DETAIL: UiText = UiText::new(
        c"Скопировать вывод последней команды OSC 133 в буфер",
        c"Copy last OSC 133 command output to clipboard",
    );
    pub const CMD_SEND_FAILED_DETAIL: UiText = UiText::new(
        c"Отправить вывод последней команды с ошибкой в агента",
        c"Send last failed command output (exit != 0) to agent pane",
    );
    pub const CMD_NO_MARKS: UiText = UiText::new(
        c"Нет отметок команд OSC 133 в этой панели",
        c"No OSC 133 command marks in this pane",
    );
    pub const CMD_NO_PREV: UiText = UiText::new(
        c"Выше нет предыдущей команды",
        c"No earlier command prompt",
    );
    pub const CMD_NO_NEXT: UiText = UiText::new(
        c"Ниже нет следующей команды",
        c"No later command prompt",
    );
    pub const CMD_NO_FAILED: UiText = UiText::new(
        c"Нет завершённых команд с ошибкой в этой панели",
        c"No failed command (exit != 0) recorded in this pane",
    );
    pub const CMD_OUTPUT_COPIED: UiText = UiText::new(
        c"Вывод команды скопирован в буфер",
        c"Command output copied to clipboard",
    );
    pub const CMD_SENT_TO_AGENT: UiText = UiText::new(
        c"Вывод ошибки отправлен в агента",
        c"Failed output sent to agent",
    );
    pub const CMD_COPIED_NO_AGENT: UiText = UiText::new(
        c"Агент не найден: Markdown-блок ошибки скопирован в буфер",
        c"No active agent pane: failed output Markdown copied to clipboard",
    );
    pub const CTX_HANDOFF_HEADER: UiText = UiText::new(c"ПЕРЕДАТЬ АГЕНТУ", c"HAND OFF TO AGENT");
    pub const CTX_HANDOFF_TIP: UiText = UiText::new(
        c"Собрать Handoff Pack, скопировать в буфер и открыть свежего агента в этом каталоге",
        c"Build Handoff Pack, copy to clipboard, and open a fresh agent in this folder",
    );
    pub const CTX_HEALTH_WARN_PREFIX: UiText = UiText::new(c"Контекст", c"Context");
    pub const CTX_HEALTH_WARN_HINT: UiText = UiText::new(
        c"окно заполняется, подготовьте передачу свежему агенту",
        c"window filling up, consider handing off to a fresh agent",
    );
    pub const CTX_HEALTH_CRIT_PREFIX: UiText = UiText::new(c"Критический контекст", c"Critical context");
    pub const CTX_HEALTH_CRIT_HINT: UiText = UiText::new(
        c"высокий риск деградации, передайте задачу свежему агенту",
        c"high context rot risk, hand off to a fresh agent now",
    );
    pub const CTX_CACHE_DROP_LABEL: UiText = UiText::new(c"Падение кэша промпта", c"Prompt cache drop");
    pub const CTX_CACHE_DROP_HINT: UiText = UiText::new(
        c"кэш префикса сброшен",
        c"prefix cache invalidated",
    );
    pub const CTX_HANDOFF_LOCAL_ONLY: UiText = UiText::new(
        c"Передача агенту доступна только для локальной вкладки",
        c"Agent handoff is available for local tabs only",
    );
    pub const CTX_HANDOFF_PALETTE_PREFIX: UiText = UiText::new(c"Передать агенту", c"Hand Off to Agent");
    pub const CTX_HANDOFF_PALETTE_DETAIL: UiText = UiText::new(
        c"Собрать Handoff Pack (git diff, вывод терминала, бриф) и открыть новую вкладку",
        c"Build Handoff Pack (git diff, terminal tail, brief) and open a new tab",
    );

    pub const REMOTE_BTN: UiText = UiText::new(c"Моб. ▾", c"Remote ▾");
    pub fn remote_btn() -> &'static CStr { REMOTE_BTN.cstr() }
    pub const REMOTE_BTN_WORD: UiText = UiText::new(c"Моб.", c"Remote");
    pub fn remote_btn_word() -> &'static CStr { REMOTE_BTN_WORD.cstr() }
    pub const REMOTE_TIP: UiText = UiText::new(
        c"Мобильный доступ: QR-код, привязанные устройства и отключение",
        c"Mobile Remote: QR pairing code, paired devices, and stop server",
    );
    pub fn remote_tip() -> &'static CStr { REMOTE_TIP.cstr() }
    pub const REMOTE_HEADER: UiText = UiText::new(
        c"МОБИЛЬНЫЙ ДОСТУП · QR-ПЭЙРИНГ",
        c"MOBILE REMOTE · QR PAIRING",
    );
    pub fn remote_header() -> &'static CStr { REMOTE_HEADER.cstr() }
    pub const REMOTE_LIVE_CLIENTS: UiText = UiText::new(c"онлайн", c"connected");
    pub fn remote_live_clients() -> &'static CStr { REMOTE_LIVE_CLIENTS.cstr() }
    pub const REMOTE_ROTATE_QR: UiText = UiText::new(c"Обновить QR", c"New QR");
    pub fn remote_rotate_qr() -> &'static CStr { REMOTE_ROTATE_QR.cstr() }
    pub const REMOTE_COPY_URL: UiText = UiText::new(c"Копировать ссылку", c"Copy URL");
    pub fn remote_copy_url() -> &'static CStr { REMOTE_COPY_URL.cstr() }
    pub const REMOTE_URL_COPIED: UiText = UiText::new(
        c"Ссылка пэйринга скопирована в буфер",
        c"Pairing URL copied to clipboard",
    );
    pub fn remote_url_copied() -> &'static CStr { REMOTE_URL_COPIED.cstr() }
    pub const REMOTE_STOP_BTN: UiText = UiText::new(c"Выключить", c"Stop Remote");
    pub fn remote_stop_btn() -> &'static CStr { REMOTE_STOP_BTN.cstr() }
    pub const REMOTE_DEVICES_HEADER: UiText = UiText::new(
        c"ПРИВЯЗАННЫЕ УСТРОЙСТВА",
        c"PAIRED DEVICES",
    );
    pub fn remote_devices_header() -> &'static CStr { REMOTE_DEVICES_HEADER.cstr() }
    pub const REMOTE_NO_DEVICES: UiText = UiText::new(
        c"Устройства ещё не привязаны — отсканируйте QR с телефона",
        c"No paired devices yet — scan the QR code from your phone",
    );
    pub fn remote_no_devices() -> &'static CStr { REMOTE_NO_DEVICES.cstr() }
    pub const REMOTE_REVOKE_BTN: UiText = UiText::new(c"Отозвать", c"Revoke");
    pub fn remote_revoke_btn() -> &'static CStr { REMOTE_REVOKE_BTN.cstr() }
    pub const REMOTE_PAIRED_WORD: UiText = UiText::new(c"устр.", c"paired");
    pub fn remote_paired_word() -> &'static CStr { REMOTE_PAIRED_WORD.cstr() }
    pub const SETTINGS_REMOTE_TITLE: UiText = UiText::new(
        c"Мобильный доступ (Android / PWA)",
        c"Mobile Remote (Android / PWA)",
    );
    pub fn settings_remote_title() -> &'static CStr { SETTINGS_REMOTE_TITLE.cstr() }
    pub const SETTINGS_REMOTE_SUBTITLE: UiText = UiText::new(
        c"Локальный HTTP/SSE-мост по Wi-Fi или WireGuard для Android-приложения и браузера",
        c"Local HTTP/SSE bridge over Wi-Fi or WireGuard for the Android app and mobile PWA",
    );
    pub fn settings_remote_subtitle() -> &'static CStr { SETTINGS_REMOTE_SUBTITLE.cstr() }
    pub const SETTINGS_REMOTE_BIND: UiText = UiText::new(c"Интерфейс", c"Bind IP");
    pub fn settings_remote_bind() -> &'static CStr { SETTINGS_REMOTE_BIND.cstr() }
    pub const SETTINGS_REMOTE_PORT: UiText = UiText::new(c"Порт", c"Port");
    pub fn settings_remote_port() -> &'static CStr { SETTINGS_REMOTE_PORT.cstr() }
    pub const SETTINGS_REMOTE_MUTE: UiText = UiText::new(
        c"Не слать пуши на телефон, пока окно AkhTerm в фокусе",
        c"Mute phone alerts while AkhTerm window is focused",
    );
    pub fn settings_remote_mute() -> &'static CStr { SETTINGS_REMOTE_MUTE.cstr() }
    pub const SETTINGS_REMOTE_QR_BTN: UiText = UiText::new(c"QR и устройства", c"QR & Devices");
    pub fn settings_remote_qr_btn() -> &'static CStr { SETTINGS_REMOTE_QR_BTN.cstr() }
    pub const SETTINGS_REMOTE_RUNNING: UiText = UiText::new(c"Активен на", c"Listening on");
    pub fn settings_remote_running() -> &'static CStr { SETTINGS_REMOTE_RUNNING.cstr() }
    pub const SETTINGS_REMOTE_STOPPED: UiText = UiText::new(
        c"Выключен — переключите тумблер выше, чтобы открыть доступ с телефона",
        c"Stopped — toggle on above to pair your phone",
    );
    pub fn settings_remote_stopped() -> &'static CStr { SETTINGS_REMOTE_STOPPED.cstr() }
    pub const PALETTE_REMOTE_TITLE: UiText = UiText::new(
        c"Мобильный доступ и QR-код",
        c"Mobile Remote & QR Pairing",
    );
    pub const PALETTE_REMOTE_DETAIL: UiText = UiText::new(
        c"Показать QR для Android / PWA или настроить локальный сервер",
        c"Show Android / PWA pairing QR code or configure local remote server",
    );

    pub const DROP_INSERT_TITLE: UiText = UiText::new(c"Вставить путь", c"Insert path");
    pub const DROP_INSERT_LOCAL_SUB: UiText =
        UiText::new(c"Экранированный путь в терминал", c"Quoted path into terminal");
    pub const DROP_INSERT_REMOTE_SUB: UiText =
        UiText::new(c"Загрузить в /tmp и вставить путь", c"Upload to /tmp and insert path");
    pub const DROP_TRANSFER_LOCAL_TITLE: UiText = UiText::new(c"Скопировать сюда", c"Copy here");
    pub const DROP_TRANSFER_REMOTE_TITLE: UiText = UiText::new(c"Загрузить сюда", c"Upload here");
    pub const DROP_TRANSFER_SUB: UiText =
        UiText::new(c"В текущий каталог", c"Into working directory");
    pub const DROP_PASTE_IMAGE_SUB: UiText =
        UiText::new(c"Сохранить изображение и передать", c"Save image and transfer");

    pub const TRANSFER_BADGE: UiText = UiText::new(c"ПЕРЕДАЧА ФАЙЛОВ", c"FILE TRANSFER");
    pub const TRANSFER_LOCAL_COPY: UiText =
        UiText::new(c"Копирование в каталог", c"Copying to folder");
    pub const TRANSFER_SSH_UPLOAD: UiText =
        UiText::new(c"Загрузка по SSH", c"Uploading via SSH");
    pub const TRANSFER_SSH_DOWNLOAD: UiText =
        UiText::new(c"Скачивание с SSH", c"Downloading from SSH");
    pub const TRANSFER_SSH_BRIDGE: UiText =
        UiText::new(c"Передача SSH → SSH", c"Transferring SSH → SSH");
    pub const TRANSFER_DONE: UiText = UiText::new(c"Готово", c"Done");
    pub const TRANSFER_FAILED: UiText = UiText::new(c"Ошибка", c"Failed");
    pub const TRANSFER_CANCELLED: UiText = UiText::new(c"Отменено", c"Cancelled");
    pub const TRANSFER_BTN_CANCEL: UiText = UiText::new(c"Отмена", c"Cancel");
    pub const TRANSFER_BTN_OPEN_FOLDER: UiText =
        UiText::new(c"Открыть папку", c"Open folder");
    pub const TRANSFER_BTN_INSERT_PATH: UiText =
        UiText::new(c"Вставить путь", c"Insert path");
    pub const TRANSFER_BTN_DISMISS: UiText = UiText::new(c"Скрыть", c"Dismiss");
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
            STATUS_ERROR, NOTIFY_FOCUS_ACTION, NOTIFY_ERROR_TITLE,
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
            i18n_text::ABOUT_COPIED_BTN, i18n_text::ABOUT_CLOSE_BTN, i18n_text::ABOUT_CHECK_UPDATES_BTN,
            i18n_text::UPDATE_BADGE, i18n_text::UPDATE_AUTHOR, i18n_text::UPDATE_CLOSE_TIP,
            i18n_text::UPDATE_STATUS_AVAILABLE, i18n_text::UPDATE_STATUS_DOWNLOADING,
            i18n_text::UPDATE_STATUS_INSTALLING, i18n_text::UPDATE_STATUS_READY,
            i18n_text::UPDATE_WHATS_NEW, i18n_text::UPDATE_DISMISS, i18n_text::UPDATE_BTN_UPDATE,
            i18n_text::UPDATE_BTN_RESTART, i18n_text::UPDATE_BTN_RETRY,
            i18n_text::SIDEBAR_TITLE, i18n_text::SIDEBAR_SEARCH_PLACEHOLDER,
            i18n_text::SIDEBAR_GROUP_LOCAL, i18n_text::SIDEBAR_GROUP_SSH,
            i18n_text::SIDEBAR_GROUP_MONITOR, i18n_text::SIDEBAR_EMPTY_TABS,
            i18n_text::SIDEBAR_EMPTY_SEARCH, i18n_text::SIDEBAR_TMUX_PREFIX,
            i18n_text::SIDEBAR_FOLDER_NEW_TAB_TIP,
            i18n_text::DROP_INSERT_TITLE, i18n_text::DROP_INSERT_LOCAL_SUB,
            i18n_text::DROP_INSERT_REMOTE_SUB, i18n_text::DROP_TRANSFER_LOCAL_TITLE,
            i18n_text::DROP_TRANSFER_REMOTE_TITLE, i18n_text::DROP_TRANSFER_SUB,
            i18n_text::DROP_PASTE_IMAGE_SUB, i18n_text::TRANSFER_BADGE,
            i18n_text::TRANSFER_LOCAL_COPY, i18n_text::TRANSFER_SSH_UPLOAD,
            i18n_text::TRANSFER_SSH_DOWNLOAD, i18n_text::TRANSFER_SSH_BRIDGE,
            i18n_text::TRANSFER_DONE, i18n_text::TRANSFER_FAILED,
            i18n_text::TRANSFER_CANCELLED, i18n_text::TRANSFER_BTN_CANCEL,
            i18n_text::TRANSFER_BTN_OPEN_FOLDER, i18n_text::TRANSFER_BTN_INSERT_PATH,
            i18n_text::TRANSFER_BTN_DISMISS,
        ] {
            assert_pair_valid(entry);
        }
    }

    fn verify_ports_pairs() {
        for entry in [
            i18n_text::PORTS_BTN, i18n_text::PORTS_BTN_WORD, i18n_text::PORTS_TIP,
            i18n_text::PORTS_HEADER, i18n_text::PORTS_REFRESH_TIP, i18n_text::PORTS_SECTION_TUNNELS,
            i18n_text::PORTS_SECTION_LOCAL, i18n_text::PORTS_SECTION_REMOTE, i18n_text::PORTS_OPEN,
            i18n_text::PORTS_COPY, i18n_text::PORTS_FORWARD_OPEN, i18n_text::PORTS_STOP_TIP,
            i18n_text::PORTS_TO_WORD, i18n_text::PORTS_EMPTY_LOCAL, i18n_text::PORTS_EMPTY_REMOTE,
            i18n_text::PORTS_COPIED, i18n_text::PORTS_TUNNEL_READY, i18n_text::PORTS_TUNNEL_STOPPED,
            i18n_text::PORTS_TUNNEL_FAILED,
        ] {
            assert_pair_valid(entry);
            for loc in [UiLocale::Ru, UiLocale::En] {
                let s = entry.for_locale(loc).to_str().unwrap();
                assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"), "arrow in {s}");
            }
        }
    }

    fn verify_sys_monitor_pairs() {
        for entry in [
            i18n_text::SYS_MON_TIP, i18n_text::SYS_MON_TITLE, i18n_text::SYS_MON_CPU,
            i18n_text::SYS_MON_CPU_DETAIL, i18n_text::SYS_MON_LOAD, i18n_text::SYS_MON_RAM,
            i18n_text::SYS_MON_SWAP, i18n_text::SYS_MON_SWAP_OFF, i18n_text::SYS_MON_TAB_RSS,
            i18n_text::SYS_MON_BATTERY, i18n_text::SYS_MON_POWER, i18n_text::SYS_MON_ETA_LEFT,
            i18n_text::SYS_MON_ETA_FULL, i18n_text::SYS_MON_HEALTH, i18n_text::SYS_MON_AC_ONLY,
            i18n_text::SYS_MON_BAT_DISCHARGING, i18n_text::SYS_MON_BAT_CHARGING,
            i18n_text::SYS_MON_BAT_FULL, i18n_text::SYS_MON_BAT_NOT_CHARGING,
            i18n_text::SYS_MON_BAT_UNKNOWN,
        ] {
            assert_pair_valid(entry);
        }
    }

    fn verify_checkpoint_pairs() {
        for entry in [
            CP_SECTION_HEADER, CP_SAVE_BTN, CP_SAVE_TIP, CP_UNDO_BTN, CP_UNDO_TIP, CP_EMPTY,
            CP_PALETTE_SAVE_TITLE, CP_PALETTE_SAVE_DETAIL, CP_PALETTE_UNDO_TITLE,
            CP_PALETTE_UNDO_DETAIL, CP_MSG_LOCAL_ONLY, CP_MSG_CLEAN, CP_MSG_DUPLICATE,
            CP_MSG_NONE, CP_MSG_SAVED, CP_MSG_RESTORED,
        ] {
            assert_pair_valid(entry);
            for loc in [UiLocale::Ru, UiLocale::En] {
                let s = entry.for_locale(loc).to_str().unwrap();
                assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"), "arrow in {s}");
            }
        }
    }

    fn verify_cmd_mark_pairs() {
        for entry in [
            i18n_text::CTX_PREV_COMMAND, i18n_text::CTX_NEXT_COMMAND,
            i18n_text::CTX_COPY_CMD_OUTPUT, i18n_text::CTX_SEND_FAILED_AGENT,
            i18n_text::CMD_PREV_DETAIL, i18n_text::CMD_NEXT_DETAIL,
            i18n_text::CMD_COPY_OUTPUT_DETAIL, i18n_text::CMD_SEND_FAILED_DETAIL,
            i18n_text::CMD_NO_MARKS, i18n_text::CMD_NO_PREV, i18n_text::CMD_NO_NEXT,
            i18n_text::CMD_NO_FAILED, i18n_text::CMD_OUTPUT_COPIED,
            i18n_text::CMD_SENT_TO_AGENT, i18n_text::CMD_COPIED_NO_AGENT,
        ] {
            assert_pair_valid(entry);
            for loc in [UiLocale::Ru, UiLocale::En] {
                let s = entry.for_locale(loc).to_str().unwrap();
                assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"), "arrow in {s}");
            }
        }
    }

    fn verify_context_handoff_pairs() {
        for entry in [
            i18n_text::CTX_HANDOFF_HEADER, i18n_text::CTX_HANDOFF_TIP,
            i18n_text::CTX_HEALTH_WARN_PREFIX, i18n_text::CTX_HEALTH_WARN_HINT,
            i18n_text::CTX_HEALTH_CRIT_PREFIX, i18n_text::CTX_HEALTH_CRIT_HINT,
            i18n_text::CTX_CACHE_DROP_LABEL, i18n_text::CTX_CACHE_DROP_HINT,
            i18n_text::CTX_HANDOFF_LOCAL_ONLY, i18n_text::CTX_HANDOFF_PALETTE_PREFIX,
            i18n_text::CTX_HANDOFF_PALETTE_DETAIL,
        ] {
            assert_pair_valid(entry);
            for loc in [UiLocale::Ru, UiLocale::En] {
                let s = entry.for_locale(loc).to_str().unwrap();
                assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"), "arrow in {s}");
            }
        }
    }

    fn verify_hooks_pairs() {
        for entry in [
            i18n_text::SETTINGS_HOOKS_TITLE, i18n_text::SETTINGS_HOOKS_SUBTITLE,
            i18n_text::SETTINGS_HOOKS_INSTALL, i18n_text::SETTINGS_HOOKS_UPDATE,
            i18n_text::SETTINGS_HOOKS_REMOVE, i18n_text::SETTINGS_HOOKS_INSTALL_ALL,
            i18n_text::SETTINGS_HOOKS_INSTALLED, i18n_text::SETTINGS_HOOKS_NOT_INSTALLED,
            i18n_text::SETTINGS_HOOKS_OUTDATED, i18n_text::PALETTE_HOOKS_TITLE,
            i18n_text::PALETTE_HOOKS_DETAIL,
        ] {
            assert_pair_valid(entry);
            for loc in [UiLocale::Ru, UiLocale::En] {
                let s = entry.for_locale(loc).to_str().unwrap();
                assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"), "arrow in {s}");
            }
        }
    }

    fn verify_worktree_pairs() {
        for entry in [
            WT_SECTION_HDR, WT_OPEN_BTN, WT_REMOVE_TIP, WT_NEW_BTN, WT_PRUNE_BTN, WT_PRIMARY_TAG,
            WT_MERGED_TAG, WT_DIRTY_TAG, WT_DETACHED_TAG, WT_DLG_TITLE, WT_DLG_BRANCH_LBL,
            WT_DLG_BRANCH_PH, WT_DLG_BASE_LBL, WT_DLG_AGENT_LBL, WT_DLG_CREATE, WT_DLG_CANCEL,
            WT_PALETTE_NEW_TITLE, WT_PALETTE_NEW_DETAIL, WT_PALETTE_PRUNE_TITLE,
            WT_PALETTE_PRUNE_DETAIL, WT_ERR_NOT_REPO, WT_ERR_EMPTY_SLUG, WT_ERR_BAD_SLUG,
            WT_ERR_BAD_BASE, WT_ERR_PRIMARY_REMOVE, WT_ERR_DIRTY_REMOVE, WT_ERR_NOT_FOUND,
            WT_ERR_PATH_EXISTS, WT_MSG_FOCUSED, WT_MSG_OPENED, WT_MSG_CREATED, WT_MSG_REMOVED,
            WT_MSG_PRUNED_NONE, WT_MSG_PRUNED_DONE, WT_LOCAL_ONLY,
        ] {
            assert_pair_valid(entry);
            for loc in [UiLocale::Ru, UiLocale::En] {
                let s = entry.for_locale(loc).to_str().unwrap();
                assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"), "arrow in {s}");
            }
        }
    }

    fn verify_remote_pairs() {
        for entry in [
            i18n_text::REMOTE_BTN, i18n_text::REMOTE_BTN_WORD, i18n_text::REMOTE_TIP,
            i18n_text::REMOTE_HEADER, i18n_text::REMOTE_LIVE_CLIENTS, i18n_text::REMOTE_ROTATE_QR,
            i18n_text::REMOTE_COPY_URL, i18n_text::REMOTE_URL_COPIED, i18n_text::REMOTE_STOP_BTN,
            i18n_text::REMOTE_DEVICES_HEADER, i18n_text::REMOTE_NO_DEVICES, i18n_text::REMOTE_REVOKE_BTN,
            i18n_text::REMOTE_PAIRED_WORD, i18n_text::SETTINGS_REMOTE_TITLE,
            i18n_text::SETTINGS_REMOTE_SUBTITLE, i18n_text::SETTINGS_REMOTE_BIND,
            i18n_text::SETTINGS_REMOTE_PORT, i18n_text::SETTINGS_REMOTE_MUTE,
            i18n_text::SETTINGS_REMOTE_QR_BTN, i18n_text::SETTINGS_REMOTE_RUNNING,
            i18n_text::SETTINGS_REMOTE_STOPPED, i18n_text::PALETTE_REMOTE_TITLE,
            i18n_text::PALETTE_REMOTE_DETAIL,
        ] {
            assert_pair_valid(entry);
            for loc in [UiLocale::Ru, UiLocale::En] {
                let s = entry.for_locale(loc).to_str().unwrap();
                assert!(!s.contains("->") && !s.contains('→') && !s.contains("=>"), "arrow in {s}");
            }
        }
    }

    #[test]
    fn every_ui_text_entry_has_valid_non_empty_ru_and_en_strings() {
        verify_core_and_palette_pairs();
        verify_tools_context_and_dialog_pairs();
        verify_ports_pairs();
        verify_sys_monitor_pairs();
        verify_checkpoint_pairs();
        verify_cmd_mark_pairs();
        verify_context_handoff_pairs();
        verify_hooks_pairs();
        verify_worktree_pairs();
        verify_remote_pairs();
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
