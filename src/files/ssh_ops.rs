//! SSH directory listing and streaming bidirectional file/folder transfers via `SshRoute`.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::proc_radar::shell_quote;
use crate::telemetry::ssh_connection::SshRoute;

use super::local_fs::{next_stage_path, resolve_unique_local_path};
use super::{sanitize_leaf_name, sort_entries, FileEntry, FileKind, TransferProgress};

const CHUNK_BYTES: usize = 64 * 1024;
const REMOTE_RESOLVE_SH: &str = include_str!("remote_resolve.sh");
const REMOTE_LIST_SH: &str = include_str!("remote_list.sh");

fn remote_dir_expr(dir: &str) -> String {
    let trimmed = dir.trim();
    if trimmed.is_empty() || trimmed == "~" {
        "\"$HOME\"".to_owned()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        format!("\"$HOME/\"{}", shell_quote(rest))
    } else {
        shell_quote(trimmed)
    }
}

/// Builds a POSIX-compatible remote directory listing script.
pub fn build_remote_list_script(dir: &str) -> String {
    let target = remote_dir_expr(dir);
    let body = REMOTE_LIST_SH;
    let prefix = format!("cd -- {target} 2>/dev/null || exit 1; pwd -P;");
    format!("{prefix}\n{body}")
}

fn parse_listing_line(cwd: &str, line: &str, show_hidden: bool) -> Option<FileEntry> {
    let mut parts = line.splitn(4, '\t');
    let kind = FileKind::from_tag(parts.next()?);
    let size = parts.next()?.parse::<u64>().unwrap_or(0);
    let mtime = parts.next()?.parse::<i64>().unwrap_or(0);
    let name = parts.next()?.trim_end_matches('\r');
    let skip = name.is_empty() || name == "." || name == ".." || (!show_hidden && name.starts_with('.'));
    let full_path = if cwd == "/" { format!("/{name}") } else { format!("{}/{name}", cwd.trim_end_matches('/')) };
    (!skip).then(|| FileEntry { name: name.to_owned(), full_path, kind, size, mtime })
}

/// Parses stdout produced by `build_remote_list_script` into `(canonical_cwd, sorted_entries)`.
pub fn parse_remote_listing(stdout: &str, show_hidden: bool) -> io::Result<(String, Vec<FileEntry>)> {
    let mut lines = stdout.lines();
    let cwd = lines.next().map(|s| s.trim_end_matches('\r').trim().to_owned())
        .filter(|s| !s.is_empty()).ok_or_else(|| io::Error::other("empty remote directory response"))?;
    let mut entries: Vec<FileEntry> = lines.filter_map(|l| parse_listing_line(&cwd, l, show_hidden)).collect();
    sort_entries(&mut entries);
    Ok((cwd, entries))
}

/// Lists a remote directory over `route`.
pub fn list_remote_dir(route: &SshRoute, dir: &str, show_hidden: bool) -> io::Result<(String, Vec<FileEntry>)> {
    let out = route.spawn(&build_remote_list_script(dir), Stdio::null(), "10")?.wait_with_output()?;
    if !out.status.success() {
        return Err(io::Error::other("SSH directory listing failed"));
    }
    parse_remote_listing(&String::from_utf8_lossy(&out.stdout), show_hidden)
}

fn target_resolution_snippet(dir_var: &str, name: &str, exact_path: Option<&str>) -> String {
    match exact_path {
        Some(p) => format!("target={}; mkdir -p -- \"$(dirname -- \"$target\")\"", shell_quote(p)),
        None => format!("target=$(resolve_unique \"${dir_var}\" {})", shell_quote(name)),
    }
}

fn wrap_remote_dest_script(dest_dir: &str, body: &str) -> String {
    let d_expr = remote_dir_expr(dest_dir);
    let prefix = format!("{REMOTE_RESOLVE_SH}\nd={d_expr}; [ -d \"$d\" ] || exit 1;");
    format!("{prefix} {body}")
}

/// Builds a POSIX script that writes stdin atomically to a remote file.
pub fn build_remote_file_upload_script(dest_dir: &str, file_name: &str, exact_path: Option<&str>) -> String {
    let target_cmd = target_resolution_snippet("d", &sanitize_leaf_name(file_name), exact_path);
    let body = format!(
        "tmp=\"$d/.akhterm-part-$$\"; trap 'rm -f -- \"$tmp\"' EXIT INT TERM; \
         cat > \"$tmp\" || exit 1; {target_cmd}; mv -- \"$tmp\" \"$target\" || exit 1; \
         trap - EXIT INT TERM; printf '%s\\n' \"$target\""
    );
    wrap_remote_dest_script(dest_dir, &body)
}

/// Builds a POSIX script that extracts a tar stream from stdin atomically into a remote directory.
pub fn build_remote_dir_upload_script(dest_dir: &str, dir_name: &str, exact_path: Option<&str>) -> String {
    let clean = sanitize_leaf_name(dir_name);
    let target_cmd = target_resolution_snippet("d", &clean, exact_path);
    let q_clean = shell_quote(&clean);
    let body = format!(
        "stage=\"$d/.akhterm-dir-$$\"; trap 'rm -rf -- \"$stage\"' EXIT INT TERM; \
         mkdir -p -- \"$stage\" || exit 1; tar -xf - -C \"$stage\" || exit 1; {target_cmd}; \
         mv -- \"$stage/\"{q_clean} \"$target\" || exit 1; rm -rf -- \"$stage\"; \
         trap - EXIT INT TERM; printf '%s\\n' \"$target\""
    );
    wrap_remote_dest_script(dest_dir, &body)
}

/// Copies `reader` to `writer` in 64 KiB chunks while updating `progress` and checking cancellation.
pub fn pump_stream(reader: &mut impl Read, writer: &mut impl Write, progress: &TransferProgress) -> io::Result<u64> {
    let (mut buf, mut total) = ([0u8; CHUNK_BYTES], 0u64);
    loop {
        if progress.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Transfer cancelled"));
        }
        let n = reader.read(&mut buf)?;
        if n == 0 { return Ok(total); }
        writer.write_all(&buf[..n])?;
        progress.add_done(n as u64);
        total += n as u64;
    }
}

fn finish_child_checked(child: Child, fallback: &str) -> io::Result<String> {
    let out = child.wait_with_output()?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    Err(io::Error::other(if err.is_empty() { fallback.to_owned() } else { err }))
}

fn pipe_child_streams(mut src: Child, mut dst: Child, progress: &TransferProgress, src_err: &str, dst_err: &str) -> io::Result<String> {
    let mut reader = src.stdout.take().ok_or_else(|| io::Error::other("missing child stdout"))?;
    let mut writer = dst.stdin.take().ok_or_else(|| io::Error::other("missing child stdin"))?;
    let pumped = pump_stream(&mut reader, &mut writer, progress);
    drop(writer);
    if let Err(err) = pumped {
        let _ = (src.kill(), src.wait(), dst.kill(), dst.wait());
        return Err(err);
    }
    finish_child_checked(src, src_err)?;
    finish_child_checked(dst, dst_err)
}

fn stream_file_to_ssh(src: &Path, mut ssh_child: Child, progress: &TransferProgress) -> io::Result<String> {
    let mut stdin = ssh_child.stdin.take().ok_or_else(|| io::Error::other("missing ssh stdin"))?;
    let res = fs::File::open(src).and_then(|mut f| pump_stream(&mut f, &mut stdin, progress));
    drop(stdin);
    if let Err(err) = res {
        let _ = ssh_child.kill(); let _ = ssh_child.wait();
        return Err(err);
    }
    finish_child_checked(ssh_child, "SSH file upload failed")
}

fn upload_file_to_ssh(route: &SshRoute, src: &Path, dest_dir: &str, exact: Option<&str>, progress: &TransferProgress) -> io::Result<String> {
    let leaf = src.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let script = build_remote_file_upload_script(dest_dir, leaf, exact);
    let ssh_child = route.spawn_rw(&script, true)?;
    stream_file_to_ssh(src, ssh_child, progress)
}

fn upload_dir_to_ssh(route: &SshRoute, src: &Path, dest_dir: &str, exact: Option<&str>, progress: &TransferProgress) -> io::Result<String> {
    let parent = src.parent().unwrap_or_else(|| Path::new("/"));
    let leaf = src.file_name().and_then(|n| n.to_str()).unwrap_or("folder");
    let script = build_remote_dir_upload_script(dest_dir, leaf, exact);
    let tar_child = Command::new("tar").args(["-cf", "-", "-C"]).arg(parent).args(["--", leaf])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let ssh_child = route.spawn_rw(&script, true)?;
    pipe_child_streams(tar_child, ssh_child, progress, "local tar failed", "SSH folder upload failed")
}

fn upload_one_local(route: &SshRoute, src: &Path, dest_dir: &str, exact: Option<&str>, progress: &TransferProgress) -> io::Result<String> {
    let remote_dir = if exact.is_some() { "/tmp" } else { dest_dir };
    if fs::symlink_metadata(src)?.file_type().is_dir() {
        upload_dir_to_ssh(route, src, remote_dir, exact, progress)
    } else {
        upload_file_to_ssh(route, src, remote_dir, exact, progress)
    }
}

/// Uploads local files and directories to `dest_dir` (or `/tmp/akhterm_*` when `to_tmp` is true).
pub fn upload_local_to_ssh(route: &SshRoute, sources: &[PathBuf], dest_dir: &str, to_tmp: bool, progress: &TransferProgress) -> io::Result<Vec<String>> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut out = Vec::with_capacity(sources.len());
    for (idx, src) in sources.iter().enumerate() {
        let exact = to_tmp.then(|| crate::file_drop::remote_drop_path(src, stamp, idx));
        out.push(upload_one_local(route, src, dest_dir, exact.as_deref(), progress)?);
    }
    Ok(out)
}

fn parse_probe_stdout(stdout: &[u8]) -> (FileKind, u64) {
    let text = String::from_utf8_lossy(stdout);
    let mut parts = text.trim().split('\t');
    let kind = FileKind::from_tag(parts.next().unwrap_or("f"));
    let size = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    (kind, size)
}

fn probe_remote_entry(route: &SshRoute, path: &str) -> io::Result<(FileKind, u64)> {
    let q = shell_quote(path);
    let script = format!("if [ -d {q} ]; then printf 'd\\t0\\n'; elif [ -f {q} ]; then printf 'f\\t%s\\n' \"$(wc -c < {q} 2>/dev/null || echo 0)\"; else exit 1; fi");
    let out = route.spawn(&script, Stdio::null(), "10")?.wait_with_output()?;
    if !out.status.success() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("remote path not found: {path}")));
    }
    Ok(parse_probe_stdout(&out.stdout))
}

fn stream_ssh_to_part(mut ssh_child: Child, part: &Path, progress: &TransferProgress) -> io::Result<()> {
    let mut stdout = ssh_child.stdout.take().ok_or_else(|| io::Error::other("missing ssh stdout"))?;
    let mut file = OpenOptions::new().create_new(true).write(true).mode(0o644).open(part)?;
    let pumped = pump_stream(&mut stdout, &mut file, progress).and_then(|_| file.sync_all());
    drop(file);
    if let Err(err) = pumped {
        let _ = ssh_child.kill(); let _ = ssh_child.wait();
        return Err(err);
    }
    finish_child_checked(ssh_child, "SSH download failed").map(|_| ())
}

fn download_file_from_ssh(route: &SshRoute, remote_path: &str, local_dir: &Path, progress: &TransferProgress) -> io::Result<PathBuf> {
    let leaf = remote_path.rsplit('/').next().unwrap_or("file");
    let (target, part) = (resolve_unique_local_path(local_dir, leaf), next_stage_path(local_dir, ".akhterm-part"));
    let ssh_child = route.spawn_rw(&format!("cat -- {}", shell_quote(remote_path)), false)?;
    stream_ssh_to_part(ssh_child, &part, progress).and_then(|()| fs::rename(&part, &target)).inspect_err(|_| {
        let _ = fs::remove_file(&part);
    })?;
    Ok(target)
}

fn split_remote_parent_leaf(remote_path: &str) -> (&str, &str) {
    let trimmed = remote_path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", leaf)) => ("/", leaf),
        Some((parent, leaf)) => (parent, leaf),
        None => (".", trimmed),
    }
}

fn download_dir_from_ssh(route: &SshRoute, remote_path: &str, local_dir: &Path, progress: &TransferProgress) -> io::Result<PathBuf> {
    let (parent, leaf) = split_remote_parent_leaf(remote_path);
    let (target, stage) = (resolve_unique_local_path(local_dir, leaf), next_stage_path(local_dir, ".akhterm-dir"));
    fs::create_dir(&stage)?;
    let ssh = route.spawn_rw(&format!("tar -cf - -C {} -- {}", shell_quote(parent), shell_quote(leaf)), false)?;
    let tar = Command::new("tar").args(["-xf", "-", "-C"]).arg(&stage)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;
    pipe_child_streams(ssh, tar, progress, "SSH folder download failed", "local tar extract failed")
        .and_then(|_| fs::rename(stage.join(leaf), &target))
        .inspect_err(|_| { let _ = fs::remove_dir_all(&stage); })?;
    Ok(target)
}

fn probe_all_remote_entries<'a>(route: &SshRoute, paths: &'a [String]) -> io::Result<(Vec<(&'a str, FileKind)>, u64)> {
    let (mut probed, mut total) = (Vec::with_capacity(paths.len()), 0u64);
    for path in paths {
        let (kind, size) = probe_remote_entry(route, path)?;
        total = total.saturating_add(size);
        probed.push((path.as_str(), kind));
    }
    Ok((probed, total))
}

fn download_one_from_ssh(route: &SshRoute, path: &str, kind: FileKind, local_dir: &Path, progress: &TransferProgress) -> io::Result<PathBuf> {
    if kind.is_dir() {
        download_dir_from_ssh(route, path, local_dir, progress)
    } else {
        download_file_from_ssh(route, path, local_dir, progress)
    }
}

/// Downloads remote files and folders over `route` into `local_dest_dir` with progress/cancel.
pub fn download_ssh_to_local(route: &SshRoute, remote_paths: &[String], local_dest_dir: &Path, progress: &TransferProgress) -> io::Result<Vec<PathBuf>> {
    if !local_dest_dir.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "local destination unavailable"));
    }
    let (probed, total) = probe_all_remote_entries(route, remote_paths)?;
    progress.set_total(total);
    probed.into_iter().map(|(p, k)| download_one_from_ssh(route, p, k, local_dest_dir, progress)).collect()
}

fn copy_same_host_ssh(route: &SshRoute, src: &str, dst_dir: &str) -> io::Result<String> {
    let leaf = sanitize_leaf_name(src.trim_end_matches('/'));
    let body = format!(
        "target=$(resolve_unique \"$d\" {}); cp -a -- {} \"$target\" || exit 1; printf '%s\\n' \"$target\"",
        shell_quote(&leaf), shell_quote(src),
    );
    let script = wrap_remote_dest_script(dst_dir, &body);
    finish_child_checked(route.spawn_rw(&script, false)?, "remote copy failed")
}

fn build_pipe_scripts(src_path: &str, kind: FileKind, dst_dir: &str) -> (String, String) {
    let (parent, leaf) = split_remote_parent_leaf(src_path);
    if kind.is_dir() {
        return (format!("tar -cf - -C {} -- {}", shell_quote(parent), shell_quote(leaf)), build_remote_dir_upload_script(dst_dir, leaf, None));
    }
    (format!("cat -- {}", shell_quote(src_path)), build_remote_file_upload_script(dst_dir, leaf, None))
}

fn pipe_ssh_item(src_route: &SshRoute, src_path: &str, kind: FileKind, dst_route: &SshRoute, dst_dir: &str, progress: &TransferProgress) -> io::Result<String> {
    let (src_script, dst_script) = build_pipe_scripts(src_path, kind, dst_dir);
    let src_child = src_route.spawn_rw(&src_script, false)?;
    let dst_child = dst_route.spawn_rw(&dst_script, true)?;
    pipe_child_streams(src_child, dst_child, progress, "source SSH read failed", "destination SSH write failed")
}

fn transfer_one_ssh(src_route: &SshRoute, src: &str, dst_route: &SshRoute, dst_dir: &str, progress: &TransferProgress) -> io::Result<String> {
    if src_route == dst_route {
        return copy_same_host_ssh(src_route, src, dst_dir);
    }
    let (kind, size) = probe_remote_entry(src_route, src)?;
    progress.set_total(size);
    pipe_ssh_item(src_route, src, kind, dst_route, dst_dir, progress)
}

/// Transfers files and folders from `src_route` to `dst_route` (server-side `cp -a` or direct pipe).
pub fn transfer_ssh_to_ssh(src_route: &SshRoute, src_paths: &[String], dst_route: &SshRoute, dst_dir: &str, progress: &TransferProgress) -> io::Result<Vec<String>> {
    src_paths.iter().map(|src| transfer_one_ssh(src_route, src, dst_route, dst_dir, progress)).collect()
}
