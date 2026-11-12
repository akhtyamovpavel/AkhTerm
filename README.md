# AkhTerm

AkhTerm is a native Linux terminal built with Rust, GTK 3 and VTE. It puts local shells, SSH connections and remote tmux sessions in one window. It remembers the open tabs, their order, pinned titles, the active tab, and the split-pane layouts and working directories of local shells.

![AkhTerm showing color-coded agent tabs beside a demo SSH tab attached to tmux](assets/screenshots/akhterm.png)

## Install a package

### Debian and Ubuntu (`.deb`)

The `amd64` package was tested on Ubuntu 24.04 and 26.04. Open the `.deb` in your graphical package installer, or install it with APT from the directory containing the file.

```sh
sudo apt install ./akhterm_1.0.0-1_amd64.deb
```

Start **AkhTerm** from the application menu or run `akhterm` (`akhterm --version` prints version and build metadata). Remove it with `sudo apt remove akhterm`. Your configuration and saved tabs stay in your home directory.

The [Build Debian package workflow](.github/workflows/build-deb.yml) runs on pushes and pull requests and can also be started manually. It builds the package on Ubuntu 24.04 and checks installation in Ubuntu 24.04 and 26.04 containers. Download the `.deb` from **Artifacts** on a successful run in GitHub Actions, or attach both `.deb` and `.rpm` artifacts automatically via the [Release workflow](.github/workflows/release.yml) on `v*` tags.

### Fedora (`.rpm`)

The `x86_64` RPM package is tested on Fedora 41 and 42. Install it with DNF from the directory containing the file.

```sh
sudo dnf install ./akhterm-1.0.0-1.x86_64.rpm
```

Start **AkhTerm** from the application menu or run `akhterm`. Remove it with `sudo dnf remove akhterm`.

The [Build Fedora RPM package workflow](.github/workflows/build-rpm.yml) runs on pushes and pull requests and can also be started manually. It builds and tests the package on Fedora 41 and verifies installation in clean Fedora 41 and 42 containers, including KDE Breeze GTK 3 and icon packages. Download the `.rpm` from **Artifacts** on a successful run in GitHub Actions.

AkhTerm runs natively under both **Wayland** and **X11** on **GNOME** and **KDE Plasma**. Its Freedesktop desktop entry (`org.akhterm.AkhTerm.desktop`) provides `StartupWMClass=AkhTerm` and localized metadata for Wayland and X11 application launchers and task switchers.

## Build and run

Install the native GTK 3, VTE 2.91 and Fontconfig build dependencies for your distribution. Rust and Cargo are also required; the build is verified with Rust and Cargo 1.85.

On Debian or Ubuntu:

```sh
sudo apt install build-essential libfontconfig-dev libgtk-3-dev libvte-2.91-0
```

On Fedora:

```sh
sudo dnf install gcc fontconfig-devel gtk3-devel vte291-devel
```

Then build and run from the repository root:

```sh
cargo build --release --offline --locked
./target/release/akhterm
```

Cargo needs no external Rust crates. AkhTerm runs in a Linux graphical session. SSH tabs use the `ssh` executable, and remote tmux tabs require `tmux` on the remote host. The optional system monitor uses `htop`, and Wayland image clipboard helpers can optionally use `wl-clipboard`.

To build the `.deb` on an Ubuntu 24.04 base, install `dpkg-dev` and run the Debian packaging script from the source directory. It writes the package to `target/deb/`.

```sh
sudo apt install dpkg-dev
./packaging/build-deb.sh
```

To build the `.rpm` on Fedora, install `rpm-build` and run the RPM packaging script from the source directory. It writes the package to `target/rpm/`.

```sh
sudo dnf install rpm-build
./packaging/build-rpm.sh
```

## Try it

1. Click `+` in the tab strip or press `Ctrl+Shift+T` to open another local tab. Change to a project directory with `cd`.
2. Close AkhTerm and run it again. The tab opens in that directory and focus returns to the previously active tab.
3. If you already have an alias in `~/.ssh/config`, open the «Профили / SSH» menu. Choose `ssh` for a remote shell or `tmux:main` for a persistent session. Use `Ctrl+Shift+R` to reconnect the active SSH tab and return to its tmux session. Running an interactive `ssh` session inside a local shell tab moves that tab into the «SSH» sidebar group while connected and returns it to «Локальные» when `ssh` exits.

`Ctrl+Shift+D` splits the active tab vertically and `Ctrl+Shift+E` splits it horizontally in the current foreground directory (supporting both local tabs and SSH/tmux remote tabs); split layouts (`tabs-v2`) and local directories are restored on restart, with directional focus navigation via `Alt+Arrow` keys and temporary pane zoom via `Ctrl+Shift+Z`. Each split pane shows a compact 22 px header with agent/process status, exit code, turn timer, and context badge. `Ctrl+Shift+W` closes the focused or secondary split pane first, or closes the active tab when one pane remains, and `Ctrl+Shift+U` jumps directly to the next pane waiting for approval or with unread output. Drag tabs with the mouse or press `Ctrl+Shift+PageUp` and `Ctrl+Shift+PageDown` to reorder the active tab, and double-click a tab (or choose **Переименовать вкладку…** / **Rename Tab…** on right-click) to pin or reset a custom title that stays fixed across `cd`. Press `Ctrl+Shift+F` (or choose Search Output in the command palette) to search terminal output and scrollback in the active pane (`Enter` jumps to the next match, `Shift+Enter` jumps to the previous match, `Aa` toggles case sensitivity, `.*` toggles PCRE2 regex mode, and `Escape` closes the bar and returns focus to the terminal). Press `Ctrl+Shift+L` or click `☀` / `☾` in the top bar to switch live between the Dark Zinc and Light Zinc themes across the window and all open terminal panes. `Ctrl+C` copies selected terminal text (or sends `SIGINT` when nothing is selected), `Ctrl+V` pastes text, clipboard images or copied files (`Ctrl+Shift+C` and `Ctrl+Shift+V` also work), `Ctrl+Click` on a relative `file:line` path or URL in terminal output opens it while leaving plain left-click drag selection unobstructed, and dragging files onto a terminal pane inserts their shell-escaped local paths or uploads them to `/tmp/` via `scp` in the background on SSH tabs. Right-clicking inside a terminal pane opens a context menu to copy, paste (with smart clipboard image routing), open or copy a URL or local `file:line` path under the cursor, open the working folder (**Открыть папку** / **Open Containing Folder**), forward selected text or recent output to another open tab, split or close panes, and clear scrollback, while `Shift + Right-Click` passes straight through to `tmux`, `vim` and other mouse-aware terminal apps. Right-clicking any tab button or sidebar row lets you rename or duplicate the tab, copy a local working directory, reconnect a remote tab, or close the tab or all other tabs. The `×` button closes its own tab, and `Alt+1` through `Alt+9` select tabs by position. Press `Ctrl+Shift+P` or `Ctrl+K` to open the command palette across open tabs, live remote `tmux` sessions, local profiles, SSH hosts, agents, theme/locale switching, Settings, About, Swarm Dashboard, Task Dispatch, and Handoff files (`Up`/`Down` selects, `Enter` runs, and `Escape` returns focus to the terminal). Press `Ctrl+,` to open Settings (live theme, bilingual Russian/English locale, font, scrollback lines, OSC 52 clipboard relay, and notification threshold) or view build diagnostics in About. Press `Ctrl+Shift+S` to toggle the **Swarm Dashboard** (`Escape` returns to the previous terminal) for live agent cards, 1-click approvals, and targeted prompt broadcasts, and press `Ctrl+Shift+N` to launch parallel **Task Dispatch** across isolated Git worktrees (`wt/<slug>`).

In a shell tab, open «Агенты» / «Agents» and choose **Claude Code** to run `claude` in that shell, or pick one of the compact `⎇` actions in the same menu to create a sibling Git worktree (`<repo>-wt-<agent>-<stamp>` on branch `wt/<agent>-<stamp>`) and launch the agent in a new tab. Local tabs inside a Git repository display their active branch (with `[wt]` for linked worktrees) and asynchronous dirty-stat badge (`+ins -del` or `±files`) in the sidebar and top-bar `Git ▾` button, and match branch names in sidebar search. Click `Git ▾` or press `Ctrl+Shift+G` to inspect modified and untracked files for the active local repository: clicking a tracked file opens `git diff` in a vertical split pane (or inline side-by-side Git Review), clicking an untracked file opens it externally, and inside a linked worktree **Remove clean worktree** verifies there are no uncommitted changes before removing the worktree and its merged `wt/*` branch. Install the [Claude Code CLI](https://code.claude.com/docs/en/quickstart) on the machine where the shell runs. When an agent pauses on a confirmation prompt or finishes a long run (including OSC 133 command completion) while the window is unfocused, AkhTerm highlights the tab and sends a desktop notification if `notify-send` is installed. Active agent turns show a live elapsed timer (`⏱`) on the tab and sidebar row, compact context usage (`ctx`) next to `MI`, and a per-tab and window-total `input · cache · output` breakdown in «Токены ▾» / «Tokens ▾» from local and remote SSH `AGY`, `Claude Code` and `Codex CLI` JSONL logs or remote `tmux` status lines.

## Local configuration

With a fresh profile, AkhTerm opens one local shell in `~`. SSH monitoring and startup connections are opt-in.

```sh
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/akhterm"
mkdir -p "$config_dir"
cp -n /usr/share/doc/akhterm/config.example.conf "$config_dir/config.conf"
```

If you built from source, copy [config.example.conf](config.example.conf) from the source directory. Set `AKHTERM_CONFIG` to use a different file. Paths can be absolute, `~`, or start with `~/`. Separate fields with `|`. Lines beginning with `#` are comments.

| Setting | Purpose |
| --- | --- |
| `local \| key \| title \| icon \| directory` | Adds a named shell tab to the menu. |
| `monitor \| key \| title \| icon \| directory` | Adds an `htop` tab. |
| `new-tab \| key` | Selects the profile used by `Ctrl+Shift+T`. |
| `startup \| key` | Opens a local profile on the first run. Repeat to set the order. |
| `startup-ssh \| host \| session` | Opens a remote tmux session on the first run. Omit the session field for a plain SSH tab. |
| `watch \| host` | Enables tmux and GPU polling for a host until changed in the SSH settings menu. |
| `shell \| /path/to/shell` | Overrides the shell from `$SHELL`. |
| `theme \| dark` | Sets the default Zinc theme (`dark` or `light`) before a manual toggle is saved. |
| `locale \| auto` | Sets the UI language (`auto`, `ru`, or `en`). |
| `font \| JetBrains Mono 11` | Sets the terminal Pango font description (`<family> <size>`). |
| `scrollback \| 10000` | Sets terminal scrollback history line limit (`100`–`100000`). |
| `osc52 \| on` | Enables or disables OSC 52 clipboard write relay (`on` or `off`). |
| `notify-secs \| 15` | Sets the minimum turn duration (seconds) for desktop notifications. |
| `briefs \| directory` | Adds a directory of Markdown briefs to the Handoff menu and uses it for saved terminal captures. |
| `mi-root \| directory` | Adds a fallback project directory for the Maintainability Index. |

AkhTerm stores tab layout, the selected theme (`theme-v1`) and SSH monitoring preferences under `$XDG_STATE_HOME/akhterm`, or `~/.local/state/akhterm`. Saved tabs and theme preferences take precedence over startup and `theme` entries in `config.conf`. Restoring a tab starts a fresh shell or SSH client. A remote tmux session retains its running processes.

Keep machine-specific configuration and saved state local. Use [config.example.conf](config.example.conf) as the shareable template.

## Run the tests

```sh
cargo test --offline --locked
```

## License

AkhTerm is licensed under the [MIT License](LICENSE). The Debian and Fedora packages include [notices for the system libraries](packaging/NOTICE).

The interface bundles [Geist 1.7.0](https://github.com/vercel/geist-font/releases/tag/1.7.0) under the [SIL Open Font License 1.1](assets/fonts/OFL-Geist.txt).
