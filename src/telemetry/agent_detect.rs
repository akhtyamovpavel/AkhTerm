//! Accurate CLI agent detection across `/proc` argv trees, runtime wrappers, and terminal titles.

use crate::ssh_detect::{exe_basename, ProcEntry};

const EXE_AGENTS: &[(&str, &str)] = &[
    ("deepseek", "DeepSeek"), ("deepseek-cli", "DeepSeek"), ("deepseek_cli", "DeepSeek"),
    ("agy", "AGY"), ("antigravity", "AGY"), ("antigravity-cli", "AGY"), ("antigravity_cli", "AGY"),
    ("codex", "Codex"), ("codex-cli", "Codex"), ("codex_cli", "Codex"), ("dsh", "DSH"),
    ("kimi", "Kimi"), ("kimi-cli", "Kimi"), ("kimi_cli", "Kimi"),
    ("claude", "Claude"), ("claude-code", "Claude"), ("claude_code", "Claude"),
];

const INSTALL_PATTERNS: &[(&str, &str)] = &[
    ("/antigravity-cli/", "AGY"), ("/antigravity/", "AGY"), ("@openai/codex", "Codex"), ("/.codex/", "Codex"),
    ("@anthropic-ai/claude-code", "Claude"), ("/claude/versions/", "Claude"), ("/.claude/local/", "Claude"),
    ("/.kimi-code/", "Kimi"), ("/kimi-cli/", "Kimi"), ("/deepseek-cli/", "DeepSeek"),
];

pub fn classify_agent_program(raw: &str) -> Option<&'static str> {
    let token = strip_pkg_version(raw.trim());
    if let Some(agent) = classify_agent_exe(&exe_basename(token)) {
        return Some(agent);
    }
    (token.contains('/') || token.starts_with('@'))
        .then(|| find_table_match(INSTALL_PATTERNS, &token.to_ascii_lowercase(), true))
        .flatten()
}

fn classify_agent_exe(exe: &str) -> Option<&'static str> {
    find_table_match(EXE_AGENTS, exe, false)
}

fn find_table_match(table: &[(&str, &'static str)], needle: &str, substr: bool) -> Option<&'static str> {
    table
        .iter()
        .find(|&&(k, _)| if substr { needle.contains(k) } else { needle == k })
        .map(|&(_, v)| v)
}

fn strip_pkg_version(token: &str) -> &str {
    match token.rsplit_once('@') {
        Some((head, ver)) if !head.is_empty() && !ver.contains('/') => head,
        _ => token,
    }
}

pub fn detect_argv_agent(argv: &[String]) -> Option<&'static str> {
    let mut slice = argv;
    for _ in 0..3 {
        let first = slice.first()?;
        if let Some(agent) = classify_agent_program(first) {
            return Some(agent);
        }
        let exe = exe_basename(first);
        if matches!(exe.as_str(), "env" | "sudo") {
            slice = skip_env_or_sudo(&exe, &slice[1..])?;
        } else {
            return detect_runtime_target(&exe, &slice[1..]).and_then(classify_agent_program);
        }
    }
    None
}

fn skip_env_or_sudo<'a>(exe: &str, args: &'a [String]) -> Option<&'a [String]> {
    let val_flags: &[&str] = if exe == "env" {
        &["-u", "--unset", "-C", "--chdir", "-S", "--split-string"]
    } else {
        &["-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-U"]
    };
    let mut idx = 0usize;
    while idx < args.len() {
        let tok = args[idx].as_str();
        if tok == "--" {
            return args.get(idx + 1..).filter(|s| !s.is_empty());
        }
        if !tok.starts_with('-') && !(exe == "env" && tok.contains('=')) {
            return Some(&args[idx..]);
        }
        idx += 1 + usize::from(val_flags.contains(&tok));
    }
    None
}

fn detect_runtime_target<'a>(exe: &str, args: &'a [String]) -> Option<&'a str> {
    if matches!(exe, "node" | "nodejs" | "bun" | "deno") {
        let skip = usize::from(exe == "deno" && args.first().map(String::as_str) == Some("run"));
        scan_flags(&args[skip..], &["-e", "--eval", "-p", "--print", "-c", "eval", "repl"], &["-r", "--require", "--loader", "--import", "-C", "--conditions"])
    } else if exe == "python" || exe.starts_with("python2") || exe.starts_with("python3") || exe.starts_with("pypy") {
        find_python_target(args)
    } else if matches!(exe, "npx" | "pnpx" | "bunx" | "uvx") {
        find_pkg_target(args)
    } else {
        None
    }
}

fn scan_flags<'a>(args: &'a [String], stop: &[&str], paired: &[&str]) -> Option<&'a str> {
    let mut idx = 0usize;
    while idx < args.len() {
        let tok = args[idx].as_str();
        if stop.contains(&tok) {
            return None;
        }
        if !tok.starts_with('-') {
            return Some(tok);
        }
        idx += 1 + usize::from(paired.contains(&tok));
    }
    None
}

fn find_python_target(args: &[String]) -> Option<&str> {
    for (idx, item) in args.iter().enumerate() {
        if item == "-m" {
            return args.get(idx + 1).map(String::as_str);
        }
        if item.starts_with("-c") {
            return None;
        }
    }
    scan_flags(args, &["-c"], &["-W", "-X", "--check-hash-based-pycs"])
}

fn find_pkg_target(args: &[String]) -> Option<&str> {
    for pair in args.windows(2) {
        if matches!(pair[0].as_str(), "-p" | "--package" | "--from") && classify_agent_program(&pair[1]).is_some() {
            return Some(pair[1].as_str());
        }
    }
    scan_flags(args, &["-c", "--call"], &["-p", "--package", "--from"])
}

pub fn select_tree_agent(root_pid: i32, tpgid: i32, procs: &[ProcEntry]) -> Option<&'static str> {
    procs
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            let agent = detect_argv_agent(&entry.argv)?;
            let fg_rank = u8::from(!in_foreground_lineage(entry, tpgid, procs));
            Some(((fg_rank, tree_depth(entry, root_pid, procs), idx), agent))
        })
        .min_by_key(|&(key, _)| key)
        .map(|(_, agent)| agent)
}

fn tree_depth(entry: &ProcEntry, root_pid: i32, procs: &[ProcEntry]) -> usize {
    let mut cur = if entry.pid == root_pid { return 0 } else { entry.ppid };
    let mut hops = 1usize;
    while hops < 32 && cur > 0 && cur != root_pid {
        let Some(parent) = procs.iter().find(|p| p.pid == cur) else { break };
        cur = parent.ppid;
        hops += 1;
    }
    hops
}

fn in_foreground_lineage(entry: &ProcEntry, tpgid: i32, procs: &[ProcEntry]) -> bool {
    tpgid <= 0
        || entry.pgrp == tpgid
        || procs.iter().filter(|p| p.pgrp == tpgid).any(|fg| is_ancestor_of(entry.pid, fg, procs))
}

fn is_ancestor_of(ancestor_pid: i32, child: &ProcEntry, procs: &[ProcEntry]) -> bool {
    let mut cur = child.ppid;
    for _ in 0..32 {
        if cur <= 0 || cur == ancestor_pid {
            return cur == ancestor_pid;
        }
        let Some(parent) = procs.iter().find(|p| p.pid == cur) else { return false };
        cur = parent.ppid;
    }
    false
}

pub fn detect_title_agent(raw: &str) -> Option<&'static str> {
    raw.split(['·', '|']).find_map(detect_segment_agent)
}

fn detect_segment_agent(seg: &str) -> Option<&'static str> {
    let mut words = strip_title_decorations(seg).split_whitespace();
    let w0 = words.next()?;
    if w0.contains('/') || w0.starts_with('.') || w0.starts_with('~') {
        return classify_agent_program(w0);
    }
    let clean0 = w0.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_').to_ascii_lowercase();
    if let Some(agent) = classify_agent_exe(&clean0) {
        return Some(agent);
    }
    let w1 = words.next()?.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_ascii_lowercase();
    match (clean0.as_str(), w1.as_str()) {
        ("openai", "codex") => Some("Codex"),
        ("google", "antigravity") => Some("AGY"),
        _ => None,
    }
}

fn strip_title_decorations(seg: &str) -> &str {
    let s = seg.trim().strip_prefix("[wt]").map_or_else(|| seg.trim(), str::trim_start);
    let s = s.trim_start_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '/' | '~' | '.' | '@'));
    match s.split_once(':') {
        Some((num, rest)) if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) => rest.trim_start(),
        _ => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| (*s).to_owned()).collect()
    }

    fn node(pid: i32, ppid: i32, pgrp: i32, words: &[&str]) -> ProcEntry {
        ProcEntry { pid, ppid, pgrp, argv: args(words) }
    }

    #[test]
    fn ignores_claude_or_agent_substrings_in_shell_and_python_arguments() {
        assert_eq!(detect_argv_agent(&args(&["bash", "-c", "cat ~/.claude/projects/MEMORY.md"])), None);
        assert_eq!(detect_argv_agent(&args(&["/venv/bin/python", "/srv/gif.py", "--cache", "~/.claude/x"])), None);
        assert_eq!(detect_argv_agent(&args(&["python3", "-c", "print('claude codex kimi agy')"])), None);
        assert_eq!(detect_argv_agent(&args(&["node", "-e", "console.log('claude')"])), None);
        assert_eq!(detect_argv_agent(&args(&["rg", "-n", "claude", "/home/u/Productivity"])), None);
        assert_eq!(detect_argv_agent(&args(&["git", "commit", "-m", "fix claude detection"])), None);
    }

    #[test]
    fn detects_direct_binaries_versioned_paths_and_runtime_wrappers() {
        assert_eq!(detect_argv_agent(&args(&["agy", "-c"])), Some("AGY"));
        assert_eq!(detect_argv_agent(&args(&["/home/u/.local/share/claude/versions/2.1.286"])), Some("Claude"));
        assert_eq!(detect_argv_agent(&args(&["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"])), Some("Claude"));
        assert_eq!(detect_argv_agent(&args(&["/usr/bin/env", "FOO=1", "node", "/opt/@openai/codex/bin/codex.js"])), Some("Codex"));
        assert_eq!(detect_argv_agent(&args(&["python3", "-u", "/home/u/.kimi-code/bin/kimi"])), Some("Kimi"));
        assert_eq!(detect_argv_agent(&args(&["python3", "-m", "deepseek_cli"])), Some("DeepSeek"));
        assert_eq!(detect_argv_agent(&args(&["npx", "-y", "@anthropic-ai/claude-code@latest"])), Some("Claude"));
    }

    #[test]
    fn keeps_parent_agy_identity_when_subprocesses_or_subtools_run() {
        let tree = [
            node(100, 1, 100, &["-zsh"]),
            node(200, 100, 200, &["agy", "-c"]),
            node(210, 200, 200, &["bash", "-c", "python3 gif.py ~/.claude/memory.md"]),
            node(211, 210, 200, &["/venv/bin/python", "/srv/gif.py", "/home/u/.claude/memory.md"]),
            node(212, 210, 200, &["claude", "-p", "sub-tool check"]),
        ];
        assert_eq!(select_tree_agent(100, 200, &tree), Some("AGY"));
        assert_eq!(select_tree_agent(100, 210, &tree), Some("AGY"));
    }

    #[test]
    fn prioritizes_foreground_agent_and_unwraps_sh_launchers() {
        let bg_claude = [
            node(100, 1, 100, &["zsh"]),
            node(150, 100, 150, &["claude"]),
            node(200, 100, 200, &["agy", "-c"]),
        ];
        assert_eq!(select_tree_agent(100, 200, &bg_claude), Some("AGY"));
        let sh_claude = [node(100, 1, 100, &["zsh"]), node(120, 100, 120, &["sh", "-c", "claude"]), node(121, 120, 120, &["claude"])];
        assert_eq!(select_tree_agent(100, 120, &sh_claude), Some("Claude"));
    }

    #[test]
    fn detects_clean_title_agents_and_rejects_editor_or_grep_titles() {
        assert_eq!(detect_title_agent("✳ Claude Code"), Some("Claude"));
        assert_eq!(detect_title_agent("0:codex"), Some("Codex"));
        assert_eq!(detect_title_agent("OpenAI Codex"), Some("Codex"));
        assert_eq!(detect_title_agent("Productivity · AGY · agy -c"), Some("AGY"));
        assert_eq!(detect_title_agent("Research · DeepSeek"), Some("DeepSeek"));
        assert_eq!(detect_title_agent("[wt] kimi"), Some("Kimi"));
        assert_eq!(detect_title_agent("vim ~/.claude/CLAUDE.md"), None);
        assert_eq!(detect_title_agent("grep claude src/proc_radar.rs"), None);
    }

    #[test]
    fn agent_detect_module_meets_grade_b_mi_target() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("agent_detect.rs"));
        assert!(mi >= 60.0, "agent_detect.rs MI = {mi:.2}");
    }
}
