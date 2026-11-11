//! Clipboard copy/paste routing across local shells, multimodal TUI agents, and SSH sessions.

use crate::ffi::{GDK_CONTROL_MASK, GDK_MOD1_MASK, GDK_SHIFT_MASK};

const KEYCODE_C: u16 = 54;
const KEYCODE_V: u16 = 55;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteRoute {
    /// Pass raw `Ctrl+V` (`0x16`) through to VTE so a local multimodal TUI agent
    /// reads the image directly from the local display clipboard as `[Image]`.
    PassThroughToPty,
    /// Intercept clipboard images or `file://` URIs, or fall back to VTE text paste.
    InterceptClipboardImage,
}

pub fn is_c_key(keyval: u32, keycode: u16) -> bool {
    matches!(keyval, 0x43 | 0x63 | 0x06d3 | 0x06f3 | 0x0421 | 0x0441) || keycode == KEYCODE_C
}

pub fn is_v_key(keyval: u32, keycode: u16) -> bool {
    matches!(keyval, 0x56 | 0x76 | 0x06cd | 0x06ed | 0x041c | 0x043c) || keycode == KEYCODE_V
}

pub fn is_plain_ctrl_shortcut(state: u32) -> bool {
    let ctrl = (state & GDK_CONTROL_MASK) != 0;
    let no_shift_or_alt = (state & (GDK_SHIFT_MASK | GDK_MOD1_MASK)) == 0;
    ctrl && no_shift_or_alt
}

pub fn is_ctrl_shift_shortcut(state: u32) -> bool {
    let ctrl = (state & GDK_CONTROL_MASK) != 0;
    let shift = (state & GDK_SHIFT_MASK) != 0;
    let no_alt = (state & GDK_MOD1_MASK) == 0;
    ctrl && shift && no_alt
}

pub fn is_multimodal_tui_agent(agent: &str) -> bool {
    matches!(agent, "AGY" | "Claude" | "Codex")
}

pub fn resolve_pane_agent(
    detected: Option<&str>,
    is_primary_pane: bool,
    session_agent: &str,
) -> String {
    if let Some(agent) = detected.filter(|a| !a.is_empty()) {
        return agent.to_owned();
    }
    if is_primary_pane {
        session_agent.to_owned()
    } else {
        String::new()
    }
}

pub fn choose_paste_route(
    plain_ctrl_v: bool,
    clipboard_has_image: bool,
    remote_host: &str,
    agent: &str,
) -> PasteRoute {
    let local_agent = remote_host.is_empty() && is_multimodal_tui_agent(agent);
    if plain_ctrl_v && clipboard_has_image && local_agent {
        PasteRoute::PassThroughToPty
    } else {
        PasteRoute::InterceptClipboardImage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_plain_ctrl_v_through_only_when_clipboard_has_image_on_local_multimodal_agents() {
        for agent in ["AGY", "Claude", "Codex"] {
            assert!(is_multimodal_tui_agent(agent));
            assert_eq!(
                choose_paste_route(true, true, "", agent),
                PasteRoute::PassThroughToPty
            );
            assert_eq!(
                choose_paste_route(true, false, "", agent),
                PasteRoute::InterceptClipboardImage
            );
        }
    }

    #[test]
    fn intercepts_remote_sessions_even_when_multimodal_agent_and_image_present() {
        for host in ["gpu-box", "prod-tmux", "10.0.0.12"] {
            for has_image in [false, true] {
                for agent in ["", "AGY", "Claude", "Codex", "DSH", "Kimi", "DeepSeek"] {
                    assert_eq!(
                        choose_paste_route(true, has_image, host, agent),
                        PasteRoute::InterceptClipboardImage
                    );
                }
            }
        }
    }

    #[test]
    fn intercepts_ctrl_shift_v_and_toolbar_screenshot_paste() {
        for host in ["", "gpu-box"] {
            for has_image in [false, true] {
                for agent in ["", "AGY", "Claude", "Codex", "DSH", "Kimi", "DeepSeek"] {
                    assert_eq!(
                        choose_paste_route(false, has_image, host, agent),
                        PasteRoute::InterceptClipboardImage
                    );
                }
            }
        }
    }

    #[test]
    fn intercepts_plain_local_shells_and_non_multimodal_agents() {
        for agent in ["", "DSH", "Kimi", "DeepSeek", "bash", "zsh"] {
            assert!(!is_multimodal_tui_agent(agent));
            for has_image in [false, true] {
                assert_eq!(
                    choose_paste_route(true, has_image, "", agent),
                    PasteRoute::InterceptClipboardImage
                );
            }
        }
    }

    #[test]
    fn matches_copy_paste_shortcuts_across_latin_cyrillic_and_alt_guards() {
        const LOCK_AND_GROUP: u32 = (1 << 1) | (1 << 4) | (1 << 13);
        assert!(is_plain_ctrl_shortcut(GDK_CONTROL_MASK | LOCK_AND_GROUP));
        assert!(!is_plain_ctrl_shortcut(GDK_CONTROL_MASK | GDK_SHIFT_MASK));
        assert!(!is_plain_ctrl_shortcut(GDK_CONTROL_MASK | GDK_MOD1_MASK));
        assert!(is_ctrl_shift_shortcut(GDK_CONTROL_MASK | GDK_SHIFT_MASK | LOCK_AND_GROUP));
        assert!(!is_ctrl_shift_shortcut(GDK_CONTROL_MASK | GDK_SHIFT_MASK | GDK_MOD1_MASK));
        assert!(is_c_key(u32::from(b'c'), 0) && is_c_key(0x06d3, 0) && is_c_key(0, 54));
        assert!(is_v_key(u32::from(b'v'), 0) && is_v_key(0x06cd, 0) && is_v_key(0, 55));
        assert!(!is_c_key(u32::from(b'x'), 53) && !is_v_key(u32::from(b'b'), 56));
    }

    #[test]
    fn resolves_split_pane_agent_with_primary_fallback_only() {
        assert_eq!(resolve_pane_agent(Some("AGY"), false, "Claude"), "AGY");
        assert_eq!(resolve_pane_agent(None, true, "Claude"), "Claude");
        assert_eq!(resolve_pane_agent(None, false, "Claude"), "");
    }

    #[test]
    fn paste_route_module_maintains_high_mi() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("paste_route.rs"));
        assert!(mi >= 58.0, "paste_route.rs MI = {mi:.2}");
    }
}
