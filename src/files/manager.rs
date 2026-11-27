//! Asynchronous file transfer queue with progress tracking and cancellation.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::ffi::GtkWidget;
use crate::files::local_fs::{compute_local_size, copy_local_items};
use crate::files::ssh_ops::{download_ssh_to_local, transfer_ssh_to_ssh, upload_local_to_ssh};
use crate::files::TransferProgress;
use crate::telemetry::ssh_connection::SshRoute;

const MAX_JOBS: usize = 8;
type OutcomeSlot = Arc<Mutex<Option<Result<Vec<String>, String>>>>;
type WorkFn = Box<dyn FnOnce(TransferProgress) -> Result<Vec<String>, String> + Send>;

/// Direction of an asynchronous transfer job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection { LocalCopy, SshUpload, SshDownload, SshToSsh }

/// Lifecycle state of a transfer job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferStatus { Running, Done, Failed(String), Cancelled }

/// UI-facing snapshot of a transfer job.
#[derive(Clone, Debug)]
pub struct TransferSnapshot {
    pub id: u64,
    pub direction: TransferDirection,
    pub label: String,
    pub destination: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub status: TransferStatus,
    pub local_open_dir: Option<PathBuf>,
    pub target_vte: *mut GtkWidget,
    pub insert_paths: Vec<String>,
    pub insert_after: bool,
}

/// Runtime hooks supplied by the terminal layer for path insertion and status updates.
#[derive(Clone, Copy)]
pub struct TransferHooks {
    pub on_insert_paths: fn(*mut GtkWidget, &[String]),
    pub on_status: fn(&str),
}

static HOOKS: OnceLock<TransferHooks> = OnceLock::new();

struct TransferJob {
    snap: TransferSnapshot,
    prog: TransferProgress,
    result: OutcomeSlot,
    fired: bool,
}

/// Register callbacks used when finished transfers should paste paths or log errors.
pub fn register_transfer_hooks(hooks: TransferHooks) {
    if HOOKS.get().is_none() {
        let _ = HOOKS.set(hooks);
    }
}

impl TransferJob {
    fn snapshot(&self) -> TransferSnapshot {
        let (done, total) = self.prog.snapshot();
        let mut out = self.snap.clone();
        out.bytes_done = done;
        out.bytes_total = total;
        out
    }

    fn finish(&mut self, outcome: Result<Vec<String>, String>) -> bool {
        if self.snap.status == TransferStatus::Cancelled {
            self.fired = true;
            return false;
        }
        let cancelled = self.prog.is_cancelled();
        self.snap.status = match outcome {
            Ok(paths) if !cancelled => { self.snap.insert_paths = paths; TransferStatus::Done }
            Ok(_) => TransferStatus::Cancelled,
            Err(_) if cancelled => TransferStatus::Cancelled,
            Err(err) => TransferStatus::Failed(err),
        };
        self.notify();
        true
    }

    fn notify(&mut self) {
        if self.fired {
            return;
        }
        self.fired = true;
        let Some(hooks) = HOOKS.get() else { return };
        let vte = self.snap.target_vte;
        if self.snap.status == TransferStatus::Done && self.snap.insert_after && !vte.is_null() {
            (hooks.on_insert_paths)(vte, &self.snap.insert_paths);
        } else if let TransferStatus::Failed(ref err) = self.snap.status {
            (hooks.on_status)(err);
        }
    }
}

/// Manages active and recent file transfer jobs without blocking GTK.
#[derive(Default)]
pub struct TransferManager {
    next_id: u64,
    jobs: Vec<TransferJob>,
}

fn local_label(sources: &[PathBuf]) -> String {
    if sources.len() == 1 {
        if let Some(name) = sources[0].file_name().and_then(|n| n.to_str()) {
            return name.to_owned();
        }
    }
    format!("{} items", sources.len())
}

fn remote_label(paths: &[String]) -> String {
    if paths.len() == 1 {
        if let Some(name) = paths[0].rsplit('/').find(|s| !s.is_empty()) {
            return name.to_owned();
        }
    }
    format!("{} items", paths.len())
}

fn to_strings(res: std::io::Result<Vec<PathBuf>>) -> Result<Vec<String>, String> {
    let paths = res.map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        out.push(p.to_string_lossy().into_owned());
    }
    Ok(out)
}

fn run_bg(prog: TransferProgress, work: WorkFn) -> OutcomeSlot {
    let slot: OutcomeSlot = Arc::new(Mutex::new(None));
    let out = Arc::clone(&slot);
    thread::spawn(move || {
        let res = work(prog);
        if let Ok(mut g) = out.lock() {
            *g = Some(res);
        }
    });
    slot
}

fn init_snap(id: u64, dir: TransferDirection, label: String, dest: String, total: u64, open: Option<PathBuf>, vte: *mut GtkWidget, insert: bool) -> TransferSnapshot {
    TransferSnapshot {
        id, direction: dir, label, destination: dest, bytes_done: 0, bytes_total: total,
        status: TransferStatus::Running, local_open_dir: open, target_vte: vte,
        insert_paths: Vec::new(), insert_after: insert,
    }
}

fn trim_finished(jobs: &mut Vec<TransferJob>) {
    while jobs.len() > MAX_JOBS {
        let Some(i) = jobs.iter().position(|j| j.snap.status != TransferStatus::Running) else {
            break;
        };
        jobs.remove(i);
    }
}

impl TransferManager {
    fn launch(&mut self, dir: TransferDirection, label: String, dest: String, total: u64, open: Option<PathBuf>, vte: *mut GtkWidget, insert: bool, work: WorkFn) -> u64 {
        self.next_id += 1;
        let snap = init_snap(self.next_id, dir, label, dest, total, open, vte, insert);
        let prog = TransferProgress::new(total);
        let result = run_bg(prog.clone(), work);
        self.jobs.push(TransferJob { snap, prog, result, fired: false });
        trim_finished(&mut self.jobs);
        self.next_id
    }

    /// Start an asynchronous local copy into `dest_dir`.
    pub fn start_local_copy(&mut self, sources: Vec<PathBuf>, dest_dir: PathBuf, vte: *mut GtkWidget, insert: bool) -> u64 {
        let total = compute_local_size(&sources);
        let label = local_label(&sources);
        let dest = dest_dir.to_string_lossy().into_owned();
        let open = Some(dest_dir.clone());
        let work = Box::new(move |p| to_strings(copy_local_items(&sources, &dest_dir, &p)));
        self.launch(TransferDirection::LocalCopy, label, dest, total, open, vte, insert, work)
    }

    /// Start an asynchronous upload from local filesystem to `route` on `host`.
    pub fn start_ssh_upload(
        &mut self, route: SshRoute, host: String, sources: Vec<PathBuf>,
        dest_dir: String, to_tmp: bool, vte: *mut GtkWidget, insert: bool,
    ) -> u64 {
        let total = compute_local_size(&sources);
        let label = local_label(&sources);
        let dest = if to_tmp { format!("{host}:/tmp") } else { format!("{host}:{dest_dir}") };
        let work = Box::new(move |p| upload_local_to_ssh(&route, &sources, &dest_dir, to_tmp, &p).map_err(|e| e.to_string()));
        self.launch(TransferDirection::SshUpload, label, dest, total, None, vte, insert, work)
    }

    /// Start an asynchronous download from `route` on `host` into `dest_dir`.
    pub fn start_ssh_download(
        &mut self, route: SshRoute, host: String, remote_paths: Vec<String>,
        dest_dir: PathBuf, vte: *mut GtkWidget, insert: bool,
    ) -> u64 {
        let label = remote_label(&remote_paths);
        let dest = format!("{host} → {}", dest_dir.to_string_lossy());
        let open = Some(dest_dir.clone());
        let work = Box::new(move |p| to_strings(download_ssh_to_local(&route, &remote_paths, &dest_dir, &p)));
        self.launch(TransferDirection::SshDownload, label, dest, 0, open, vte, insert, work)
    }

    /// Start a streamed SSH-to-SSH transfer bridging through local memory.
    pub fn start_ssh_to_ssh(
        &mut self, src_route: SshRoute, src_paths: Vec<String>, dst_route: SshRoute,
        dst_host: String, dst_dir: String, vte: *mut GtkWidget, insert: bool,
    ) -> u64 {
        let label = remote_label(&src_paths);
        let dest = format!("{dst_host}:{dst_dir}");
        let work = Box::new(move |p| transfer_ssh_to_ssh(&src_route, &src_paths, &dst_route, &dst_dir, &p).map_err(|e| e.to_string()));
        self.launch(TransferDirection::SshToSsh, label, dest, 0, None, vte, insert, work)
    }

    /// Poll worker results, update statuses, fire hooks, and return newly settled snapshots.
    pub fn poll_updates(&mut self) -> Vec<TransferSnapshot> {
        let mut completed = Vec::new();
        for job in &mut self.jobs {
            if let Some(outcome) = job.result.lock().ok().and_then(|mut g| g.take()) {
                if job.finish(outcome) {
                    completed.push(job.snapshot());
                }
            }
        }
        completed
    }

    /// Request cancellation of an active transfer job.
    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|j| j.snap.id == id && j.snap.status == TransferStatus::Running) else {
            return false;
        };
        job.prog.cancel();
        job.snap.status = TransferStatus::Cancelled;
        true
    }

    /// Remove a finished or cancelled job from history.
    pub fn dismiss(&mut self, id: u64) {
        self.jobs.retain(|j| j.snap.id != id);
    }

    /// Poll pending outcomes and return the most recent active or historical snapshot.
    pub fn latest_snapshot(&mut self) -> Option<TransferSnapshot> {
        let _ = self.poll_updates();
        self.jobs
            .iter()
            .rev()
            .find(|j| j.snap.status == TransferStatus::Running)
            .or_else(|| self.jobs.last())
            .map(TransferJob::snapshot)
    }

    /// Whether any job is currently running.
    pub fn has_running(&self) -> bool {
        self.jobs.iter().any(|j| j.snap.status == TransferStatus::Running)
    }
}
