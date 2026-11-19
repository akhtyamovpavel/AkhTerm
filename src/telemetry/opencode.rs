use super::{json_str, json_u64, TelemetrySource, TokenSnapshot};
use crate::paths::{state_dir, write_state_file};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::{env, fs, io};

fn write_tui_files(root: &Path, config: &Path) -> io::Result<()> {
    write_state_file(
        &root.join("usage.mjs"),
        include_str!("../../assets/opencode/usage.mjs"),
    )?;
    write_state_file(config, r#"{"plugin":["./usage.mjs"]}"#)
}

pub fn prepare_tui() -> io::Result<()> {
    if env::var_os("OPENCODE_TUI_CONFIG").is_some() {
        return Ok(());
    }
    let root = state_dir().join("opencode");
    let config = root.join("tui.json");
    write_tui_files(&root, &config)?;
    env::set_var("OPENCODE_TUI_CONFIG", config);
    Ok(())
}

fn is_opencode_jsonl(file: &Path, root: &Path) -> bool {
    file.starts_with(root)
        && file.extension().is_some_and(|x| x == "jsonl")
        && file.is_file()
}

fn find_opencode_jsonl_in_pid(pid: i32, root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    entries
        .flatten()
        .filter_map(|e| fs::read_link(e.path()).ok())
        .find(|file| is_opencode_jsonl(file, root))
}

pub fn discover(pid: i32) -> Option<PathBuf> {
    let root = state_dir().join("opencode");
    crate::tmux_local::collect_fd_tree_pids(pid)
        .into_iter()
        .find_map(|p| find_opencode_jsonl_in_pid(p, &root))
}

fn hash_request_id(id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

fn populate_token_counts(line: &str, context: u64, snap: &mut TokenSnapshot) {
    snap.source = TelemetrySource::OpenCode;
    snap.input_tokens = json_u64(line, "input").unwrap_or(0);
    snap.cache_tokens = json_u64(line, "read").unwrap_or(0);
    snap.cache_write_tokens = json_u64(line, "write").unwrap_or(0);
    snap.output_tokens = json_u64(line, "output").unwrap_or(0);
    snap.context_tokens = context;
}

pub fn apply_usage(line: &str, snap: &mut TokenSnapshot) {
    let Some(context) = json_u64(line, "context") else {
        *snap = TokenSnapshot::default();
        return;
    };
    populate_token_counts(line, context, snap);
    let serial = json_str(line, "request").as_deref().map(hash_request_id);
    snap.requests
        .record(json_u64(line, "last_read"), context, serial);
}
