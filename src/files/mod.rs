#![allow(dead_code)]
//! Domain types and progress state for local and SSH file drag-and-drop and transfer operations.

pub mod drop_overlay;
pub mod local_fs;
pub mod manager;
pub mod mime;
pub mod resolve;
pub mod ssh_ops;
pub mod transfer_banner;
#[cfg(test)]
#[cfg(test)]
mod tests;
#[cfg(test)]
mod ui_tests;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::telemetry::ssh_connection::SshRoute;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropMode {
    InsertPath,
    CopyToCwd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
}

impl FileKind {
    pub fn as_tag(self) -> char {
        match self {
            Self::Dir => 'd',
            Self::Symlink => 'l',
            Self::File => 'f',
        }
    }

    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "d" => Self::Dir,
            "l" => Self::Symlink,
            _ => Self::File,
        }
    }

    pub fn is_dir(self) -> bool {
        self == Self::Dir
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub full_path: String,
    pub kind: FileKind,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFileItem {
    pub host: String,
    pub ssh_pid: i32,
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragPayload {
    LocalPaths(Vec<PathBuf>),
    RemoteItems(Vec<RemoteFileItem>),
    PngBytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileEndpoint {
    Local { cwd: PathBuf },
    Remote { sid: usize, host: String, ssh_pid: i32, route: SshRoute, cwd: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFileLink {
    pub sid: usize,
    pub host: String,
    pub ssh_pid: i32,
    pub route: SshRoute,
    pub path: String,
}

#[derive(Clone, Default)]
pub struct TransferProgress {
    pub bytes_done: Arc<AtomicU64>,
    pub bytes_total: Arc<AtomicU64>,
    pub cancelled: Arc<AtomicBool>,
}

impl TransferProgress {
    pub fn new(total: u64) -> Self {
        Self {
            bytes_done: Arc::new(AtomicU64::new(0)),
            bytes_total: Arc::new(AtomicU64::new(total)),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn add_done(&self, delta: u64) {
        self.bytes_done.fetch_add(delta, Ordering::Relaxed);
    }

    pub fn set_total(&self, total: u64) {
        self.bytes_total.store(total, Ordering::Relaxed);
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.bytes_done.load(Ordering::Relaxed),
            self.bytes_total.load(Ordering::Relaxed),
        )
    }
}

/// Formats byte counts into compact binary units (`B`, `KB`, `MB`, `GB`).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut val = bytes as f64;
    let mut idx = 0usize;
    while val >= 1024.0 && idx + 1 < UNITS.len() {
        val /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{bytes} B")
    } else if val >= 10.0 {
        format!("{val:.0} {}", UNITS[idx])
    } else {
        format!("{val:.1} {}", UNITS[idx])
    }
}

/// Sanitizes a single path leaf component, rejecting traversal and NUL/slash bytes.
pub fn sanitize_leaf_name(raw: &str) -> String {
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or(raw).replace('\0', "");
    if leaf.is_empty() || leaf == "." || leaf == ".." {
        "item".to_owned()
    } else {
        leaf
    }
}

/// Sorts directory entries with folders first, then case-insensitive filename order.
pub fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        b.kind
            .is_dir()
            .cmp(&a.kind.is_dir())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}
