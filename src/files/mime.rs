//! Pure D&D MIME payload codec (`application/vnd.akhterm.files`, URI lists, GNOME copies, PNG staging).

use std::ffi::CStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::{DragPayload, FileKind, RemoteFileItem};

pub const MIME_AKHTERM_FILES: &CStr = c"application/vnd.akhterm.files";
pub const MIME_URI_LIST: &CStr = c"text/uri-list";
pub const MIME_GNOME_COPIED: &CStr = c"x-special/gnome-copied-files";
pub const MIME_IMAGE_PNG: &CStr = c"image/png";

pub const INFO_AKHTERM_FILES: u32 = 1;
pub const INFO_URI_LIST: u32 = 2;
pub const INFO_GNOME_COPIED: u32 = 3;
pub const INFO_IMAGE_PNG: u32 = 4;

const AKHTERM_FILES_HEADER: &str = "akhterm-files-v1";
const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Encodes local file paths into the internal `akhterm-files-v1` drag payload.
pub fn encode_local_drag(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = format!("{AKHTERM_FILES_HEADER}\n");
    for raw in paths.iter().map(|p| p.to_string_lossy()).filter(|r| is_valid_wire_field(r)) {
        out.push_str(&format!("L\t{raw}\n"));
    }
    out.into_bytes()
}

fn is_valid_wire_field(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b >= 0x20 && b != 0x7f)
}

fn push_remote_record(out: &mut String, item: &RemoteFileItem) {
    if is_valid_wire_field(&item.host) && is_valid_wire_field(&item.path) {
        let tag = item.kind.as_tag();
        out.push_str(&format!("R\t{}\t{}\t{tag}\t{}\t{}\n", item.host, item.ssh_pid, item.size, item.path));
    }
}

/// Encodes remote SSH entries into the internal `akhterm-files-v1` drag payload.
pub fn encode_remote_drag(items: &[RemoteFileItem]) -> Vec<u8> {
    let mut out = format!("{AKHTERM_FILES_HEADER}\n");
    for item in items {
        push_remote_record(&mut out, item);
    }
    out.into_bytes()
}

fn parse_remote_fields(parts: &[&str]) -> Option<RemoteFileItem> {
    let (host, path) = (parts[1], parts[5]);
    if !is_valid_wire_field(host) || !is_valid_wire_field(path) {
        return None;
    }
    Some(RemoteFileItem {
        host: host.to_owned(), ssh_pid: parts[2].parse().ok()?, path: path.to_owned(),
        kind: FileKind::from_tag(parts[3]), size: parts[4].parse().ok()?,
    })
}

fn parse_remote_line(line: &str) -> Option<RemoteFileItem> {
    let parts: Vec<&str> = line.split('\t').collect();
    (parts.len() == 6 && parts[0] == "R").then(|| parse_remote_fields(&parts)).flatten()
}

fn decode_local_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Option<DragPayload> {
    let mut paths = Vec::new();
    for line in lines {
        let raw = line.strip_prefix("L\t").filter(|s| is_valid_wire_field(s))?;
        paths.push(PathBuf::from(raw));
    }
    (!paths.is_empty()).then_some(DragPayload::LocalPaths(paths))
}

fn decode_remote_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Option<DragPayload> {
    let mut items = Vec::new();
    for line in lines {
        items.push(parse_remote_line(line)?);
    }
    (!items.is_empty()).then_some(DragPayload::RemoteItems(items))
}

fn dispatch_akhterm_lines(rest: Vec<&str>) -> Option<DragPayload> {
    let first = *rest.first()?;
    if first.starts_with("L\t") {
        decode_local_lines(rest.into_iter())
    } else if first.starts_with("R\t") {
        decode_remote_lines(rest.into_iter())
    } else {
        None
    }
}

/// Decodes `application/vnd.akhterm.files` payload bytes into local paths or remote items.
pub fn decode_akhterm_drag(raw: &[u8]) -> Option<DragPayload> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut lines = text.lines().filter(|l| !l.is_empty());
    (lines.next()? == AKHTERM_FILES_HEADER).then(|| dispatch_akhterm_lines(lines.collect())).flatten()
}

fn decode_uri_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<PathBuf> {
    let uris: Vec<String> = lines
        .map(|l| l.trim_end_matches('\r').to_owned())
        .filter(|l| !l.trim().is_empty())
        .collect();
    crate::file_drop::decode_uri_list(&uris)
}

/// Decodes `text/uri-list` payload bytes into local `PathBuf` items.
pub fn decode_uri_list_bytes(raw: &[u8]) -> Vec<PathBuf> {
    let Ok(text) = std::str::from_utf8(raw) else {
        return Vec::new();
    };
    decode_uri_lines(text.lines())
}

/// Decodes `x-special/gnome-copied-files` (`copy\nfile://...` or `cut\nfile://...`) bytes.
pub fn decode_gnome_copied_files(raw: &[u8]) -> Vec<PathBuf> {
    let Ok(text) = std::str::from_utf8(raw) else {
        return Vec::new();
    };
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    matches!(lines.next().map(str::trim), Some("copy" | "cut"))
        .then(|| decode_uri_lines(lines))
        .unwrap_or_default()
}

fn write_atomic_png(part: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).mode(0o600).open(part)?;
    let write_res = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    write_res.and_then(|()| fs::rename(part, target)).inspect_err(|_| {
        let _ = fs::remove_file(part);
    })
}

/// Validates PNG magic bytes and writes an image drop atomically into `dir`.
pub fn stage_png_bytes(bytes: &[u8], dir: &Path, stamp: u64) -> io::Result<PathBuf> {
    if !bytes.starts_with(PNG_MAGIC) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid PNG header"));
    }
    fs::create_dir_all(dir)?;
    let target = dir.join(format!("akhterm_drop_{stamp}.png"));
    let part = dir.join(format!(".akhterm-part-{}-{stamp}.png", std::process::id()));
    write_atomic_png(&part, &target, bytes)?;
    Ok(target)
}
