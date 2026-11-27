//! Local filesystem listing, recursive size estimation, and atomic chunked copy operations.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use super::{sanitize_leaf_name, sort_entries, FileEntry, FileKind, TransferProgress};

const CHUNK_BYTES: usize = 64 * 1024;
const MAX_WALK_DEPTH: usize = 32;
const MAX_WALK_ENTRIES: usize = 50_000;
static STAGE_SEQ: AtomicU64 = AtomicU64::new(1);

fn split_collision_name(name: &str) -> (&str, Option<&str>) {
    if let Some((stem, ext)) = name.rsplit_once('.') {
        if !stem.is_empty() && !ext.is_empty() {
            return (stem, Some(ext));
        }
    }
    (name, None)
}

fn numbered_name(stem: &str, ext: Option<&str>, idx: u32) -> String {
    match ext {
        Some(e) => format!("{stem} ({idx}).{e}"),
        None => format!("{stem} ({idx})"),
    }
}

fn find_numbered_candidate(dest_dir: &Path, clean: &str) -> PathBuf {
    let (stem, ext) = split_collision_name(clean);
    for idx in 1..=10_000u32 {
        let candidate = dest_dir.join(numbered_name(stem, ext, idx));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    dest_dir.join(format!("{clean}.{}", STAGE_SEQ.fetch_add(1, Ordering::Relaxed)))
}

/// Resolves a collision-safe target path inside `dest_dir` (`name`, `name (1).ext`, ...).
pub fn resolve_unique_local_path(dest_dir: &Path, raw_name: &str) -> PathBuf {
    let clean = sanitize_leaf_name(raw_name);
    let direct = dest_dir.join(&clean);
    if fs::symlink_metadata(&direct).is_err() {
        return direct;
    }
    find_numbered_candidate(dest_dir, &clean)
}

fn push_dir_children(dir: &Path, depth: usize, stack: &mut Vec<(PathBuf, usize)>) {
    if let Ok(entries) = fs::read_dir(dir) {
        entries.flatten().for_each(|e| stack.push((e.path(), depth + 1)));
    }
}

fn accumulate_size_node(path: &Path, depth: usize, stack: &mut Vec<(PathBuf, usize)>) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.file_type().is_file() {
        return meta.len();
    }
    if meta.file_type().is_dir() && depth < MAX_WALK_DEPTH {
        push_dir_children(path, depth, stack);
    }
    0
}

/// Estimates total byte size of local files and directories (bounded depth/entries).
pub fn compute_local_size(paths: &[PathBuf]) -> u64 {
    let mut stack: Vec<(PathBuf, usize)> = paths.iter().cloned().map(|p| (p, 0)).collect();
    let (mut total, mut visited) = (0u64, 0usize);
    while let Some((path, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_WALK_ENTRIES {
            break;
        }
        total = total.saturating_add(accumulate_size_node(&path, depth, &mut stack));
    }
    total
}

pub(crate) fn next_stage_path(dir: &Path, prefix: &str) -> PathBuf {
    let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{prefix}-{}-{seq}", std::process::id()))
}

fn check_cancelled(progress: &TransferProgress) -> io::Result<()> {
    if progress.is_cancelled() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Transfer cancelled"));
    }
    Ok(())
}

fn copy_file_chunks(src: &mut File, dst: &mut File, progress: &TransferProgress) -> io::Result<()> {
    let mut buf = [0u8; CHUNK_BYTES];
    loop {
        check_cancelled(progress)?;
        let n = src.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        dst.write_all(&buf[..n])?;
        progress.add_done(n as u64);
    }
}

fn open_stage_writer(part: &Path) -> io::Result<File> {
    OpenOptions::new().create_new(true).write(true).mode(0o644).open(part)
}

fn copy_and_sync_part(src: &Path, part: &Path, progress: &TransferProgress) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    let (mut reader, mut writer) = (File::open(src)?, open_stage_writer(part)?);
    copy_file_chunks(&mut reader, &mut writer, progress)?;
    writer.set_permissions(meta.permissions())?;
    writer.sync_all()
}

fn copy_file_to(src: &Path, dst: &Path, progress: &TransferProgress) -> io::Result<()> {
    let parent = dst.parent().ok_or_else(|| io::Error::other("missing destination parent"))?;
    let part = next_stage_path(parent, ".akhterm-part");
    copy_and_sync_part(src, &part, progress)
        .and_then(|()| fs::rename(&part, dst))
        .inspect_err(|_| {
            let _ = fs::remove_file(&part);
        })
}

fn copy_dir_contents(src: &Path, dst: &Path, progress: &TransferProgress, depth: usize) -> io::Result<()> {
    if depth >= MAX_WALK_DEPTH {
        return Err(io::Error::other("directory nesting limit exceeded"));
    }
    for entry in fs::read_dir(src)? {
        check_cancelled(progress)?;
        let entry = entry?;
        copy_node_to(&entry.path(), &dst.join(entry.file_name()), progress, depth + 1)?;
    }
    Ok(())
}

fn copy_node_to(src: &Path, dst: &Path, progress: &TransferProgress, depth: usize) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        return std::os::unix::fs::symlink(fs::read_link(src)?, dst);
    }
    if meta.file_type().is_dir() {
        fs::create_dir(dst)?;
        return copy_dir_contents(src, dst, progress, depth);
    }
    if meta.file_type().is_file() {
        return copy_file_to(src, dst, progress);
    }
    Ok(())
}

fn check_not_self_descendant(src: &Path, dest_dir: &Path) -> io::Result<()> {
    if let (Ok(c_src), Ok(c_dst)) = (fs::canonicalize(src), fs::canonicalize(dest_dir)) {
        if c_dst.starts_with(&c_src) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Cannot copy a folder into itself"));
        }
    }
    Ok(())
}

fn copy_top_dir(src: &Path, dest_dir: &Path, target: &Path, progress: &TransferProgress) -> io::Result<()> {
    check_not_self_descendant(src, dest_dir)?;
    let stage = next_stage_path(dest_dir, ".akhterm-dir");
    fs::create_dir(&stage)?;
    let res = copy_dir_contents(src, &stage, progress, 0).and_then(|()| fs::rename(&stage, target));
    res.inspect_err(|_| {
        let _ = fs::remove_dir_all(&stage);
    })
}

fn copy_single_source(src: &Path, dest_dir: &Path, progress: &TransferProgress) -> io::Result<PathBuf> {
    let meta = fs::symlink_metadata(src)?;
    let raw_name = src.file_name().and_then(|n| n.to_str()).unwrap_or("item");
    let target = resolve_unique_local_path(dest_dir, raw_name);
    if meta.file_type().is_dir() {
        copy_top_dir(src, dest_dir, &target, progress)?;
    } else {
        copy_node_to(src, &target, progress, 0)?;
    }
    Ok(target)
}

/// Copies local files, symlinks, and folders atomically into `dest_dir` with progress/cancel.
pub fn copy_local_items(sources: &[PathBuf], dest_dir: &Path, progress: &TransferProgress) -> io::Result<Vec<PathBuf>> {
    if !dest_dir.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "destination directory unavailable"));
    }
    let mut created = Vec::with_capacity(sources.len());
    for src in sources {
        created.push(copy_single_source(src, dest_dir, progress)?);
    }
    Ok(created)
}

fn entry_mtime_secs(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

fn classify_local_kind(meta: &fs::Metadata) -> FileKind {
    if meta.file_type().is_symlink() {
        FileKind::Symlink
    } else if meta.file_type().is_dir() {
        FileKind::Dir
    } else {
        FileKind::File
    }
}

fn build_local_entry(name: String, path: &Path, meta: &fs::Metadata) -> FileEntry {
    let kind = classify_local_kind(meta);
    let size = if kind.is_dir() { 0 } else { meta.len() };
    FileEntry {
        name, full_path: path.to_string_lossy().into_owned(),
        kind, size, mtime: entry_mtime_secs(meta),
    }
}

fn read_local_entry(path: &Path, show_hidden: bool) -> Option<FileEntry> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if !show_hidden && name.starts_with('.') {
        return None;
    }
    let meta = fs::symlink_metadata(path).ok()?;
    Some(build_local_entry(name, path, &meta))
}

/// Lists a local directory, returning `(canonical_dir, sorted_entries)`.
pub fn list_local_dir(dir: &Path, show_hidden: bool) -> io::Result<(PathBuf, Vec<FileEntry>)> {
    let canon = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut items: Vec<FileEntry> = fs::read_dir(&canon)?
        .flatten()
        .filter_map(|e| read_local_entry(&e.path(), show_hidden))
        .collect();
    sort_entries(&mut items);
    Ok((canon, items))
}

/// Resolves the user's Downloads directory (`$XDG_DOWNLOAD_DIR` -> `~/Downloads` -> `~`).
pub fn default_downloads_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DOWNLOAD_DIR").map(PathBuf::from) {
        if dir.is_absolute() && dir.is_dir() {
            return dir;
        }
    }
    let home = PathBuf::from(crate::paths::home_dir());
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}
