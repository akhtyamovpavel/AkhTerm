use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::paths::{config_path, expand_home, home_dir};
use crate::tabs::{is_profile_key, is_ssh_host, Tab};
use crate::theme_mode::ThemeMode;

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

    fn parse(content: &str, path: &Path) -> Self {
        let mut config = Self::default();
        for (index, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<_> = line.split('|').map(str::trim).collect();
            if line.contains('\0') || !config.apply(&fields) {
                eprintln!(
                    "AkhTerm: invalid setting in {}:{}",
                    path.display(),
                    index + 1
                );
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
            ["startup-ssh", host] if is_ssh_host(host) => {
                self.startup.push(Tab::remote(host, None))
            }
            ["startup-ssh", host, tmux] if is_ssh_host(host) && !tmux.is_empty() => {
                self.startup.push(Tab::remote(host, Some(tmux)));
            }
            ["watch", host] if is_ssh_host(host) => self.watch.push((*host).to_owned()),
            ["shell", path] if Path::new(path).is_absolute() => self.shell = (*path).to_owned(),
            ["briefs", path] => self.briefs_dir = Some(PathBuf::from(expand_home(path))),
            ["mi-root", path] => self.mi_root = Some(expand_home(path)),
            ["theme", mode] => return self.set_theme(mode),
            _ => return false,
        }
        true
    }

    fn set_theme(&mut self, mode: &str) -> bool {
        let Some(parsed) = ThemeMode::parse_config(mode) else {
            return false;
        };
        self.theme = Some(parsed);
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
        let mut paths: Vec<_> = self
            .profiles
            .iter()
            .map(|profile| profile.directory.clone())
            .collect();
        paths.extend(self.mi_root.iter().cloned());
        let home = home_dir();
        paths.retain(|path| path != &home && valid_directory(path));
        paths.sort();
        paths.dedup();
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_config_uses_local_shell_without_ssh() {
        let config = WorkspaceConfig::default();
        assert_eq!(config.new_tab(), Tab::local("shell"));
        assert!(config.startup.is_empty() && config.watch.is_empty());
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
        assert_eq!(
            config.startup,
            [
                Tab::local("work"),
                Tab::remote("devbox", Some("work queue"))
            ]
        );
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
}
