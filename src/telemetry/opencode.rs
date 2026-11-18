use super::{json_str, json_u64, TelemetrySource, TokenSnapshot};
use crate::paths::{state_dir, write_state_file};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::{env, fs, io, path::PathBuf};

pub fn prepare_tui() -> io::Result<()> {
    if env::var_os("OPENCODE_TUI_CONFIG").is_some() {
        return Ok(());
    }
    let root = state_dir().join("opencode");
    let config = root.join("tui.json");
    write_state_file(
        &root.join("usage.mjs"),
        include_str!("../../assets/opencode/usage.mjs"),
    )?;
    write_state_file(&config, r#"{"plugin":["./usage.mjs"]}"#)?;
    env::set_var("OPENCODE_TUI_CONFIG", config);
    Ok(())
}

pub fn discover(pid: i32) -> Option<PathBuf> {
    let root = state_dir().join("opencode");
    for pid in crate::tmux_local::collect_fd_tree_pids(pid) {
        let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for file in entries
            .flatten()
            .filter_map(|e| fs::read_link(e.path()).ok())
        {
            if file.starts_with(&root)
                && file.extension().is_some_and(|x| x == "jsonl")
                && file.is_file()
            {
                return Some(file);
            }
        }
    }
    None
}

pub fn apply_usage(line: &str, snap: &mut TokenSnapshot) {
    let Some(context) = json_u64(line, "context") else {
        *snap = TokenSnapshot::default();
        return;
    };
    snap.source = TelemetrySource::OpenCode;
    snap.input_tokens = json_u64(line, "input").unwrap_or(0);
    snap.cache_tokens = json_u64(line, "read").unwrap_or(0);
    snap.cache_write_tokens = json_u64(line, "write").unwrap_or(0);
    snap.output_tokens = json_u64(line, "output").unwrap_or(0);
    snap.context_tokens = context;
    let serial = json_str(line, "request").map(|id| {
        let mut hasher = DefaultHasher::new();
        id.hash(&mut hasher);
        hasher.finish()
    });
    snap.requests
        .record(json_u64(line, "last_read"), context, serial);
}
