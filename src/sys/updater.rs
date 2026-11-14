#![allow(dead_code)]
//! Self-updater: strategy detection, curl/sha256sum verification, atomic install, and restart.

use std::{env, io, thread};
use std::ffi::{c_char, c_int, c_void, CString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex};

use super::paths;
use super::updater_manifest::{
    find_asset, find_sha256sums_asset, parse_release_manifest, parse_sha256sums, AssetKind,
    ReleaseAsset, ReleaseManifest, SemVer,
};

pub const DEFAULT_MANIFEST_URL: &str =
    "https://api.github.com/repos/akhtyamovpavel/AkhTerm/releases/latest";

const W_OK: c_int = 2;
static STAGE_SEQ: AtomicU64 = AtomicU64::new(1);
static PENDING_RESTART_EXE: Mutex<Option<PathBuf>> = Mutex::new(None);
pub type GSourceFunc = extern "C" fn(*mut c_void) -> c_int;

pub fn is_auto_check_disabled() -> bool {
    env::var("AKHTERM_DISABLE_UPDATE_CHECK")
        .ok()
        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"))
}

fn manifest_url_override() -> Option<String> {
    env::var("AKHTERM_UPDATE_MANIFEST_URL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

pub fn resolve_manifest_url() -> (String, bool) {
    match manifest_url_override() {
        Some(url) => (url, true),
        None => (DEFAULT_MANIFEST_URL.to_owned(), false),
    }
}

pub fn resolve_target_exe() -> io::Result<PathBuf> {
    if let Some(custom) = env::var_os("AKHTERM_UPDATE_TARGET_EXE") {
        if !custom.is_empty() {
            return Ok(PathBuf::from(custom));
        }
    }
    env::current_exe()
}

#[cfg(test)]
type PrivilegedRunner = Arc<dyn Fn(&[String]) -> Result<(), String> + Send + Sync>;

struct SchedulerInner {
    state: UpdateState,
    last_available: Option<(String, ReleaseManifest)>,
    #[cfg(test)]
    privileged_runner: Option<PrivilegedRunner>,
}

#[derive(Clone)]
pub struct UpdateScheduler {
    inner: Arc<Mutex<SchedulerInner>>,
}

impl UpdateScheduler {
    pub fn new() -> Self {
        let inner = SchedulerInner {
            state: UpdateState::Idle,
            last_available: None,
            #[cfg(test)]
            privileged_runner: None,
        };
        Self { inner: Arc::new(Mutex::new(inner)) }
    }

    #[cfg(test)]
    pub(crate) fn with_privileged_runner_for_test(runner: PrivilegedRunner) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SchedulerInner {
                state: UpdateState::Idle,
                last_available: None,
                privileged_runner: Some(runner),
            })),
        }
    }

    pub fn state(&self) -> UpdateState {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.state.clone()
    }

    pub fn check_async(&self, current_version: &str, manual: bool, notify_idle: Option<GSourceFunc>) {
        if (!manual && is_auto_check_disabled()) || !self.try_enter_checking() {
            return;
        }
        self.spawn_check_thread(current_version.to_owned(), manual, notify_idle);
    }

    fn spawn_check_thread(&self, current: String, manual: bool, notify_idle: Option<GSourceFunc>) {
        notify_ui(notify_idle);
        let scheduler = self.clone();
        thread::spawn(move || {
            scheduler.run_check(&current, manual);
            notify_ui(notify_idle);
        });
    }

    fn try_enter_checking(&self) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if is_busy_state(&guard.state) {
            return false;
        }
        guard.state = UpdateState::Checking;
        true
    }

    pub fn install_async(&self, notify_idle: Option<GSourceFunc>) {
        let Some((current_version, manifest)) = self.begin_install_transition() else {
            return;
        };
        notify_ui(notify_idle);
        let scheduler = self.clone();
        thread::spawn(move || {
            scheduler.run_install(current_version, manifest, notify_idle);
            notify_ui(notify_idle);
        });
    }

    pub fn dismiss_current(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ver) = extract_latest_version(&guard.state) {
            if let Err(err) = save_dismissed_version(&ver) {
                eprintln!("AkhTerm: failed to persist dismissed update version: {err}");
            }
        }
        guard.state = UpdateState::Idle;
    }

    fn begin_install_transition(&self) -> Option<(String, ReleaseManifest)> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if is_busy_state(&guard.state) {
            return None;
        }
        let pair = available_or_cached_manifest(&guard)?;
        guard.state = UpdateState::Downloading {
            current_version: pair.0.clone(),
            latest_version: pair.1.tag_name.clone(),
            release_url: pair.1.html_url.clone(),
        };
        Some(pair)
    }

    fn run_check(&self, current_version: &str, manual: bool) {
        let outcome = evaluate_remote_release(current_version, manual);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        apply_check_outcome(&mut guard, current_version, manual, outcome);
        drop(guard);
    }

    fn run_install(&self, current: String, manifest: ReleaseManifest, notify_idle: Option<GSourceFunc>) {
        let latest = manifest.tag_name.clone();
        let url = manifest.html_url.clone();
        let mark_installing = || {
            self.set_state(UpdateState::Installing {
                current_version: current.clone(),
                latest_version: latest.clone(),
                release_url: url.clone(),
            });
            notify_ui(notify_idle);
        };
        let res = execute_install(&manifest, mark_installing, |cmd| self.run_pkexec(cmd));
        self.set_state(install_result_state(current, latest, url, res));
    }

    fn set_state(&self, next: UpdateState) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).state = next;
    }

    fn run_pkexec(&self, cmd: &[String]) -> Result<(), String> {
        #[cfg(test)]
        if let Some(runner) = self.test_runner() {
            return runner(cmd);
        }
        run_system_command(cmd)
    }

    #[cfg(test)]
    fn test_runner(&self) -> Option<PrivilegedRunner> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.privileged_runner.clone()
    }
}

impl Default for UpdateScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    Available { current_version: String, latest_version: String, release_url: String, manifest: ReleaseManifest },
    UpToDate { current_version: String },
    Downloading { current_version: String, latest_version: String, release_url: String },
    Installing { current_version: String, latest_version: String, release_url: String },
    ReadyToRestart { current_version: String, latest_version: String, release_url: String, exe_path: PathBuf },
    Failed { current_version: String, latest_version: Option<String>, release_url: String, error: String },
}

impl UpdateState {
    fn is_busy(&self) -> bool {
        matches!(self, Self::Checking | Self::Downloading { .. } | Self::Installing { .. })
    }
}

enum CheckOutcome {
    Available(ReleaseManifest),
    UpToDate,
    Dismissed,
    Failed(String),
}

fn apply_check_outcome(
    guard: &mut SchedulerInner,
    current_version: &str,
    manual: bool,
    outcome: CheckOutcome,
) {
    match outcome {
        CheckOutcome::Available(manifest) => {
            guard.last_available = Some((current_version.to_owned(), manifest.clone()));
            guard.state = available_state(current_version, manifest);
        }
        CheckOutcome::UpToDate => {
            guard.state = up_to_date_or_idle(current_version, manual);
        }
        CheckOutcome::Dismissed => guard.state = UpdateState::Idle,
        CheckOutcome::Failed(err) => guard.state = check_failure_state(current_version, manual, err),
    }
}

fn up_to_date_or_idle(current_version: &str, manual: bool) -> UpdateState {
    if manual {
        UpdateState::UpToDate { current_version: current_version.to_owned() }
    } else {
        UpdateState::Idle
    }
}

fn available_or_cached_manifest(guard: &SchedulerInner) -> Option<(String, ReleaseManifest)> {
    match &guard.state {
        UpdateState::Available { current_version, manifest, .. } => {
            Some((current_version.clone(), manifest.clone()))
        }
        UpdateState::Failed { .. } => guard.last_available.clone(),
        _ => None,
    }
}

fn available_state(current_version: &str, manifest: ReleaseManifest) -> UpdateState {
    UpdateState::Available {
        current_version: current_version.to_owned(),
        latest_version: manifest.tag_name.clone(),
        release_url: manifest.html_url.clone(),
        manifest,
    }
}

fn check_failure_state(current_version: &str, manual: bool, error: String) -> UpdateState {
    if !manual {
        return UpdateState::Idle;
    }
    UpdateState::Failed {
        current_version: current_version.to_owned(),
        latest_version: None,
        release_url: String::new(),
        error,
    }
}

fn install_result_state(
    current_version: String,
    latest_version: String,
    release_url: String,
    res: Result<PathBuf, String>,
) -> UpdateState {
    match res {
        Ok(exe_path) => {
            UpdateState::ReadyToRestart { current_version, latest_version, release_url, exe_path }
        }
        Err(error) => UpdateState::Failed {
            current_version,
            latest_version: Some(latest_version),
            release_url,
            error,
        },
    }
}

fn is_busy_state(state: &UpdateState) -> bool {
    state.is_busy()
}

fn extract_latest_version(state: &UpdateState) -> Option<String> {
    match state {
        UpdateState::Available { latest_version, .. }
        | UpdateState::Downloading { latest_version, .. }
        | UpdateState::Installing { latest_version, .. }
        | UpdateState::ReadyToRestart { latest_version, .. } => Some(latest_version.clone()),
        UpdateState::Failed { latest_version, .. } => latest_version.clone(),
        UpdateState::Idle | UpdateState::Checking | UpdateState::UpToDate { .. } => None,
    }
}

fn notify_ui(cb: Option<GSourceFunc>) {
    let Some(func) = cb else {
        return;
    };
    unsafe {
        super::ffi::g_idle_add(func, std::ptr::null_mut());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallStrategy {
    UserBinary { target_exe: PathBuf },
    DebPackage { target_exe: PathBuf },
    RpmPackage { target_exe: PathBuf },
    Unsupported { target_exe: PathBuf, reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemPackageKind {
    Deb,
    Rpm,
}

extern "C" {
    fn access(pathname: *const c_char, mode: c_int) -> c_int;
    fn execv(path: *const c_char, argv: *const *const c_char) -> c_int;
}

pub fn is_parent_writable(exe_path: &Path) -> bool {
    let parent = match exe_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let Ok(c_path) = CString::new(parent.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { access(c_path.as_ptr(), W_OK) == 0 }
}

pub fn detect_install_strategy(exe_path: &Path) -> InstallStrategy {
    if is_parent_writable(exe_path) {
        return InstallStrategy::UserBinary { target_exe: exe_path.to_path_buf() };
    }
    let has_dpkg_pkg = command_succeeds("dpkg", &["-s", "akhterm"]);
    let has_rpm_pkg = !has_dpkg_pkg && command_succeeds("rpm", &["-q", "akhterm"]);
    let has_dpkg_bin = Path::new("/usr/bin/dpkg").exists();
    let has_rpm_bin = Path::new("/usr/bin/rpm").exists();
    classify_non_writable_strategy(exe_path, has_dpkg_pkg, has_rpm_pkg, has_dpkg_bin, has_rpm_bin)
}

pub fn classify_non_writable_strategy(
    exe_path: &Path, has_dpkg_pkg: bool, has_rpm_pkg: bool, has_dpkg_bin: bool, has_rpm_bin: bool,
) -> InstallStrategy {
    let target_exe = exe_path.to_path_buf();
    if has_dpkg_pkg || (!has_rpm_pkg && has_dpkg_bin) {
        return InstallStrategy::DebPackage { target_exe };
    }
    if has_rpm_pkg || has_rpm_bin {
        return InstallStrategy::RpmPackage { target_exe };
    }
    let reason = "read-only executable directory and neither dpkg nor rpm was found".to_owned();
    InstallStrategy::Unsupported { target_exe, reason }
}

fn command_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
static TEST_STATE_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

fn updater_state_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(dir) = TEST_STATE_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return dir;
    }
    paths::state_dir()
}

pub fn dismissed_version_path() -> PathBuf {
    updater_state_dir().join("update-dismissed.txt")
}

pub fn read_dismissed_version() -> Option<String> {
    let content = fs::read_to_string(dismissed_version_path()).ok()?;
    let trimmed = content.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub fn save_dismissed_version(ver: &str) -> io::Result<()> {
    paths::write_state_file(&dismissed_version_path(), &format!("{}\n", ver.trim()))
}

fn is_version_dismissed(manifest: &ReleaseManifest) -> bool {
    let Some(saved) = read_dismissed_version() else {
        return false;
    };
    saved == manifest.tag_name
        || SemVer::parse(&saved).is_ok_and(|sv| sv == manifest.version)
}

fn evaluate_remote_release(current_version: &str, manual: bool) -> CheckOutcome {
    let Ok(current) = SemVer::parse(current_version) else {
        return CheckOutcome::Failed(format!("invalid current version '{current_version}'"));
    };
    let (url, allow_file) = resolve_manifest_url();
    let manifest = match curl_fetch_text(&url, allow_file).and_then(|b| parse_release_manifest(&b)) {
        Ok(m) => m,
        Err(e) => return CheckOutcome::Failed(e),
    };
    if !manifest.version.is_newer_than(&current) {
        return CheckOutcome::UpToDate;
    }
    if !manual && is_version_dismissed(&manifest) {
        return CheckOutcome::Dismissed;
    }
    CheckOutcome::Available(manifest)
}

pub fn validate_download_url(url: &str, allow_file: bool) -> Result<(), String> {
    if url.starts_with("https://") || (allow_file && url.starts_with("file://")) {
        return Ok(());
    }
    Err(format!("blocked non-HTTPS update URL: {url}"))
}

pub fn build_curl_args(
    url: &str,
    out_path: Option<&Path>,
    allow_file: bool,
) -> Result<Vec<String>, String> {
    validate_download_url(url, allow_file)?;
    let mut args = base_curl_args(allow_file);
    if let Some(dest) = out_path {
        args.push("-o".to_owned());
        args.push(dest.to_string_lossy().into_owned());
    }
    args.push(url.to_owned());
    Ok(args)
}

fn base_curl_args(allow_file: bool) -> Vec<String> {
    let proto = if allow_file { "=file,https" } else { "=https" };
    [
        "-fsSL", "--connect-timeout", "10", "--max-time", "120",
        "--proto", proto, "--proto-redir", "=https", "-A", "AkhTerm-Updater/1.0",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn curl_fetch_text(url: &str, allow_file: bool) -> Result<String, String> {
    let args = build_curl_args(url, None, allow_file)?;
    let out = run_curl(&args)?;
    String::from_utf8(out.stdout).map_err(|e| format!("invalid UTF-8 manifest response: {e}"))
}

fn curl_download_file(url: &str, dest: &Path, allow_file: bool) -> Result<(), String> {
    let args = build_curl_args(url, Some(dest), allow_file)?;
    let _ = run_curl(&args)?;
    Ok(())
}

fn run_curl(args: &[String]) -> Result<std::process::Output, String> {
    let out = Command::new("curl")
        .args(args)
        .output()
        .map_err(|e| format!("failed to launch curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(format!("curl failed ({}): {err}", out.status));
    }
    Ok(out)
}

fn ensure_private_dir(path: &Path, fresh: bool) -> Result<(), String> {
    if fresh {
        DirBuilder::new().mode(0o700).create(path).map_err(|e| format!("create stage: {e}"))?;
    } else {
        fs::create_dir_all(path).map_err(|e| format!("create dir: {e}"))?;
    }
    fs::set_permissions(path, PermissionsExt::from_mode(0o700))
        .map_err(|e| format!("chmod dir: {e}"))
}

struct TempStageDir {
    path: PathBuf,
}

impl TempStageDir {
    fn create() -> Result<Self, String> {
        let base = updater_state_dir().join("updates");
        ensure_private_dir(&base, false)?;
        let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = base.join(format!("stage-{}-{seq}", std::process::id()));
        ensure_private_dir(&dir, true)?;
        Ok(Self { path: dir })
    }
}

impl Drop for TempStageDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn compute_file_sha256(path: &Path) -> Result<String, String> {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("failed to execute sha256sum: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(format!("sha256sum failed: {err}"));
    }
    parse_sha256sum_stdout(&out.stdout)
}

fn parse_sha256sum_stdout(stdout: &[u8]) -> Result<String, String> {
    let text = String::from_utf8_lossy(stdout);
    let hex = text.split_whitespace().next().unwrap_or("");
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("unexpected sha256sum output format".to_owned());
    }
    Ok(hex.to_ascii_lowercase())
}

pub fn constant_time_hex_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn sanitize_asset_filename(name: &str) -> Result<&str, String> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0']);
    if bad {
        return Err(format!("unsafe release asset filename: '{name}'"));
    }
    Ok(name)
}

fn download_and_verify_asset(
    stage: &Path,
    manifest: &ReleaseManifest,
    asset: &ReleaseAsset,
    allow_file: bool,
) -> Result<PathBuf, String> {
    let sums_asset = find_sha256sums_asset(manifest)
        .ok_or_else(|| "release is missing SHA256SUMS asset".to_owned())?;
    let sums_path = stage.join("SHA256SUMS");
    curl_download_file(&sums_asset.browser_download_url, &sums_path, allow_file)?;
    let sums_text = fs::read_to_string(&sums_path).map_err(|e| format!("read SHA256SUMS: {e}"))?;
    let expected = parse_sha256sums(&sums_text)
        .remove(&asset.name)
        .ok_or_else(|| format!("SHA256SUMS does not contain {}", asset.name))?;
    verify_downloaded_file(stage, asset, &expected, allow_file)
}

fn verify_downloaded_file(
    stage: &Path, asset: &ReleaseAsset, expected_hex: &str, allow_file: bool,
) -> Result<PathBuf, String> {
    let archive_path = stage.join(sanitize_asset_filename(&asset.name)?);
    curl_download_file(&asset.browser_download_url, &archive_path, allow_file)?;
    verify_file_digest(&archive_path, expected_hex, &asset.name)?;
    Ok(archive_path)
}

fn verify_file_digest(path: &Path, expected_hex: &str, name: &str) -> Result<(), String> {
    let actual = compute_file_sha256(path)?;
    if !constant_time_hex_eq(&actual, expected_hex) {
        return Err(format!("SHA-256 mismatch for {name}: expected {expected_hex}, got {actual}"));
    }
    Ok(())
}

fn execute_install(
    manifest: &ReleaseManifest,
    on_installing: impl FnOnce(),
    pkexec_runner: impl FnOnce(&[String]) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let exe = resolve_target_exe().map_err(|e| format!("resolve current executable: {e}"))?;
    let strategy = detect_install_strategy(&exe);
    let (_, allow_file) = resolve_manifest_url();
    let stage = TempStageDir::create()?;
    dispatch_install_strategy(&stage.path, strategy, manifest, allow_file, on_installing, pkexec_runner)
}

fn dispatch_install_strategy(
    stage: &Path, strategy: InstallStrategy, manifest: &ReleaseManifest, allow_file: bool,
    on_installing: impl FnOnce(), pkexec_runner: impl FnOnce(&[String]) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let (pkg_kind, target_exe) = match strategy {
        InstallStrategy::UserBinary { target_exe } => {
            install_user_tarball(stage, manifest, allow_file, &target_exe, on_installing)?;
            return Ok(target_exe);
        }
        InstallStrategy::DebPackage { target_exe } => (SystemPackageKind::Deb, target_exe),
        InstallStrategy::RpmPackage { target_exe } => (SystemPackageKind::Rpm, target_exe),
        InstallStrategy::Unsupported { reason, .. } => return Err(reason),
    };
    install_sys_pkg(stage, manifest, pkg_kind, allow_file, on_installing, pkexec_runner)?;
    Ok(target_exe)
}

fn install_user_tarball(
    stage: &Path, manifest: &ReleaseManifest, allow_file: bool, target_exe: &Path,
    on_installing: impl FnOnce(),
) -> Result<(), String> {
    let asset = find_asset(manifest, AssetKind::Tarball)
        .ok_or_else(|| "no portable Linux tarball asset for this arch".to_owned())?;
    let archive = download_and_verify_asset(stage, manifest, asset, allow_file)?;
    on_installing();
    let staged_bin = extract_and_verify_binary(stage, &archive)?;
    install_user_binary(&staged_bin, target_exe)
}

fn install_sys_pkg(
    stage: &Path, manifest: &ReleaseManifest, pkg_kind: SystemPackageKind, allow_file: bool,
    on_installing: impl FnOnce(), pkexec_runner: impl FnOnce(&[String]) -> Result<(), String>,
) -> Result<(), String> {
    let asset_kind = match pkg_kind {
        SystemPackageKind::Deb => AssetKind::Deb,
        SystemPackageKind::Rpm => AssetKind::Rpm,
    };
    let asset = find_asset(manifest, asset_kind)
        .ok_or_else(|| format!("no {asset_kind:?} package asset for this arch"))?;
    let pkg_path = download_and_verify_asset(stage, manifest, asset, allow_file)?;
    on_installing();
    pkexec_runner(&build_privileged_install_command(pkg_kind, &pkg_path))
}

fn extract_and_verify_binary(stage: &Path, archive: &Path) -> Result<PathBuf, String> {
    let extract_dir = stage.join("extract");
    ensure_private_dir(&extract_dir, true)?;
    let out = Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(&extract_dir)
        .output().map_err(|e| format!("launch tar: {e}"))?;
    if !out.status.success() {
        return Err(format!("tar extraction failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let bin = locate_extracted_binary(&extract_dir)?;
    verify_extracted_executable(&bin)?;
    Ok(bin)
}

fn verify_extracted_executable(bin: &Path) -> Result<(), String> {
    fs::set_permissions(bin, PermissionsExt::from_mode(0o755))
        .map_err(|e| format!("chmod extracted binary: {e}"))?;
    let ver = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("verify extracted binary --version: {e}"))?;
    if !ver.status.success() {
        return Err(format!("extracted binary --version failed ({})", ver.status));
    }
    Ok(())
}

fn locate_extracted_binary(extract_dir: &Path) -> Result<PathBuf, String> {
    let is_file = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file());
    let direct = extract_dir.join("akhterm");
    if is_file(&direct) {
        return Ok(direct);
    }
    for entry in fs::read_dir(extract_dir).map_err(|e| format!("scan extract dir: {e}"))?.flatten() {
        let candidate = entry.path().join("akhterm");
        if is_file(&candidate) {
            return Ok(candidate);
        }
    }
    Err("tarball archive did not contain an 'akhterm' binary".to_owned())
}

fn install_user_binary(staged_bin: &Path, target_exe: &Path) -> Result<(), String> {
    let parent = target_exe
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(".akhterm.update.{}.{seq}", std::process::id()));
    let copy_res = write_executable_copy(staged_bin, &tmp_path)
        .and_then(|()| fs::rename(&tmp_path, target_exe));
    if let Err(err) = copy_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("atomic binary replacement failed: {err}"));
    }
    Ok(())
}

fn write_executable_copy(src: &Path, dst: &Path) -> io::Result<()> {
    let mut reader = fs::File::open(src)?;
    let mut writer = OpenOptions::new().create_new(true).write(true).mode(0o755).open(dst)?;
    io::copy(&mut reader, &mut writer)?;
    writer.sync_all()?;
    fs::set_permissions(dst, PermissionsExt::from_mode(0o755))
}

pub fn build_privileged_install_command(kind: SystemPackageKind, pkg_path: &Path) -> Vec<String> {
    let pkg = pkg_path.to_string_lossy().into_owned();
    match kind {
        SystemPackageKind::Deb => vec!["pkexec".to_owned(), "dpkg".to_owned(), "-i".to_owned(), pkg],
        SystemPackageKind::Rpm => vec!["pkexec".to_owned(), "rpm".to_owned(), "-U".to_owned(), pkg],
    }
}

fn run_system_command(cmd: &[String]) -> Result<(), String> {
    let (program, args) = cmd.split_first().ok_or("empty privileged command")?;
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("failed to launch {program}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(format!("{program} failed ({}): {err}", out.status));
    }
    Ok(())
}

pub fn request_restart_exe(path: PathBuf) {
    *PENDING_RESTART_EXE.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
}

pub fn take_pending_restart_exe() -> Option<PathBuf> {
    PENDING_RESTART_EXE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub fn exec_replace_process(exe: &Path) -> io::Error {
    let Ok(c_exe) = CString::new(exe.as_os_str().as_bytes()) else {
        return io::Error::new(io::ErrorKind::InvalidInput, "executable path contains NUL");
    };
    let argv = [c_exe.as_ptr(), std::ptr::null()];
    unsafe {
        execv(c_exe.as_ptr(), argv.as_ptr());
    }
    io::Error::last_os_error()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn set(pairs: &[(&'static str, Option<&str>)], state_dir: Option<&Path>) -> Self {
            *TEST_STATE_DIR.lock().unwrap_or_else(|e| e.into_inner()) = state_dir.map(Path::to_path_buf);
            let mut saved = Vec::new();
            for &(k, v) in pairs {
                saved.push((k, env::var_os(k)));
                match v {
                    Some(val) => env::set_var(k, val),
                    None => env::remove_var(k),
                }
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, orig) in self.saved.drain(..).rev() {
                match orig {
                    Some(val) => env::set_var(k, val),
                    None => env::remove_var(k),
                }
            }
            *TEST_STATE_DIR.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    struct TempTestDir(PathBuf);

    impl TempTestDir {
        fn new(label: &str) -> Self {
            let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
            let name = format!("akhterm-upd-{label}-{}-{seq}", std::process::id());
            let dir = env::temp_dir().join(name);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FileUpdateFixture {
        _dir: TempTestDir,
        state_home: PathBuf,
        target_exe: PathBuf,
        manifest_url: String,
    }

    impl FileUpdateFixture {
        fn new(label: &str, sha_override: Option<String>, include_sums: bool) -> Self {
            let dir = TempTestDir::new(label);
            let state_home = dir.0.join("state");
            let bin_dir = dir.0.join("bin");
            fs::create_dir_all(&bin_dir).unwrap();
            let target_exe = write_fake_exe(&bin_dir.join("akhterm"), "1.0.0");
            let archive_name = format!("akhterm_1.1.0_linux_{}.tar.gz", env::consts::ARCH);
            let (archive_path, real_sha) = write_mock_tarball(&dir.0, &archive_name, "1.1.0");
            let sums_path = dir.0.join("SHA256SUMS");
            let sha = sha_override.unwrap_or(real_sha);
            fs::write(&sums_path, format!("{sha}  {archive_name}\n")).unwrap();
            let sums_opt = include_sums.then_some(sums_path.as_path());
            let manifest_path =
                write_file_manifest(&dir.0, "v1.1.0", &archive_name, &archive_path, sums_opt);
            let manifest_url = format!("file://{}", manifest_path.display());
            Self { _dir: dir, state_home, target_exe, manifest_url }
        }

        fn env_guard(&self, disable_flag: Option<&str>) -> EnvGuard {
            EnvGuard::set(
                &[
                    ("AKHTERM_UPDATE_MANIFEST_URL", Some(&self.manifest_url)),
                    ("AKHTERM_UPDATE_TARGET_EXE", Some(self.target_exe.to_str().unwrap())),
                    ("AKHTERM_DISABLE_UPDATE_CHECK", disable_flag),
                ],
                Some(&self.state_home),
            )
        }
    }

    fn write_fake_exe(path: &Path, version: &str) -> PathBuf {
        fs::write(path, format!("#!/bin/sh\necho 'AkhTerm {version}'\n")).unwrap();
        fs::set_permissions(path, PermissionsExt::from_mode(0o755)).unwrap();
        path.to_path_buf()
    }

    fn wait_until_settled(scheduler: &UpdateScheduler) -> UpdateState {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let st = scheduler.state();
            if !is_busy_state(&st) {
                return st;
            }
            assert!(Instant::now() < deadline, "timed out waiting for updater: {st:?}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn write_mock_tarball(root: &Path, archive_name: &str, version_str: &str) -> (PathBuf, String) {
        let stage = root.join("tar-stage");
        let inner = stage.join(format!("akhterm-{version_str}-linux-{}", env::consts::ARCH));
        fs::create_dir_all(&inner).unwrap();
        write_fake_exe(&inner.join("akhterm"), version_str);
        let archive = root.join(archive_name);
        let status = Command::new("tar")
            .args(["-C", stage.to_str().unwrap(), "-czf", archive.to_str().unwrap()])
            .arg(inner.file_name().unwrap())
            .status()
            .unwrap();
        assert!(status.success());
        let sha = compute_file_sha256(&archive).unwrap();
        (archive, sha)
    }

    fn write_file_manifest(
        dir: &Path,
        tag: &str,
        archive_name: &str,
        archive_path: &Path,
        sums_path: Option<&Path>,
    ) -> PathBuf {
        let sums_json = sums_path.map_or_else(String::new, |p| {
            format!("{{\"name\":\"SHA256SUMS\",\"browser_download_url\":\"file://{}\"}},", p.display())
        });
        let url = format!("file://{}", archive_path.display());
        let json = format!(
            "{{\"tag_name\":\"{tag}\",\"html_url\":\"https://example.test/releases/{tag}\",\"assets\":[{sums_json}{{\"name\":\"{archive_name}\",\"browser_download_url\":\"{url}\"}}]}}"
        );
        let path = dir.join("latest.json");
        fs::write(&path, json).unwrap();
        path
    }

    #[test]
    fn curl_args_enforce_https_in_production_and_block_downgrades() {
        assert!(build_curl_args("file:///etc/passwd", None, false).is_err());
        assert!(build_curl_args("http://example.com/a", None, false).is_err());
        assert!(build_curl_args("http://example.com/a", None, true).is_err());
        let args = build_curl_args(DEFAULT_MANIFEST_URL, None, false).unwrap();
        assert!(args.windows(2).any(|w| w == ["--proto", "=https"]));
        assert!(args.windows(2).any(|w| w == ["--proto-redir", "=https"]));
    }

    #[test]
    fn writable_parent_uses_user_binary_strategy() {
        let dir = TempTestDir::new("strategy");
        let exe = write_fake_exe(&dir.0.join("akhterm"), "1.0.0");
        assert!(is_parent_writable(&exe));
        assert_eq!(detect_install_strategy(&exe), InstallStrategy::UserBinary { target_exe: exe });
    }

    #[test]
    fn non_writable_parent_selects_deb_rpm_or_unsupported() {
        let sys = Path::new("/usr/bin/akhterm");
        assert_eq!(
            classify_non_writable_strategy(sys, true, false, true, false),
            InstallStrategy::DebPackage { target_exe: sys.to_path_buf() }
        );
        assert_eq!(
            classify_non_writable_strategy(sys, false, true, true, true),
            InstallStrategy::RpmPackage { target_exe: sys.to_path_buf() }
        );
        assert!(matches!(
            classify_non_writable_strategy(sys, false, false, false, false),
            InstallStrategy::Unsupported { .. }
        ));
    }

    #[test]
    fn file_integration_replaces_binary_atomically() {
        let _lock = lock_env();
        let fx = FileUpdateFixture::new("e2e", None, true);
        let _env = fx.env_guard(None);
        let scheduler = UpdateScheduler::new();
        scheduler.check_async("1.0.0", false, None);
        assert!(matches!(wait_until_settled(&scheduler), UpdateState::Available { .. }));
        scheduler.install_async(None);
        assert!(matches!(wait_until_settled(&scheduler), UpdateState::ReadyToRestart { .. }));
        let out = Command::new(&fx.target_exe).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "AkhTerm 1.1.0");
    }

    #[test]
    fn dismissed_version_skipped_on_auto_check_but_shown_on_manual() {
        let _lock = lock_env();
        let fx = FileUpdateFixture::new("dismiss", None, true);
        let _env = fx.env_guard(None);
        let scheduler = UpdateScheduler::new();
        scheduler.check_async("1.0.0", false, None);
        assert!(matches!(wait_until_settled(&scheduler), UpdateState::Available { .. }));
        scheduler.dismiss_current();
        assert_eq!(read_dismissed_version().as_deref(), Some("v1.1.0"));
        scheduler.check_async("1.0.0", false, None);
        assert_eq!(wait_until_settled(&scheduler), UpdateState::Idle);
        scheduler.check_async("1.0.0", true, None);
        assert!(matches!(wait_until_settled(&scheduler), UpdateState::Available { .. }));
    }

    #[test]
    fn tamper_and_missing_sha256sums_fail_closed_without_touching_target() {
        let _lock = lock_env();
        let fx = FileUpdateFixture::new("tamper", Some("0".repeat(64)), true);
        let _env = fx.env_guard(None);
        let scheduler = UpdateScheduler::new();
        scheduler.check_async("1.0.0", true, None);
        assert!(matches!(wait_until_settled(&scheduler), UpdateState::Available { .. }));
        scheduler.install_async(None);
        assert!(matches!(wait_until_settled(&scheduler), UpdateState::Failed { .. }));
        let ver_out = Command::new(&fx.target_exe).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&ver_out.stdout).trim(), "AkhTerm 1.0.0");
    }

    #[test]
    fn missing_sha256sums_asset_is_rejected_before_install() {
        let _lock = lock_env();
        let fx = FileUpdateFixture::new("nosums", None, false);
        let _env = fx.env_guard(None);
        let scheduler = UpdateScheduler::new();
        scheduler.check_async("1.0.0", true, None);
        assert!(matches!(wait_until_settled(&scheduler), UpdateState::Available { .. }));
        scheduler.install_async(None);
        assert!(matches!(
            wait_until_settled(&scheduler),
            UpdateState::Failed { ref error, .. } if error.contains("SHA256SUMS")
        ));
    }

    #[test]
    fn disable_env_skips_auto_check_but_allows_manual_and_restart_queue() {
        let _lock = lock_env();
        let fx = FileUpdateFixture::new("disable", None, true);
        let _env = fx.env_guard(Some("1"));
        let scheduler = UpdateScheduler::new();
        scheduler.check_async("1.0.0", false, None);
        assert_eq!(scheduler.state(), UpdateState::Idle);
        scheduler.check_async("1.0.0", true, None);
        assert!(matches!(wait_until_settled(&scheduler), UpdateState::Available { .. }));
        request_restart_exe(PathBuf::from("/tmp/akhterm-next"));
        assert_eq!(take_pending_restart_exe(), Some(PathBuf::from("/tmp/akhterm-next")));
        assert_eq!(take_pending_restart_exe(), None);
        assert_eq!(
            exec_replace_process(Path::new("/bad\0exe")).kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn privileged_command_builder_and_mock_runner_cover_deb_and_rpm() {
        let captured: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let cap_clone = Arc::clone(&captured);
        let scheduler = UpdateScheduler::with_privileged_runner_for_test(Arc::new(move |cmd| {
            cap_clone.lock().unwrap().push(cmd.to_vec());
            Ok(())
        }));
        let deb = build_privileged_install_command(SystemPackageKind::Deb, Path::new("/tmp/a.deb"));
        let rpm = build_privileged_install_command(SystemPackageKind::Rpm, Path::new("/tmp/a.rpm"));
        assert_eq!(deb, ["pkexec", "dpkg", "-i", "/tmp/a.deb"]);
        assert_eq!(rpm, ["pkexec", "rpm", "-U", "/tmp/a.rpm"]);
        scheduler.run_pkexec(&deb).unwrap();
        scheduler.run_pkexec(&rpm).unwrap();
        assert_eq!(captured.lock().unwrap().len(), 2);
    }

    #[test]
    fn build_tarball_script_is_executable_and_passes_bash_syntax_check() {
        let script = Path::new("packaging/build-tarball.sh");
        let mode = fs::metadata(script).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0, "packaging/build-tarball.sh must be executable");
        let status = Command::new("bash").arg("-n").arg(script).status().unwrap();
        assert!(status.success(), "bash -n packaging/build-tarball.sh failed");
    }
}
