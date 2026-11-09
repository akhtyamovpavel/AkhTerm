use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub fn home_dir() -> String {
    env::var("HOME").unwrap_or_else(|_| "/".to_owned())
}

pub fn expand_home(path: &str) -> String {
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return Path::new(&home_dir())
            .join(rest)
            .to_string_lossy()
            .into_owned();
    }
    path.to_owned()
}

pub fn shorten_home(path: &str) -> String {
    match Path::new(path).strip_prefix(home_dir()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.to_owned(),
    }
}

fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| Path::new(&home_dir()).join(fallback))
        .join("akhterm")
}

pub fn state_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state")
}

pub fn config_path() -> PathBuf {
    if let Some(path) = env::var_os("AKHTERM_CONFIG").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    xdg_dir("XDG_CONFIG_HOME", ".config").join("config.conf")
}

pub fn write_state_file(path: &Path, content: &str) -> io::Result<()> {
    fs::create_dir_all(path.parent().expect("state directory"))?;
    let staged = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = stage_file(&staged, content).and_then(|()| fs::rename(&staged, path));
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn stage_file(path: &Path, content: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_bytes())
}
