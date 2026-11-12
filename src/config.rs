use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::i18n::UiLocale;
use crate::paths::{config_path, expand_home, home_dir};
use crate::tabs::{is_profile_key, is_ssh_host, Tab};
use crate::theme_mode::ThemeMode;

pub const DEFAULT_FONT_DESC: &str = "DejaVu Sans Mono 10.5";
pub const DEFAULT_SCROLLBACK_LINES: usize = 15_000;
pub const MIN_SCROLLBACK_LINES: usize = 500;
pub const MAX_SCROLLBACK_LINES: usize = 500_000;
pub const DEFAULT_NOTIFY_SECS: u64 = 15;
pub const MAX_NOTIFY_SECS: u64 = 3_600;

const MANAGED_PREF_KEYS: [&str; 6] = [
    "theme",
    "locale",
    "font",
    "scrollback",
    "osc52",
    "notify-secs",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserPrefs {
    pub theme: ThemeMode,
    pub locale: UiLocale,
    pub font_desc: String,
    pub scrollback_lines: usize,
    pub osc52_enabled: bool,
    pub notify_secs: u64,
}

impl Default for UserPrefs {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            locale: UiLocale::Auto,
            font_desc: DEFAULT_FONT_DESC.to_owned(),
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            osc52_enabled: true,
            notify_secs: DEFAULT_NOTIFY_SECS,
        }
    }
}

#[derive(Clone)]
pub struct LocalProfile {
    pub key: String,
    pub title: String,
    pub icon: String,
    pub directory: String,
    pub monitor: bool,
}

impl LocalProfile {
    fn shell() -> Self {
        Self {
            key: "shell".to_owned(),
            title: "Терминал".to_owned(),
            icon: "⌂".to_owned(),
            directory: home_dir(),
            monitor: false,
        }
    }

    pub fn resolve_cwd(&self, saved: &str) -> String {
        if valid_directory(saved) {
            return saved.to_owned();
        }
        if valid_directory(&self.directory) {
            return self.directory.clone();
        }
        home_dir()
    }
}

fn valid_directory(path: &str) -> bool {
    let path = Path::new(path);
    path.is_absolute() && path.is_dir()
}

pub struct WorkspaceConfig {
    pub profiles: Vec<LocalProfile>,
    pub startup: Vec<Tab>,
    pub watch: Vec<String>,
    pub shell: String,
    pub briefs_dir: Option<PathBuf>,
    pub mi_root: Option<String>,
    pub theme: Option<ThemeMode>,
    pub font_desc: String,
    pub scrollback_lines: usize,
    pub locale: UiLocale,
    pub osc52_enabled: bool,
    pub notify_secs: u64,
    new_tab: String,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            profiles: vec![LocalProfile::shell()],
            startup: Vec::new(),
            watch: Vec::new(),
            shell: shell_path(),
            briefs_dir: None,
            mi_root: None,
            theme: None,
            font_desc: DEFAULT_FONT_DESC.to_owned(),
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            locale: UiLocale::Auto,
            osc52_enabled: true,
            notify_secs: DEFAULT_NOTIFY_SECS,
            new_tab: "shell".to_owned(),
        }
    }
}

fn shell_path() -> String {
    env::var("SHELL")
        .ok()
        .filter(|path| Path::new(path).is_absolute() && Path::new(path).is_file())
        .unwrap_or_else(|| "/bin/sh".to_owned())
}

pub fn parse_bool_flag(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" => Some(true),
        "off" | "false" | "0" => Some(false),
        _ => None,
    }
}

impl WorkspaceConfig {
    pub fn load() -> Self {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(content) => Self::parse(&content, &path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                eprintln!("AkhTerm: cannot read {}: {error}", path.display());
                Self::default()
            }
        }
    }

    pub fn parse(content: &str, path: &Path) -> Self {
        let mut config = Self::default();
        for (index, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<_> = line.split('|').map(str::trim).collect();
            if line.contains('\0') || !config.apply(&fields) {
                eprintln!("AkhTerm: invalid setting in {}:{}", path.display(), index + 1);
            }
        }
        config
    }

    fn apply(&mut self, fields: &[&str]) -> bool {
        match fields {
            [kind @ ("local" | "monitor"), key, title, icon, cwd] if is_profile_key(key) => {
                self.add_profile(kind == &"monitor", key, title, icon, cwd);
            }
            ["new-tab", key] if is_profile_key(key) => self.new_tab = (*key).to_owned(),
            ["startup", key] if is_profile_key(key) => self.startup.push(Tab::local(key)),
            ["startup-ssh", host] if is_ssh_host(host) => self.startup.push(Tab::remote(host, None)),
            ["startup-ssh", host, tmux] if is_ssh_host(host) && !tmux.is_empty() => {
                self.startup.push(Tab::remote(host, Some(tmux)));
            }
            ["watch", host] if is_ssh_host(host) => self.watch.push((*host).to_owned()),
            ["shell", path] if Path::new(path).is_absolute() => self.shell = (*path).to_owned(),
            ["briefs", path] => self.briefs_dir = Some(PathBuf::from(expand_home(path))),
            ["mi-root", path] => self.mi_root = Some(expand_home(path)),
            _ => return self.apply_pref_field(fields),
        }
        true
    }

    fn apply_pref_field(&mut self, fields: &[&str]) -> bool {
        match fields {
            ["theme", mode] => self.set_theme(mode),
            ["font", desc] => self.set_font(desc),
            ["scrollback", raw] => self.set_scrollback(raw),
            ["locale", raw] => self.set_locale_pref(raw),
            ["osc52", raw] => self.set_osc52(raw),
            ["notify-secs", raw] => self.set_notify_secs(raw),
            _ => false,
        }
    }

    fn set_theme(&mut self, mode: &str) -> bool {
        let Some(parsed) = ThemeMode::parse_config(mode) else {
            return false;
        };
        self.theme = Some(parsed);
        true
    }

    fn set_font(&mut self, desc: &str) -> bool {
        let trimmed = desc.trim();
        if trimmed.is_empty() {
            return false;
        }
        self.font_desc = trimmed.to_owned();
        true
    }

    fn set_scrollback(&mut self, raw: &str) -> bool {
        let Ok(lines) = raw.trim().parse::<usize>() else {
            return false;
        };
        self.scrollback_lines = lines.clamp(MIN_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES);
        true
    }

    fn set_locale_pref(&mut self, raw: &str) -> bool {
        let Some(loc) = UiLocale::from_config(raw) else {
            return false;
        };
        self.locale = loc;
        true
    }

    fn set_osc52(&mut self, raw: &str) -> bool {
        let Some(enabled) = parse_bool_flag(raw) else {
            return false;
        };
        self.osc52_enabled = enabled;
        true
    }

    fn set_notify_secs(&mut self, raw: &str) -> bool {
        let Ok(secs) = raw.trim().parse::<u64>() else {
            return false;
        };
        self.notify_secs = secs.min(MAX_NOTIFY_SECS);
        true
    }

    fn add_profile(&mut self, monitor: bool, key: &str, title: &str, icon: &str, cwd: &str) {
        let profile = LocalProfile {
            key: key.to_owned(),
            title: title.to_owned(),
            icon: icon.to_owned(),
            directory: expand_home(cwd),
            monitor,
        };
        self.profiles.retain(|current| current.key != key);
        self.profiles.push(profile);
    }

    pub fn profile(&self, key: &str) -> LocalProfile {
        self.profiles
            .iter()
            .find(|profile| profile.key == key)
            .cloned()
            .unwrap_or_else(LocalProfile::shell)
    }

    pub fn new_tab(&self) -> Tab {
        Tab::local(&self.profile(&self.new_tab).key)
    }

    pub fn mi_directories(&self) -> Vec<String> {
        let mut paths: Vec<_> = self.profiles.iter().map(|p| p.directory.clone()).collect();
        paths.extend(self.mi_root.iter().cloned());
        let home = home_dir();
        paths.retain(|path| path != &home && valid_directory(path));
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn to_user_prefs(&self, active_theme: ThemeMode) -> UserPrefs {
        UserPrefs {
            theme: self.theme.unwrap_or(active_theme),
            locale: self.locale,
            font_desc: self.font_desc.clone(),
            scrollback_lines: self.scrollback_lines,
            osc52_enabled: self.osc52_enabled,
            notify_secs: self.notify_secs,
        }
    }

    pub fn apply_user_prefs(&mut self, prefs: &UserPrefs) {
        let norm = normalize_prefs(prefs);
        self.theme = Some(norm.theme);
        self.locale = norm.locale;
        self.font_desc = norm.font_desc;
        self.scrollback_lines = norm.scrollback_lines;
        self.osc52_enabled = norm.osc52_enabled;
        self.notify_secs = norm.notify_secs;
    }
}

pub fn normalize_prefs(prefs: &UserPrefs) -> UserPrefs {
    let trimmed_font = prefs.font_desc.trim();
    let font_desc = if trimmed_font.is_empty() {
        DEFAULT_FONT_DESC.to_owned()
    } else {
        trimmed_font.to_owned()
    };
    UserPrefs {
        theme: prefs.theme,
        locale: prefs.locale,
        font_desc,
        scrollback_lines: prefs.scrollback_lines.clamp(MIN_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES),
        osc52_enabled: prefs.osc52_enabled,
        notify_secs: prefs.notify_secs.min(MAX_NOTIFY_SECS),
    }
}

fn format_pref_line(key: &str, prefs: &UserPrefs) -> String {
    match key {
        "theme" => format!("theme | {}", prefs.theme.as_str()),
        "locale" => format!("locale | {}", prefs.locale.as_config_str()),
        "font" => format!("font | {}", prefs.font_desc),
        "scrollback" => format!("scrollback | {}", prefs.scrollback_lines),
        "osc52" => format!("osc52 | {}", if prefs.osc52_enabled { "on" } else { "off" }),
        _ => format!("notify-secs | {}", prefs.notify_secs),
    }
}

fn managed_key_index(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('|')?;
    let key = key.trim();
    MANAGED_PREF_KEYS.iter().position(|&k| k == key)
}

pub fn update_config_content(existing: &str, prefs: &UserPrefs) -> String {
    let norm = normalize_prefs(prefs);
    let mut seen = [false; 6];
    let mut lines = Vec::new();
    for raw_line in existing.lines() {
        if let Some(idx) = managed_key_index(raw_line) {
            if !seen[idx] {
                seen[idx] = true;
                lines.push(format_pref_line(MANAGED_PREF_KEYS[idx], &norm));
            }
        } else {
            lines.push(raw_line.to_owned());
        }
    }
    for (idx, &key) in MANAGED_PREF_KEYS.iter().enumerate() {
        if !seen[idx] {
            lines.push(format_pref_line(key, &norm));
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn write_config_atomic(path: &Path, content: &str) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let staged = path.with_extension(format!("conf.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&staged)?;
    if let Err(err) = file.write_all(content.as_bytes()).and_then(|()| fs::rename(&staged, path)) {
        let _ = fs::remove_file(&staged);
        return Err(err);
    }
    Ok(())
}

pub fn save_user_preferences(path: &Path, prefs: &UserPrefs) -> io::Result<()> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let updated = update_config_content(&existing, prefs);
    write_config_atomic(path, &updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_config_uses_local_shell_without_ssh_and_has_v1_defaults() {
        let config = WorkspaceConfig::default();
        assert_eq!(config.new_tab(), Tab::local("shell"));
        assert!(config.startup.is_empty() && config.watch.is_empty());
        assert_eq!(config.font_desc, DEFAULT_FONT_DESC);
        assert_eq!(config.scrollback_lines, 15_000);
        assert_eq!(config.locale, UiLocale::Auto);
        assert!(config.osc52_enabled);
        assert_eq!(config.notify_secs, 15);
    }

    #[test]
    fn config_preserves_startup_order_and_requires_explicit_watch() {
        let text = concat!(
            "local | work | Workspace | ◇ | /work folder\n",
            "new-tab | work\n",
            "startup | work\n",
            "startup-ssh | devbox | work queue\n",
            "watch | monitored\n",
        );
        let config = WorkspaceConfig::parse(text, Path::new("test.conf"));
        assert_eq!(config.profile("work").directory, "/work folder");
        assert_eq!(config.new_tab(), Tab::local("work"));
        assert_eq!(config.startup, [Tab::local("work"), Tab::remote("devbox", Some("work queue"))]);
        assert_eq!(config.watch, ["monitored"]);
    }

    #[test]
    fn parses_optional_theme_setting_and_rejects_unknown_modes() {
        let light = WorkspaceConfig::parse("theme | light\n", Path::new("test.conf"));
        assert_eq!(light.theme, Some(ThemeMode::Light));
        let dark = WorkspaceConfig::parse("theme | dark\n", Path::new("test.conf"));
        assert_eq!(dark.theme, Some(ThemeMode::Dark));
        let invalid = WorkspaceConfig::parse("theme | solarized\n", Path::new("test.conf"));
        assert_eq!(invalid.theme, None);
    }

    #[test]
    fn parses_v1_pipe_preferences_with_clamping_and_validation() {
        let text = concat!(
            "font | JetBrains Mono 11\n",
            "scrollback | 999999\n",
            "locale | ru\n",
            "osc52 | off\n",
            "notify-secs | 9999\n",
        );
        let cfg = WorkspaceConfig::parse(text, Path::new("test.conf"));
        assert_eq!(cfg.font_desc, "JetBrains Mono 11");
        assert_eq!(cfg.scrollback_lines, MAX_SCROLLBACK_LINES);
        assert_eq!(cfg.locale, UiLocale::Ru);
        assert!(!cfg.osc52_enabled);
        assert_eq!(cfg.notify_secs, MAX_NOTIFY_SECS);

        let low = WorkspaceConfig::parse("scrollback | 10\nosc52 | 1\nnotify-secs | 0\n", Path::new("test.conf"));
        assert_eq!(low.scrollback_lines, MIN_SCROLLBACK_LINES);
        assert!(low.osc52_enabled);
        assert_eq!(low.notify_secs, 0);

        let bad = WorkspaceConfig::parse("font |   \nscrollback | abc\nlocale | fr\nosc52 | maybe\n", Path::new("test.conf"));
        assert_eq!(bad.font_desc, DEFAULT_FONT_DESC);
        assert_eq!(bad.scrollback_lines, DEFAULT_SCROLLBACK_LINES);
        assert_eq!(bad.locale, UiLocale::Auto);
        assert!(bad.osc52_enabled);
    }

    #[test]
    fn save_user_preferences_preserves_comments_and_profiles_verbatim() {
        let dir = std::env::temp_dir().join(format!("akhterm_cfg_rt_{}", std::process::id()));
        let path = dir.join("config.conf");
        let _ = fs::remove_file(&path);
        let initial = concat!(
            "# Top comment\n",
            "local | work | Workspace | ◇ | ~/projects\n",
            "monitor | mon | Top | 📈 | ~\n",
            "startup | work\n",
            "startup-ssh | devbox | main\n",
            "watch | devbox\n",
            "briefs | ~/notes\n",
            "mi-root | ~/code\n",
            "# theme | dark\n",
            "theme | dark\n",
        );
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, initial).unwrap();

        let prefs = UserPrefs {
            theme: ThemeMode::Light,
            locale: UiLocale::En,
            font_desc: "IBM Plex Mono 11".to_owned(),
            scrollback_lines: 30_000,
            osc52_enabled: false,
            notify_secs: 45,
        };
        save_user_preferences(&path, &prefs).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# Top comment\n"));
        assert!(saved.contains("local | work | Workspace | ◇ | ~/projects\n"));
        assert!(saved.contains("monitor | mon | Top | 📈 | ~\n"));
        assert!(saved.contains("startup | work\n"));
        assert!(saved.contains("startup-ssh | devbox | main\n"));
        assert!(saved.contains("watch | devbox\n"));
        assert!(saved.contains("briefs | ~/notes\n"));
        assert!(saved.contains("mi-root | ~/code\n"));
        assert!(saved.contains("# theme | dark\n"));

        let reparsed = WorkspaceConfig::parse(&saved, &path);
        assert_eq!(reparsed.theme, Some(ThemeMode::Light));
        assert_eq!(reparsed.locale, UiLocale::En);
        assert_eq!(reparsed.font_desc, "IBM Plex Mono 11");
        assert_eq!(reparsed.scrollback_lines, 30_000);
        assert!(!reparsed.osc52_enabled);
        assert_eq!(reparsed.notify_secs, 45);
        assert_eq!(reparsed.watch, ["devbox"]);

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn config_module_maintains_baseline_mi() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("config.rs"));
        assert!(mi >= 60.49, "config.rs MI {mi:.2} < 60.49");
    }
}
