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

fn has_unmatched_closer(s: &str, open: char, close: char) -> bool {
    s.matches(close).count() > s.matches(open).count()
}

pub fn strip_link_punctuation(raw: &str) -> &str {
    let mut cur = raw.trim();
    while let Some(ch) = cur.chars().next_back() {
        let drop_ch = match ch {
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' => true,
            ')' => has_unmatched_closer(cur, '(', ')'),
            ']' => has_unmatched_closer(cur, '[', ']'),
            '}' => has_unmatched_closer(cur, '{', '}'),
            _ => false,
        };
        if !drop_ch {
            break;
        }
        cur = &cur[..cur.len() - ch.len_utf8()];
    }
    cur
}

fn parse_line_num(part: &str) -> Option<u32> {
    (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        .then(|| part.parse::<u32>().ok())
        .flatten()
}

pub fn split_path_line_col(raw: &str) -> (&str, Option<u32>, Option<u32>) {
    let clean = strip_link_punctuation(raw);
    let Some((head, tail)) = clean.rsplit_once(':') else {
        return (clean, None, None);
    };
    let Some(n1) = parse_line_num(tail) else {
        return (clean, None, None);
    };
    if let Some((head2, line_s)) = head.rsplit_once(':') {
        if let Some(n2) = parse_line_num(line_s) {
            return (head2, Some(n2), Some(n1));
        }
    }
    (head, Some(n1), None)
}

pub fn strip_anchor_or_line(path_str: &str) -> &str {
    let without_frag = path_str.split('#').next().unwrap_or(path_str);
    let (base, line, _) = split_path_line_col(without_frag);
    if line.is_some() { base } else { without_frag }
}

pub fn clean_joined_path(base: &Path, rel: &str) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in base.join(rel).components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
