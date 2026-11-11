//! Clipboard paste routing across local shells, multimodal TUI agents, and SSH sessions.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteRoute {
    /// Pass raw `Ctrl+V` (`0x16`) through to VTE so a local multimodal TUI agent
    /// reads the image directly from the local display clipboard as `[Image]`.
    PassThroughToPty,
    /// Intercept clipboard images, save locally or upload via SSH, and feed the path.
    InterceptClipboardImage,
}

pub fn is_multimodal_tui_agent(agent: &str) -> bool {
    matches!(agent, "AGY" | "Claude" | "Codex")
}

pub fn choose_paste_route(plain_ctrl_v: bool, remote_host: &str, agent: &str) -> PasteRoute {
    if plain_ctrl_v && remote_host.is_empty() && is_multimodal_tui_agent(agent) {
        PasteRoute::PassThroughToPty
    } else {
        PasteRoute::InterceptClipboardImage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_plain_ctrl_v_through_for_local_multimodal_agents() {
        for agent in ["AGY", "Claude", "Codex"] {
            assert!(is_multimodal_tui_agent(agent));
            assert_eq!(
                choose_paste_route(true, "", agent),
                PasteRoute::PassThroughToPty
            );
        }
    }

    #[test]
    fn intercepts_remote_sessions_even_when_multimodal_agent_runs() {
        for host in ["gpu-box", "prod-tmux", "10.0.0.12"] {
            for agent in ["", "AGY", "Claude", "Codex", "DSH", "Kimi", "DeepSeek"] {
                assert_eq!(
                    choose_paste_route(true, host, agent),
                    PasteRoute::InterceptClipboardImage
                );
            }
        }
    }

    #[test]
    fn intercepts_ctrl_shift_v_and_toolbar_screenshot_paste() {
        for host in ["", "gpu-box"] {
            for agent in ["", "AGY", "Claude", "Codex", "DSH", "Kimi", "DeepSeek"] {
                assert_eq!(
                    choose_paste_route(false, host, agent),
                    PasteRoute::InterceptClipboardImage
                );
            }
        }
    }

    #[test]
    fn intercepts_plain_local_shells_and_non_multimodal_agents() {
        for agent in ["", "DSH", "Kimi", "DeepSeek", "bash", "zsh"] {
            assert!(!is_multimodal_tui_agent(agent));
            assert_eq!(
                choose_paste_route(true, "", agent),
                PasteRoute::InterceptClipboardImage
            );
        }
    }

    #[test]
    fn paste_route_module_maintains_high_mi() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("paste_route.rs"));
        assert!(mi >= 58.0, "paste_route.rs MI = {mi:.2}");
    }
}
