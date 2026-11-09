# Working on AkhTerm

AkhTerm is a native Linux terminal built with Rust, GTK 3 and VTE 2.91. Start with [README.md](README.md) for setup and [config.example.conf](config.example.conf) for profiles.

## Build and verify

Use Rust 1.85.1 and the system packages listed in the README. Keep Cargo builds offline with no external Rust crates. GTK and VTE provide the interface and terminal widget.

```sh
cargo test --offline --locked
cargo build --release --offline --locked
./packaging/build-deb.sh
```

The [package workflow](.github/workflows/build-deb.yml) builds on Ubuntu 24.04 and checks installation in clean Ubuntu 24.04 and 26.04 containers. Check the rebuilt package whenever you change assets or native dependencies. Keep the application license and third-party notices in the installer.

Keep changes small and reuse the existing modules. Aim for a Maintainability Index of at least 75. Use the same calculation as `src/mi.rs` when comparing touched modules. For modules already below the target, record the before and after scores and explain any decline.

## Interface

- Use GTK widgets and the Zinc palette in `src/theme.css`, `src/tab_strip.css` and `src/sidebar.css`. Keep the dark preference and native header bar together so the top of the window follows the application theme.
- Use the bundled Geist font for interface text. Register it inside the process before GTK starts. Terminal cells use a monospace font and have their own zoom controls.
- Keep static button labels and tooltips as named `&CStr` constants in `src/ui_text.rs`. Build tab names, connection labels and numeric values from live state. Give truncated titles a tooltip with the full text.
- Keep each agent menu label with its shell command in `AGENT_ACTIONS`. Check local process detection in `src/proc_radar.rs` and tmux title detection in `src/session.rs` when adding an agent.
- Check interface changes in GTK with Cyrillic labels. Try opening and closing tabs by mouse and keyboard, narrowing the window, and switching to and from fullscreen.

## Configuration and sessions

- Verify first launch with the built-in profile. Start with empty `XDG_CONFIG_HOME` and `XDG_STATE_HOME` directories and unset `AKHTERM_CONFIG`. A fresh workspace opens one local shell in `~`. Use an isolated home directory for checks that must exclude existing OpenSSH aliases.
- Keep machine-specific paths and SSH aliases in local configuration. Use placeholders in examples and exclude private configs, saved state and build output from commits.
- Keep tab order, the active tab and local working directories restorable. Preserve the existing state format and atomic writes. A tab closed before exit stays closed, and an empty saved workspace stays empty.
- Reconnect an SSH or tmux tab by replacing its local client. Preserve the chosen remote tmux session and its running processes.
- Keep shortcuts consistent with the README. `Ctrl+Shift+T` opens a local tab, `Ctrl+Shift+W` closes the active tab, and `Ctrl+Shift+R` reconnects it when it is remote.
