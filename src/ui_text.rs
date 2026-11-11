//! Static interface labels and tooltips shared by the GTK views.

use std::ffi::CStr;

pub const APP_NAME: &CStr = c"AkhTerm";
pub const PROFILES: &CStr = c"Профили / SSH ▾";
pub const SERVERS: &CStr = c"Серверы ▾";
pub const AGENTS: &CStr = c"Агенты ▾";
pub const HANDOFF: &CStr = c"Handoff ▾";
pub const MAINTAINABILITY: &CStr = c"MI ▾";
pub const TOGGLE_SIDEBAR: &CStr = c"◧";
pub const TOGGLE_SIDEBAR_TIP: &CStr = c"Показать или скрыть боковую панель";
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

pub const AGENT_ACTIONS: [(&CStr, &str); 5] = [
    (c"agy · Antigravity CLI", "agy\n"),
    (c"codex · OpenAI Codex CLI", "codex\n"),
    (c"claude · Claude Code", "claude\n"),
    (c"dsh · DSH Agent", "dsh\n"),
    (c"kimi · Kimi Code CLI", "kimi\n"),
];
