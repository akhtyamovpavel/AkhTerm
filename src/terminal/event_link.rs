//! Resolve OSC hyperlinks and plain text matches at the clicked terminal cell.
use crate::{ffi::*, markdown, proc_radar::resolve_link_target};
use crate::{
    paths::expand_home,
    proc_radar::{foreground_cwd, open_external_link},
};
use std::cell::Cell;
use std::ffi::{c_char, c_int, c_void, CStr, CString};

pub(super) fn take_c_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let text = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
    unsafe { g_free(ptr.cast::<c_void>()) };
    Some(text)
}

fn event_target(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    cwd: Option<&str>,
) -> Option<(String, Option<markdown::RemoteDocument>)> {
    if vte.is_null() || event.is_null() {
        return None;
    }
    let resolve = |raw: String| {
        if let Some(doc) = markdown::remote_document(vte, &raw) {
            return Some((raw, Some(doc)));
        }
        resolve_link_target(&raw, cwd).map(|target| (target, None))
    };
    unsafe {
        take_c_string(vte_terminal_hyperlink_check_event(vte, event))
            .and_then(&resolve)
            .or_else(|| {
                let mut tag = -1;
                take_c_string(vte_terminal_match_check_event(vte, event, &mut tag))
                    .and_then(resolve)
            })
    }
}

#[cfg(test)]
pub(super) fn check_vte_event_link(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    cwd: Option<&str>,
) -> Option<String> {
    event_target(vte, event, cwd).map(|(target, _)| target)
}

pub(super) const URL_PATTERNS: &[&str] = &[
    r"https?://[^\s\x22'<>()`│┃┆┊║]+",
    r"file://[^\s\x22'<>()\]`│┃┆┊║]+(?:\([^\s\x22'<>()\]`│┃┆┊║]*\)[^\s\x22'<>()\]`│┃┆┊║]*)*",
    r"\b(localhost|127\.0\.0\.1|0\.0\.0\.0):[0-9]{2,5}(/[^\s\x22'<>()`│┃┆┊║]*)?",
    r"(?:~|/home/[a-zA-Z0-9_.-]+)/[^\s\x22'<>()\]`│┃┆┊║]+(?:\([^\s\x22'<>()\]`│┃┆┊║]*\)[^\s\x22'<>()\]`│┃┆┊║]*)*",
    r"(?<![^\s\x22'<>(\[{=`│┃┆┊║])(?:(?:[^\s\x22'<>():~`│┃┆┊║/\[\]{};,]+/)+[^\s\x22'<>():~`│┃┆┊║/\[\]{};,]+\.[a-zA-Z][a-zA-Z0-9]{0,9}(?::[0-9]+){0,2}|(?:[^\s\x22'<>():~`│┃┆┊║/\[\]{};,]*[^\s\x22'<>():~`│┃┆┊║/\[\]{};,.])\.[a-zA-Z][a-zA-Z0-9]{0,9}:[0-9]+(?::[0-9]+)?)",
    r"(?i)(?<![^\s\x22'<>(\[{`=│┃┆┊║])(?:/|\./|\.\./)?[^\s\x22'<>():\[\]{}`│┃┆┊║]+\.(?:md|markdown)\b",
];

pub(super) fn register_vte_url_matches(vte: *mut GtkWidget) {
    unsafe {
        for pat in URL_PATTERNS {
            let Ok(pat_c) = CString::new(*pat) else {
                continue;
            };
            let vreg = vte_regex_new_for_match(
                pat_c.as_ptr(),
                -1,
                PCRE2_MULTILINE_UTF,
                std::ptr::null_mut(),
            );
            if !vreg.is_null() {
                let tag = vte_terminal_match_add_regex(vte, vreg, 0);
                if tag >= 0 {
                    vte_terminal_match_set_cursor_name(vte, tag, c"pointer".as_ptr());
                }
                vte_regex_unref(vreg);
            }
        }
    }
}

pub enum LinkCwd {
    Unmanaged,
    Remote,
    Local(String),
}

struct VteLinkCtx {
    pid: i32,
    fallback_cwd: String,
}

pub type VteMenuHook =
    fn(*mut GtkWidget, *const GdkEvent, Option<String>, Option<crate::markdown::RemoteDocument>);

thread_local! {
    static LINK_CWD_FN: Cell<Option<fn(*mut GtkWidget) -> LinkCwd>> = const { Cell::new(None) };
    static VTE_MENU_FN: Cell<Option<VteMenuHook>> = const { Cell::new(None) };
}

pub fn register_link_cwd_lookup(lookup: fn(*mut GtkWidget) -> LinkCwd) {
    LINK_CWD_FN.with(|slot| slot.set(Some(lookup)));
}

pub fn register_vte_menu_hook(hook: VteMenuHook) {
    VTE_MENU_FN.with(|slot| slot.set(Some(hook)));
}

extern "C" fn drop_vte_link_ctx(data: *mut c_void, _closure: *mut c_void) {
    if !data.is_null() {
        unsafe { drop(Box::from_raw(data.cast::<VteLinkCtx>())) };
    }
}

fn resolve_vte_cwd(vte: *mut GtkWidget, user_data: *mut c_void) -> Option<String> {
    if let Some(lookup) = LINK_CWD_FN.with(Cell::get) {
        match lookup(vte) {
            LinkCwd::Local(dir) => return Some(dir),
            LinkCwd::Remote => return None,
            LinkCwd::Unmanaged => {}
        }
    }
    if user_data.is_null() {
        return None;
    }
    let ctx = unsafe { &*user_data.cast::<VteLinkCtx>() };
    foreground_cwd(ctx.pid)
        .or_else(|| (!ctx.fallback_cwd.is_empty()).then(|| ctx.fallback_cwd.clone()))
}

fn handle_vte_right_click(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    user_data: *mut c_void,
) -> c_int {
    let mut state: u32 = 0;
    unsafe { gdk_event_get_state(event, &mut state) };
    if crate::context_menu::should_passthrough_vte_click(3, state) {
        return 0;
    }
    let Some(hook) = VTE_MENU_FN.with(Cell::get) else {
        return 0;
    };
    let cwd = resolve_vte_cwd(vte, user_data);
    let (link, doc) = event_target(vte, event, cwd.as_deref()).unzip();
    hook(vte, event, link, doc.flatten());
    1
}

pub fn should_open_vte_link(button: u32, state: u32) -> bool {
    if button != 1 {
        return false;
    }
    let ctrl_held = (state & GDK_CONTROL_MASK) != 0;
    let no_alt = (state & GDK_MOD1_MASK) == 0;
    ctrl_held && no_alt
}

pub(super) fn handle_vte_left_click(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    state: u32,
    user_data: *mut c_void,
) -> c_int {
    if !should_open_vte_link(1, state) {
        return 0;
    }
    let cwd = resolve_vte_cwd(vte, user_data);
    match event_target(vte, event, cwd.as_deref()) {
        Some((_, Some(doc))) => markdown::open_remote(doc),
        Some((target, None)) => open_external_link(&target),
        None => return 0,
    }
    1
}

pub(super) extern "C" fn on_vte_button_press(
    vte: *mut GtkWidget,
    event: *const GdkEvent,
    user_data: *mut c_void,
) -> c_int {
    let mut button: u32 = 0;
    let mut state: u32 = 0;
    let is_press = unsafe {
        !event.is_null()
            && gdk_event_get_event_type(event) == GDK_BUTTON_PRESS
            && gdk_event_get_button(event, &mut button) != 0
    };
    if !is_press {
        return 0;
    }
    unsafe { gdk_event_get_state(event, &mut state) };
    match button {
        1 => handle_vte_left_click(vte, event, state, user_data),
        3 => handle_vte_right_click(vte, event, user_data),
        _ => 0,
    }
}

pub(super) fn connect(vte: *mut GtkWidget, pid: i32, cwd: &str) {
    unsafe {
        let ctx = Box::into_raw(Box::new(VteLinkCtx {
            pid,
            fallback_cwd: expand_home(cwd),
        }));
        g_signal_connect_data(
            vte,
            c"button-press-event".as_ptr(),
            on_vte_button_press as *const c_void,
            ctx.cast(),
            drop_vte_link_ctx as *const c_void,
            0,
        );
    }
}
