//! OpenSSH `~/.ssh/config` and `Include` parser with lightweight mtime/size fingerprinting.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::paths::{home_dir, state_dir};
use crate::proc_radar::SshHostEntry;
use crate::tabs::{is_ssh_host, Tab};

const MAX_INCLUDE_DEPTH: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SshConfigFingerprint {
    pub entries: Vec<(PathBuf, Option<SystemTime>, u64)>,
}

impl SshConfigFingerprint {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

struct ParseCtx<'a> {
    ssh_dir: &'a Path,
    home: &'a str,
    watched_enabled: &'a HashSet<String>,
    visited: HashSet<PathBuf>,
    watched_paths: Vec<PathBuf>,
    entries: Vec<SshHostEntry>,
}

struct HostBlock {
    aliases: Vec<String>,
    hostname: String,
    user: String,
}

impl HostBlock {
    fn new() -> Self {
        Self { aliases: Vec::new(), hostname: String::new(), user: String::new() }
    }

    fn flush(&mut self, watched: &HashSet<String>, out: &mut Vec<SshHostEntry>) {
        for alias in self.aliases.drain(..) {
            if !is_tracked_alias(&alias, out) {
                continue;
            }
            let enabled = watched.contains(&alias);
            out.push(SshHostEntry {
                alias,
                hostname: self.hostname.clone(),
                user: self.user.clone(),
                enabled,
            });
        }
        self.hostname.clear();
        self.user.clear();
    }
}

fn is_tracked_alias(alias: &str, existing: &[SshHostEntry]) -> bool {
    is_ssh_host(alias)
        && !alias.starts_with('!')
        && !alias.contains('*')
        && !alias.contains('?')
        && !matches!(alias, "github.com" | "gitlab.com" | "bitbucket.org")
        && !existing.iter().any(|e| e.alias == alias)
}

fn stat_path(path: &Path) -> (Option<SystemTime>, u64) {
    let Ok(meta) = fs::metadata(path) else {
        return (None, 0);
    };
    (meta.modified().ok(), meta.len())
}

pub fn has_fingerprint_changed(fp: &SshConfigFingerprint) -> bool {
    fp.entries.iter().any(|(path, mtime, len)| stat_path(path) != (*mtime, *len))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn read_saved_enabled(conf_path: &Path) -> Option<HashSet<String>> {
    let txt = fs::read_to_string(conf_path).ok()?;
    let set = txt
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    Some(set)
}

fn split_ssh_tokens(raw_line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in raw_line.chars() {
        if !step_token_char(ch, &mut tokens, &mut cur, &mut in_quotes) {
            break;
        }
    }
    flush_token(&mut tokens, &mut cur);
    tokens
}

fn step_token_char(
    ch: char,
    tokens: &mut Vec<String>,
    cur: &mut String,
    in_quotes: &mut bool,
) -> bool {
    if *in_quotes {
        if ch == '"' { *in_quotes = false; } else { cur.push(ch); }
        return true;
    }
    match ch {
        '#' => return false,
        '"' => *in_quotes = true,
        c if c.is_whitespace() => flush_token(tokens, cur),
        '=' if tokens.is_empty() && !cur.is_empty() => flush_token(tokens, cur),
        '=' if tokens.len() == 1 && cur.is_empty() => {}
        c => cur.push(c),
    }
    true
}

fn flush_token(tokens: &mut Vec<String>, cur: &mut String) {
    if !cur.is_empty() {
        tokens.push(std::mem::take(cur));
    }
}

fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
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

fn resolve_include_pattern(ssh_dir: &Path, home: &str, raw: &str) -> PathBuf {
    let joined = if raw == "~" {
        PathBuf::from(home)
    } else if let Some(rest) = raw.strip_prefix("~/") {
        Path::new(home).join(rest)
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        ssh_dir.join(raw)
    };
    clean_path(&joined)
}

fn has_glob_chars(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

fn glob_match(pat: &str, text: &str) -> bool {
    if text.starts_with('.') && !pat.starts_with('.') {
        return false;
    }
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_chars(&p, &t)
}

fn glob_match_chars(p: &[char], t: &[char]) -> bool {
    let (mut pi, mut ti, mut star_pi, mut star_ti) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn matching_dir_entries(dir: &Path, pat: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut matched: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| glob_match(pat, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    matched.sort();
    matched
}

fn expand_glob_component(candidates: Vec<PathBuf>, seg: &str, watched: &mut Vec<PathBuf>) -> Vec<PathBuf> {
    let mut next = Vec::new();
    for dir in candidates {
        push_unique_path(watched, dir.clone());
        next.extend(matching_dir_entries(&dir, seg));
    }
    next
}

fn expand_include_pattern(pattern: &Path, watched: &mut Vec<PathBuf>) -> Vec<PathBuf> {
    let pat_str = pattern.to_string_lossy();
    if !has_glob_chars(&pat_str) {
        if let Some(parent) = pattern.parent() {
            push_unique_path(watched, parent.to_path_buf());
        }
        push_unique_path(watched, pattern.to_path_buf());
        return pattern.is_file().then(|| vec![pattern.to_path_buf()]).unwrap_or_default();
    }
    let mut candidates = vec![PathBuf::new()];
    for comp in pattern.components() {
        candidates = step_pattern_component(candidates, comp, watched);
    }
    collect_matched_files(candidates, watched)
}

fn step_pattern_component(
    mut candidates: Vec<PathBuf>,
    comp: Component<'_>,
    watched: &mut Vec<PathBuf>,
) -> Vec<PathBuf> {
    match comp {
        Component::Normal(seg) => {
            let s = seg.to_string_lossy();
            if has_glob_chars(&s) {
                expand_glob_component(candidates, &s, watched)
            } else {
                candidates.iter_mut().for_each(|c| c.push(seg));
                candidates
            }
        }
        Component::CurDir | Component::ParentDir => candidates,
        other => {
            candidates.iter_mut().for_each(|c| c.push(other.as_os_str()));
            candidates
        }
    }
}

fn collect_matched_files(candidates: Vec<PathBuf>, watched: &mut Vec<PathBuf>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in candidates {
        if let Some(parent) = path.parent() {
            push_unique_path(watched, parent.to_path_buf());
        }
        if path.is_file() {
            push_unique_path(watched, path.clone());
            files.push(path);
        }
    }
    files
}

fn parse_include_line(patterns: &[String], depth: usize, ctx: &mut ParseCtx<'_>) {
    for raw in patterns {
        let pat = resolve_include_pattern(ctx.ssh_dir, ctx.home, raw);
        for target in expand_include_pattern(&pat, &mut ctx.watched_paths) {
            parse_ssh_file(&target, depth + 1, ctx);
        }
    }
}

fn set_block_field(has_aliases: bool, field: &mut String, tokens: &[String]) {
    if has_aliases && field.is_empty() {
        if let Some(val) = tokens.get(1) {
            field.clone_from(val);
        }
    }
}

fn apply_host_line(tokens: &[String], block: &mut HostBlock, depth: usize, ctx: &mut ParseCtx<'_>) {
    let key = &tokens[0];
    if key.eq_ignore_ascii_case("Host") {
        block.flush(ctx.watched_enabled, &mut ctx.entries);
        block.aliases.extend(tokens[1..].iter().cloned());
    } else if key.eq_ignore_ascii_case("Match") {
        block.flush(ctx.watched_enabled, &mut ctx.entries);
    } else if key.eq_ignore_ascii_case("Include") {
        block.flush(ctx.watched_enabled, &mut ctx.entries);
        parse_include_line(&tokens[1..], depth, ctx);
    } else if key.eq_ignore_ascii_case("HostName") {
        set_block_field(!block.aliases.is_empty(), &mut block.hostname, tokens);
    } else if key.eq_ignore_ascii_case("User") {
        set_block_field(!block.aliases.is_empty(), &mut block.user, tokens);
    }
}

fn parse_ssh_file(path: &Path, depth: usize, ctx: &mut ParseCtx<'_>) {
    if depth >= MAX_INCLUDE_DEPTH {
        return;
    }
    let canon = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !ctx.visited.insert(canon) {
        return;
    }
    push_unique_path(&mut ctx.watched_paths, path.to_path_buf());
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let mut block = HostBlock::new();
    for raw_line in content.lines() {
        let tokens = split_ssh_tokens(raw_line);
        if !tokens.is_empty() {
            apply_host_line(&tokens, &mut block, depth, ctx);
        }
    }
    block.flush(ctx.watched_enabled, &mut ctx.entries);
}

fn append_extra_hosts(
    watch: &[String],
    startup: &[Tab],
    watched_enabled: &HashSet<String>,
    entries: &mut Vec<SshHostEntry>,
) {
    let startup_hosts = startup.iter().filter_map(|tab| match tab {
        Tab::Remote(remote) => Some(&remote.host),
        Tab::Local { .. } => None,
    });
    for alias in watch.iter().chain(startup_hosts) {
        if entries.iter().any(|entry| entry.alias == *alias) {
            continue;
        }
        entries.push(SshHostEntry {
            alias: alias.clone(),
            hostname: String::new(),
            user: String::new(),
            enabled: watched_enabled.contains(alias),
        });
    }
}

fn build_fingerprint(paths: Vec<PathBuf>) -> SshConfigFingerprint {
    let entries = paths
        .into_iter()
        .map(|p| {
            let (mtime, len) = stat_path(&p);
            (p, mtime, len)
        })
        .collect();
    SshConfigFingerprint { entries }
}

pub fn load_ssh_hosts_from(
    home: &str,
    ssh_dir: &Path,
    state_dir: &Path,
    watch: &[String],
    startup: &[Tab],
) -> (Vec<SshHostEntry>, SshConfigFingerprint) {
    let watched_enabled = read_saved_enabled(&state_dir.join("servers.conf"))
        .unwrap_or_else(|| watch.iter().cloned().collect());
    let mut watched_paths = Vec::new();
    push_unique_path(&mut watched_paths, ssh_dir.to_path_buf());
    let main_cfg = ssh_dir.join("config");
    push_unique_path(&mut watched_paths, main_cfg.clone());
    let mut ctx = ParseCtx {
        ssh_dir,
        home,
        watched_enabled: &watched_enabled,
        visited: HashSet::new(),
        watched_paths,
        entries: Vec::new(),
    };
    parse_ssh_file(&main_cfg, 0, &mut ctx);
    append_extra_hosts(watch, startup, &watched_enabled, &mut ctx.entries);
    (ctx.entries, build_fingerprint(ctx.watched_paths))
}

pub fn load_ssh_hosts_with_fingerprint(
    watch: &[String],
    startup: &[Tab],
) -> (Vec<SshHostEntry>, SshConfigFingerprint) {
    let home = home_dir();
    let ssh_dir = PathBuf::from(&home).join(".ssh");
    load_ssh_hosts_from(&home, &ssh_dir, &state_dir(), watch, startup)
}

#[allow(dead_code)]
pub fn load_ssh_hosts(watch: &[String], startup: &[Tab]) -> Vec<SshHostEntry> {
    load_ssh_hosts_with_fingerprint(watch, startup).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    struct TempEnv {
        root: PathBuf,
        home: String,
        ssh_dir: PathBuf,
        state_dir: PathBuf,
    }

    impl TempEnv {
        fn new(tag: &str) -> Self {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!("akhterm_ssh_cfg_{tag}_{}_{n}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let ssh_dir = root.join(".ssh");
            let state_dir = root.join("state");
            fs::create_dir_all(&ssh_dir).unwrap();
            fs::create_dir_all(&state_dir).unwrap();
            let home = root.to_string_lossy().into_owned();
            Self { root, home, ssh_dir, state_dir }
        }

        fn write(&self, rel: &str, body: &str) {
            let p = self.ssh_dir.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, body).unwrap();
        }
    }

    impl Drop for TempEnv {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn write_fixture_includes(env: &TempEnv) {
        env.write(
            "config",
            "Include config.d/*.conf\nInclude ~/.ssh/extra/hosts-?.conf\nInclude cycle_a.conf\nHost root-box\n  HostName 10.0.0.1\n",
        );
        env.write("config.d/01-a.conf", "Host box-a\n  HostName 10.0.1.1\n");
        env.write("config.d/02-b.conf", "Host box-b\n  HostName 10.0.1.2\n");
        env.write("config.d/.hidden.conf", "Host hidden-box\n  HostName 10.0.1.9\n");
        env.write("extra/hosts-1.conf", "Host box-1\n  User ml\n");
        env.write("extra/hosts-12.conf", "Host box-12\n");
        env.write("cycle_a.conf", "Include cycle_b.conf\nHost cyc-a\n");
        env.write("cycle_b.conf", "Include cycle_a.conf\nInclude config\nHost cyc-b\n");
    }

    #[test]
    fn parses_host_blocks_and_ignores_wildcards() {
        let env = TempEnv::new("hosts");
        let cfg = "# comment\nHost gpu01 gpu-main *.internal !bastion github.com gitlab.com bitbucket.org\n  HostName 10.1.2.3\n  User pavel\nHost *\n  User fallback\nHost cpu01 # trailing\n  HostName=10.1.2.4\n  User = alice\n";
        env.write("config", cfg);
        let watch = vec!["gpu-main".to_owned(), "watch-only".to_owned()];
        let startup = vec![Tab::remote("startup-box", None)];
        let (hosts, _) = load_ssh_hosts_from(&env.home, &env.ssh_dir, &env.state_dir, &watch, &startup);
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, vec!["gpu01", "gpu-main", "cpu01", "watch-only", "startup-box"]);
        assert_eq!((hosts[0].hostname.as_str(), hosts[0].user.as_str(), hosts[0].enabled), ("10.1.2.3", "pavel", false));
        assert_eq!((hosts[1].enabled, hosts[2].hostname.as_str(), hosts[2].user.as_str()), (true, "10.1.2.4", "alice"));
    }

    #[test]
    fn parses_include_globs_tilde_and_cycle_guard() {
        let env = TempEnv::new("include");
        write_fixture_includes(&env);
        let (hosts, _) = load_ssh_hosts_from(&env.home, &env.ssh_dir, &env.state_dir, &[], &[]);
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, vec!["box-a", "box-b", "box-1", "cyc-b", "cyc-a", "root-box"]);
    }

    #[test]
    fn fingerprint_detects_new_and_edited_include_files() {
        let env = TempEnv::new("fp");
        env.write("config", "Include config.d/*.conf\nHost alpha\n");
        env.write("config.d/01.conf", "Host beta\n");

        let (h1, fp1) = load_ssh_hosts_from(&env.home, &env.ssh_dir, &env.state_dir, &[], &[]);
        assert_eq!(h1.len(), 2);
        assert!(!has_fingerprint_changed(&fp1));

        std::thread::sleep(std::time::Duration::from_millis(20));
        env.write("config.d/02.conf", "Host gamma\n");
        assert!(has_fingerprint_changed(&fp1));

        let (h2, fp2) = load_ssh_hosts_from(&env.home, &env.ssh_dir, &env.state_dir, &[], &[]);
        assert_eq!(h2.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(), vec!["beta", "gamma", "alpha"]);
        assert!(!has_fingerprint_changed(&fp2));

        std::thread::sleep(std::time::Duration::from_millis(20));
        env.write("config.d/02.conf", "Host gamma\n  HostName 10.2.3.4\n");
        assert!(has_fingerprint_changed(&fp2));
    }
}
