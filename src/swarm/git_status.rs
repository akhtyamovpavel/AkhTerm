//! Asynchronous Git dirty-stat inspector, changed-files popover, and clean worktree removal.

use std::collections::HashMap;
use std::ffi::{c_int, c_uint, c_void, CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::ptr;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::connect_click;
use crate::ffi::*;
use crate::git_wt::{detect_git_head, detect_git_repo, detect_git_repo_bounded, GitHeadInfo};
use crate::paths::shorten_home;
use crate::ui_text as text;

pub type ClickHandler = extern "C" fn(*mut GtkWidget, *mut c_void);
pub type KeyPressHandler = extern "C" fn(*mut GtkWidget, *const GdkEvent, *mut c_void) -> c_int;

pub const DEFAULT_GIT_TTL: Duration = Duration::from_secs(15);
pub const MAX_GIT_FILES: usize = 30;
pub const KEYCODE_G: u16 = 42;

pub const DIFF_PANE_SCRIPT: &str = concat!(
    "run_diff() { ",
    "if git -C \"$1\" rev-parse --verify HEAD >/dev/null 2>&1; then ",
    "git -C \"$1\" diff --color=always HEAD -- \"$2\"; ",
    "else ",
    "git -C \"$1\" diff --color=always --cached -- \"$2\"; ",
    "git -C \"$1\" diff --color=always -- \"$2\"; ",
    "fi; }; ",
    "if command -v less >/dev/null 2>&1; then ",
    "run_diff \"$1\" \"$2\" | less -R; ",
    "else ",
    "run_diff \"$1\" \"$2\"; exec \"${3:-sh}\"; ",
    "fi"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitFileKind {
    Modified,
    Added,
    Deleted,
    Untracked,
}

impl GitFileKind {
    pub fn is_untracked(self) -> bool {
        matches!(self, Self::Untracked)
    }

    pub fn tag(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Untracked => "?",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitChangedFile {
    pub path: String,
    pub kind: GitFileKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitRepoStatus {
    pub root: String,
    pub head: String,
    pub is_linked_worktree: bool,
    pub changed_count: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub files: Vec<GitChangedFile>,
}

impl GitRepoStatus {
    pub fn is_dirty(&self) -> bool {
        self.changed_count > 0 || self.insertions > 0 || self.deletions > 0
    }

    pub fn dirty_badge(&self) -> String {
        if !self.is_dirty() {
            String::new()
        } else if self.insertions > 0 || self.deletions > 0 {
            format!("+{} -{}", self.insertions, self.deletions)
        } else {
            format!("±{}", self.changed_count)
        }
    }

    pub fn summary_line(&self) -> String {
        if !self.is_dirty() {
            return "Рабочее дерево чистое".to_owned();
        }
        if self.insertions > 0 || self.deletions > 0 {
            format!(
                "Изменено файлов: {}  ·  +{} −{}",
                self.changed_count, self.insertions, self.deletions
            )
        } else {
            format!("Изменённых / новых файлов: ±{}", self.changed_count)
        }
    }
}

pub fn format_git_button_label(status: Option<&GitRepoStatus>) -> Option<String> {
    let badge = status?.dirty_badge();
    (!badge.is_empty()).then(|| format!("Git {badge} ▾"))
}

pub fn is_git_popover_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    let ctrl_shift = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
    if (state & ctrl_shift) != ctrl_shift || (state & GDK_MOD1_MASK) != 0 {
        return false;
    }
    matches!(
        keyval,
        0x47 | 0x67 | 0x6d0 | 0x6f0 | 0x041f | 0x043f | 0x0100_041f | 0x0100_043f
    ) || keycode == KEYCODE_G
}

pub fn build_diff_pane_argv<'a>(
    repo_root: &'a str,
    rel_path: &'a str,
    shell: &'a str,
) -> [&'a str; 7] {
    [
        "sh",
        "-c",
        DIFF_PANE_SCRIPT,
        "akhterm-git-diff",
        repo_root,
        rel_path,
        shell,
    ]
}

fn classify_xy(x: char, y: char) -> Option<GitFileKind> {
    match (x, y) {
        ('?', '?') => Some(GitFileKind::Untracked),
        ('!', '!') | (' ', ' ') => None,
        (_, 'D') | ('D', ' ') => Some(GitFileKind::Deleted),
        ('A', ' ') => Some(GitFileKind::Added),
        _ => Some(GitFileKind::Modified),
    }
}

fn push_escaped_byte(chars: &mut std::str::Chars<'_>, first: char, out: &mut Vec<u8>) {
    match first {
        '\\' => out.push(b'\\'),
        '"' => out.push(b'"'),
        't' => out.push(b'\t'),
        'n' => out.push(b'\n'),
        'r' => out.push(b'\r'),
        'a' => out.push(0x07),
        'b' => out.push(0x08),
        'f' => out.push(0x0c),
        'v' => out.push(0x0b),
        '0'..='7' => push_octal_escape(chars, first, out),
        other => push_utf8_char(other, out),
    }
}

fn push_octal_escape(chars: &mut std::str::Chars<'_>, first: char, out: &mut Vec<u8>) {
    let mut val = (first as u8) - b'0';
    for _ in 0..2 {
        let mut peek = chars.clone();
        if let Some(d @ '0'..='7') = peek.next() {
            val = val.wrapping_mul(8).wrapping_add((d as u8) - b'0');
            let _ = chars.next();
        } else {
            break;
        }
    }
    out.push(val);
}

fn push_utf8_char(ch: char, out: &mut Vec<u8>) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

fn unquote_c_token(input: &str) -> (String, &str) {
    let Some(rest) = input.strip_prefix('"') else {
        return (input.to_owned(), "");
    };
    let mut bytes = Vec::with_capacity(rest.len());
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            return (String::from_utf8_lossy(&bytes).into_owned(), chars.as_str());
        }
        if ch == '\\' {
            if let Some(esc) = chars.next() {
                push_escaped_byte(&mut chars, esc, &mut bytes);
            }
        } else {
            push_utf8_char(ch, &mut bytes);
        }
    }
    (String::from_utf8_lossy(&bytes).into_owned(), "")
}

fn extract_porcelain_path(x: char, y: char, raw_tail: &str) -> String {
    let is_rename = matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C');
    if !is_rename {
        return unquote_c_token(raw_tail).0;
    }
    if raw_tail.starts_with('"') {
        let (_, after_orig) = unquote_c_token(raw_tail);
        let dest = after_orig.strip_prefix(" -> ").unwrap_or(raw_tail);
        return unquote_c_token(dest).0;
    }
    let dest = raw_tail.split_once(" -> ").map_or(raw_tail, |(_, right)| right);
    unquote_c_token(dest).0
}

fn parse_porcelain_line(raw_line: &str) -> Option<GitChangedFile> {
    let line = raw_line.trim_end_matches('\r');
    let mut chars = line.chars();
    let (x, y, sep) = (chars.next()?, chars.next()?, chars.next()?);
    if sep != ' ' {
        return None;
    }
    let kind = classify_xy(x, y)?;
    let path = extract_porcelain_path(x, y, chars.as_str());
    (!path.is_empty()).then_some(GitChangedFile { path, kind })
}

pub fn parse_porcelain_status(raw: &str) -> (usize, Vec<GitChangedFile>) {
    let mut count = 0usize;
    let mut files = Vec::new();
    for line in raw.lines() {
        if let Some(item) = parse_porcelain_line(line) {
            count += 1;
            if files.len() < MAX_GIT_FILES {
                files.push(item);
            }
        }
    }
    (count, files)
}

fn leading_usize(part: &str) -> Option<usize> {
    part.split_whitespace().next()?.parse::<usize>().ok()
}

pub fn parse_shortstat(raw: &str) -> (usize, usize) {
    let mut ins = 0usize;
    let mut del = 0usize;
    for part in raw.lines().flat_map(|line| line.split(',')) {
        let trimmed = part.trim();
        if trimmed.contains("(+)") || trimmed.contains("insertion") {
            ins += leading_usize(trimmed).unwrap_or(0);
        } else if trimmed.contains("(-)") || trimmed.contains("deletion") {
            del += leading_usize(trimmed).unwrap_or(0);
        }
    }
    (ins, del)
}

fn expand_with_home(path: &str, home: &str) -> PathBuf {
    if path == "~" {
        return PathBuf::from(home);
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return Path::new(home).join(rest);
    }
    PathBuf::from(path)
}

fn clean_home(home: &str) -> PathBuf {
    let trimmed = home.trim();
    if trimmed.is_empty() {
        return PathBuf::new();
    }
    fs::canonicalize(trimmed).unwrap_or_else(|_| {
        let stripped = trimmed.trim_end_matches('/');
        PathBuf::from(if stripped.is_empty() { "/" } else { stripped })
    })
}

fn is_excluded_root(dir: &Path, home: &Path) -> bool {
    dir.as_os_str().is_empty()
        || dir == Path::new("/")
        || (!home.as_os_str().is_empty() && home.starts_with(dir))
}

pub fn resolve_git_repo(cwd: &str, home: &str) -> Option<(String, GitHeadInfo)> {
    let trimmed = cwd.trim();
    if trimmed.is_empty() {
        return None;
    }
    let home_path = clean_home(home);
    let start = fs::canonicalize(expand_with_home(trimmed, home))
        .ok()
        .filter(|dir| dir.is_dir())?;
    let (root, info) = detect_git_repo_bounded(&start, &home_path)?;
    (!is_excluded_root(&root, &home_path)).then(|| (root.to_string_lossy().into_owned(), info))
}

pub fn lookup_git_status<'a>(
    map: &'a HashMap<String, GitRepoStatus>,
    cwd: &str,
    home: &str,
) -> Option<&'a GitRepoStatus> {
    let (root, _) = resolve_git_repo(cwd, home)?;
    map.get(&root)
}

fn run_git_cmd(cwd: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("git")
        .env("LC_MESSAGES", "C")
        .arg("-c")
        .arg("core.quotepath=false")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Не удалось запустить git: {e}"))
}

fn shortstat_for_args(root: &Path, args: &[&str]) -> Option<(usize, usize)> {
    let out = run_git_cmd(root, args).ok().filter(|o| o.status.success())?;
    Some(parse_shortstat(&String::from_utf8_lossy(&out.stdout)))
}

fn collect_shortstat_counts(root: &Path) -> (usize, usize) {
    if let Some(counts) = shortstat_for_args(root, &["diff", "HEAD", "--shortstat"]) {
        return counts;
    }
    let unstaged = shortstat_for_args(root, &["diff", "--shortstat"]).unwrap_or((0, 0));
    let staged = shortstat_for_args(root, &["diff", "--cached", "--shortstat"]).unwrap_or((0, 0));
    (unstaged.0 + staged.0, unstaged.1 + staged.1)
}

fn collect_porcelain_files(root: &Path) -> (usize, Vec<GitChangedFile>) {
    let Ok(out) = run_git_cmd(root, &["status", "--porcelain=v1", "-uall"]) else {
        return (0, Vec::new());
    };
    if !out.status.success() {
        return (0, Vec::new());
    }
    parse_porcelain_status(&String::from_utf8_lossy(&out.stdout))
}

pub fn collect_git_status(repo_root: &str) -> GitRepoStatus {
    let root_path = Path::new(repo_root);
    let (head, is_linked_worktree) = detect_git_head(root_path)
        .map_or_else(|| ("HEAD".to_owned(), false), |info| (info.head, info.is_linked_worktree));
    let (changed_count, files) = collect_porcelain_files(root_path);
    let (insertions, deletions) = collect_shortstat_counts(root_path);
    GitRepoStatus {
        root: repo_root.to_owned(),
        head,
        is_linked_worktree,
        changed_count,
        insertions,
        deletions,
        files,
    }
}

struct GitSchedulerState {
    entries: HashMap<String, (Instant, GitRepoStatus)>,
    targets: Vec<String>,
    force_due: Vec<String>,
    ttl: Duration,
}

impl GitSchedulerState {
    fn is_due(&self, root: &str, now: Instant) -> bool {
        if self.force_due.iter().any(|r| r == root) {
            return true;
        }
        match self.entries.get(root) {
            Some((ts, _)) => now.saturating_duration_since(*ts) >= self.ttl,
            None => true,
        }
    }

    fn due_roots(&self, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        for root in self.targets.iter().chain(self.force_due.iter()) {
            if self.is_due(root, now) && !out.contains(root) {
                out.push(root.clone());
            }
        }
        out
    }
}

#[derive(Clone)]
pub struct GitStatusScheduler {
    shared: Arc<(Mutex<GitSchedulerState>, Condvar)>,
}

impl Default for GitStatusScheduler {
    fn default() -> Self {
        Self::new(DEFAULT_GIT_TTL)
    }
}

impl GitStatusScheduler {
    pub fn new(ttl: Duration) -> Self {
        let state = GitSchedulerState {
            entries: HashMap::new(),
            targets: Vec::new(),
            force_due: Vec::new(),
            ttl,
        };
        Self {
            shared: Arc::new((Mutex::new(state), Condvar::new())),
        }
    }

    pub fn sync_targets(&self, local_cwds: &[String], home: &str) {
        let mut next = Vec::new();
        for cwd in local_cwds {
            if let Some((root, _)) = resolve_git_repo(cwd, home) {
                if !next.contains(&root) {
                    next.push(root);
                }
            }
        }
        let (lock, cvar) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            state.targets = next;
            if !state.due_roots(Instant::now()).is_empty() {
                cvar.notify_one();
            }
        }
    }

    pub fn request_refresh(&self, repo_root: &str) {
        if repo_root.is_empty() {
            return;
        }
        let (lock, cvar) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            if !state.force_due.iter().any(|r| r == repo_root) {
                state.force_due.push(repo_root.to_owned());
            }
            cvar.notify_one();
        }
    }

    pub fn remove_root(&self, repo_root: &str) {
        let (lock, _) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            state.entries.remove(repo_root);
            state.targets.retain(|r| r != repo_root);
            state.force_due.retain(|r| r != repo_root);
        }
    }

    fn take_due(&self, now: Instant) -> Vec<String> {
        let (lock, _) = &*self.shared;
        let Ok(mut state) = lock.lock() else {
            return Vec::new();
        };
        let due = state.due_roots(now);
        state.force_due.retain(|r| !due.contains(r));
        due
    }

    fn store_batch(&self, now: Instant, batch: Vec<(String, GitRepoStatus)>) {
        let (lock, _) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            for (root, status) in batch {
                state.entries.insert(root, (now, status));
            }
        }
    }

    pub fn refresh_due(&self, now: Instant) -> usize {
        self.refresh_due_with(now, collect_git_status)
    }

    pub fn refresh_due_with<F>(&self, now: Instant, mut inspect: F) -> usize
    where
        F: FnMut(&str) -> GitRepoStatus,
    {
        let due = self.take_due(now);
        if due.is_empty() {
            return 0;
        }
        let batch: Vec<_> = due.into_iter().map(|r| (r.clone(), inspect(&r))).collect();
        let count = batch.len();
        self.store_batch(now, batch);
        count
    }

    pub fn snapshot(&self) -> HashMap<String, GitRepoStatus> {
        let (lock, _) = &*self.shared;
        let Ok(state) = lock.lock() else {
            return HashMap::new();
        };
        state
            .entries
            .iter()
            .map(|(k, (_, st))| (k.clone(), st.clone()))
            .collect()
    }

    pub fn wait_for_turn(&self, max_wait: Duration) {
        let (lock, cvar) = &*self.shared;
        if let Ok(state) = lock.lock() {
            if state.due_roots(Instant::now()).is_empty() {
                let _ = cvar.wait_timeout(state, max_wait);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeRemovalOutcome {
    pub main_repo: PathBuf,
    pub wt_root: PathBuf,
    pub branch: String,
    pub branch_deleted: bool,
}

impl WorktreeRemovalOutcome {
    pub fn success_message(&self) -> String {
        let shown = shorten_home(&self.wt_root.to_string_lossy());
        if self.branch_deleted {
            format!("⎇ Удалён чистый worktree {shown} и ветка {}", self.branch)
        } else if self.branch.starts_with("wt/") {
            format!("⎇ Удалён чистый worktree {shown} (ветка {} сохранена)", self.branch)
        } else {
            format!("⎇ Удалён чистый worktree {shown}")
        }
    }
}

pub fn parse_main_worktree_porcelain(raw: &str) -> Option<PathBuf> {
    let first = raw
        .lines()
        .find_map(|line| line.strip_prefix("worktree "))?
        .trim_end_matches('\r');
    (!first.is_empty()).then(|| PathBuf::from(first))
}

fn verify_clean_worktree_tree(wt_root: &Path) -> Result<(), String> {
    let out = run_git_cmd(wt_root, &["status", "--porcelain=v1", "-uall"])?;
    if !out.status.success() {
        return Err("⚠ Не удалось проверить статус Git worktree".to_owned());
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let (dirty_count, _) = parse_porcelain_status(&raw);
    if dirty_count > 0 || !raw.trim().is_empty() {
        let n = dirty_count.max(1);
        return Err(format!(
            "⚠ Нельзя удалить нечистый worktree: есть незакоммиченные изменения ({n} файл.)"
        ));
    }
    Ok(())
}

fn resolve_main_worktree_root(wt_root: &Path) -> Result<PathBuf, String> {
    let out = run_git_cmd(wt_root, &["worktree", "list", "--porcelain"])?;
    if !out.status.success() {
        return Err("⚠ Не удалось получить список Git worktree".to_owned());
    }
    let main_raw = parse_main_worktree_porcelain(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "⚠ Не найден основной репозиторий для worktree".to_owned())?;
    let main_canon = fs::canonicalize(&main_raw).unwrap_or(main_raw);
    if main_canon == wt_root {
        return Err("⚠ Нельзя удалить основной каталог репозитория".to_owned());
    }
    Ok(main_canon)
}

fn exec_git_worktree_remove(main_repo: &Path, wt_root: &Path) -> Result<(), String> {
    let wt_str = wt_root.to_str().ok_or_else(|| "⚠ Некорректный путь worktree".to_owned())?;
    let out = run_git_cmd(main_repo, &["worktree", "remove", "--", wt_str])?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail = stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("ошибка git");
    Err(format!("⚠ Не удалось удалить worktree: {detail}"))
}

fn delete_merged_wt_branch(main_repo: &Path, branch: &str) -> bool {
    branch.starts_with("wt/")
        && run_git_cmd(main_repo, &["branch", "-d", "--", branch]).is_ok_and(|o| o.status.success())
}

pub fn remove_clean_worktree(wt_dir: &Path) -> Result<WorktreeRemovalOutcome, String> {
    let canon = fs::canonicalize(wt_dir).map_err(|_| "⚠ Каталог вкладки не найден".to_owned())?;
    let (wt_root, head_info) = detect_git_repo(&canon)
        .ok_or_else(|| "⚠ Каталог вкладки не является Git-репозиторием".to_owned())?;
    if !head_info.is_linked_worktree {
        return Err("⚠ Удаление доступно только для дополнительного Git worktree".to_owned());
    }
    verify_clean_worktree_tree(&wt_root)?;
    let main_repo = resolve_main_worktree_root(&wt_root)?;
    exec_git_worktree_remove(&main_repo, &wt_root)?;
    let branch_deleted = delete_merged_wt_branch(&main_repo, &head_info.head);
    Ok(WorktreeRemovalOutcome { main_repo, wt_root, branch: head_info.head, branch_deleted })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitFileEntry {
    pub repo_root: String,
    pub path: String,
    pub kind: GitFileKind,
}

#[derive(Clone, Copy)]
pub struct GitMenuCallbacks {
    pub on_file: ClickHandler,
    pub on_remove_wt: ClickHandler,
}

#[derive(Clone, Copy)]
pub struct GitMenuWidgets {
    pub popover: *mut GtkWidget,
    pub outer_box: *mut GtkWidget,
    pub old_inner: *mut GtkWidget,
}

pub struct GitMenu {
    button: *mut GtkWidget,
    popover: *mut GtkWidget,
    outer_box: *mut GtkWidget,
    inner_box: *mut GtkWidget,
    entries: Vec<GitFileEntry>,
}

pub fn update_git_button(
    button: *mut GtkWidget,
    is_local: bool,
    status: Option<&GitRepoStatus>,
) {
    if button.is_null() {
        return;
    }
    unsafe { gtk_widget_set_sensitive(button, i32::from(is_local)) };
    let dirty_lbl = if is_local { format_git_button_label(status) } else { None };
    set_class(button, "btn-mi-b", dirty_lbl.is_some());
    match dirty_lbl.and_then(|s| CString::new(s).ok()) {
        Some(c) => unsafe { gtk_button_set_label(button, c.as_ptr()) },
        None => unsafe { gtk_button_set_label(button, text::GIT_MENU.as_ptr()) },
    }
}

fn make_section_lbl(title: &CStr) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(title.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-header");
        lbl
    }
}

fn make_muted_lbl(content: &str) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "ssh-count");
        set_label_text(lbl, content);
        lbl
    }
}

fn format_file_tooltip(file: &GitChangedFile) -> String {
    if file.kind.is_untracked() {
        format!("{} — неотслеживаемый файл (открыть)", file.path)
    } else {
        format!("{} — открыть git diff в вертикальном сплите", file.path)
    }
}

fn make_file_row_btn(file: &GitChangedFile, idx: usize, cb: ClickHandler) -> *mut GtkWidget {
    let row_txt = format!("[{}]  {}", file.kind.tag(), file.path);
    let tip = format_file_tooltip(file);
    unsafe {
        let btn = gtk_button_new();
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        gtk_label_set_ellipsize(lbl, PANGO_ELLIPSIZE_MIDDLE);
        gtk_label_set_max_width_chars(lbl, 56);
        set_label_text(lbl, &row_txt);
        gtk_container_add(btn, lbl);
        set_tooltip(btn, &tip);
        connect_click(btn, cb, idx as *mut c_void);
        btn
    }
}

fn wrap_files_scroll(list: *mut GtkWidget, count: usize) -> *mut GtkWidget {
    if count <= 10 {
        return list;
    }
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, 420);
        gtk_scrolled_window_set_min_content_height(scroll, 280);
        gtk_scrolled_window_set_max_content_height(scroll, 360);
        gtk_container_add(scroll, list);
        scroll
    }
}

fn pack_repo_header(
    vbox: *mut GtkWidget,
    root: &str,
    head: &GitHeadInfo,
    status: Option<&GitRepoStatus>,
    review: &crate::git_review::BranchReview,
) {
    let title = format!("{}  ·  {}", head.badge(), shorten_home(root));
    let base_sum = status.map_or_else(
        || text::GIT_LOADING.to_string_lossy().into_owned(),
        GitRepoStatus::summary_line,
    );
    let summary = crate::git_review::format_summary_with_branch(&base_sum, review);
    unsafe {
        let title_lbl = make_section_lbl(c"");
        set_label_text(title_lbl, &title);
        gtk_box_pack_start(vbox, title_lbl, 0, 1, 0);
        gtk_box_pack_start(vbox, make_muted_lbl(&summary), 0, 1, 0);
    }
    if head.is_linked_worktree {
        let _ = (text::GIT_REMOVE_WT_BTN, text::GIT_REMOVE_WT_TIP);
        crate::git_review::pack_worktree_actions(vbox);
    }
}

fn pack_repo_sections(
    popover: *mut GtkWidget,
    vbox: *mut GtkWidget,
    root: &str,
    head: &GitHeadInfo,
    status: Option<&GitRepoStatus>,
    on_file: ClickHandler,
) -> Vec<GitFileEntry> {
    let review = crate::git_review::collect_branch_review(Path::new(root), head);
    crate::git_review::reset_review_ui_state(popover, root, &review);
    pack_repo_header(vbox, root, head, status, &review);
    let entries = pack_repo_files(vbox, root, status, on_file);
    crate::git_review::pack_branch_review(vbox, &review);
    entries
}

fn build_file_buttons(
    root: &str,
    files: &[GitChangedFile],
    on_file: ClickHandler,
) -> (*mut GtkWidget, Vec<GitFileEntry>) {
    let mut entries = Vec::with_capacity(files.len());
    let list_box = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 3) };
    for (idx, file) in files.iter().enumerate() {
        let btn = make_file_row_btn(file, idx, on_file);
        unsafe { gtk_box_pack_start(list_box, btn, 0, 1, 0) };
        entries.push(GitFileEntry {
            repo_root: root.to_owned(),
            path: file.path.clone(),
            kind: file.kind,
        });
    }
    (wrap_files_scroll(list_box, entries.len()), entries)
}

fn pack_repo_files(
    vbox: *mut GtkWidget,
    root: &str,
    status: Option<&GitRepoStatus>,
    on_file: ClickHandler,
) -> Vec<GitFileEntry> {
    unsafe { gtk_box_pack_start(vbox, make_section_lbl(text::GIT_FILES_HEADER), 0, 0, 0) };
    let Some(st) = status else {
        unsafe { gtk_box_pack_start(vbox, make_muted_lbl(&text::GIT_LOADING.to_string_lossy()), 0, 1, 2) };
        return Vec::new();
    };
    if st.files.is_empty() {
        unsafe { gtk_box_pack_start(vbox, make_muted_lbl(&text::GIT_CLEAN.to_string_lossy()), 0, 1, 2) };
        return Vec::new();
    }
    let (wrapped, entries) = build_file_buttons(root, &st.files, on_file);
    unsafe { gtk_box_pack_start(vbox, wrapped, 1, 1, 0) };
    if st.changed_count > entries.len() {
        let extra = format!("…и ещё {} файл.", st.changed_count - entries.len());
        unsafe { gtk_box_pack_start(vbox, make_muted_lbl(&extra), 0, 1, 0) };
    }
    entries
}

fn pack_non_repo_notice(vbox: *mut GtkWidget) {
    let msg = text::GIT_NOT_REPO.to_string_lossy();
    unsafe {
        gtk_box_pack_start(vbox, make_section_lbl(text::GIT_HEADER), 0, 0, 0);
        gtk_box_pack_start(vbox, make_muted_lbl(&msg), 0, 1, 2);
    }
}

pub fn rebuild_git_widgets(
    widgets: GitMenuWidgets,
    repo: Option<(&str, &GitHeadInfo)>,
    status: Option<&GitRepoStatus>,
    cbs: GitMenuCallbacks,
) -> (*mut GtkWidget, Vec<GitFileEntry>) {
    let _ = cbs.on_remove_wt;
    unsafe {
        if !widgets.old_inner.is_null() {
            gtk_widget_destroy(widgets.old_inner);
        }
        let vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
        let entries = match repo {
            Some((root, head)) => {
                pack_repo_sections(widgets.popover, vbox, root, head, status, cbs.on_file)
            }
            None => {
                pack_non_repo_notice(vbox);
                Vec::new()
            }
        };
        gtk_box_pack_start(widgets.outer_box, vbox, 1, 1, 0);
        gtk_widget_show_all(widgets.outer_box);
        (vbox, entries)
    }
}

fn wire_git_menu_signals(
    button: *mut GtkWidget,
    popover: *mut GtkWidget,
    on_open: ClickHandler,
    on_key: KeyPressHandler,
) {
    connect_click(button, on_open, ptr::null_mut());
    unsafe {
        g_signal_connect_data(
            popover,
            c"key-press-event".as_ptr(),
            on_key as *const c_void,
            ptr::null_mut(),
            ptr::null(),
            0,
        );
    }
}

impl GitMenu {
    pub fn new(button: *mut GtkWidget, on_open: ClickHandler, on_key: KeyPressHandler) -> Self {
        let (popover, outer_box) = unsafe {
            let pop = gtk_popover_new(button);
            let outer = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
            add_class(outer, "popover-box");
            gtk_container_add(pop, outer);
            (pop, outer)
        };
        wire_git_menu_signals(button, popover, on_open, on_key);
        Self {
            button,
            popover,
            outer_box,
            inner_box: ptr::null_mut(),
            entries: Vec::new(),
        }
    }

    pub fn button(&self) -> *mut GtkWidget {
        self.button
    }

    pub fn popover(&self) -> *mut GtkWidget {
        self.popover
    }

    pub fn take_widgets(&mut self) -> GitMenuWidgets {
        GitMenuWidgets {
            popover: self.popover,
            outer_box: self.outer_box,
            old_inner: std::mem::replace(&mut self.inner_box, ptr::null_mut()),
        }
    }

    pub fn save_snapshot(&mut self, inner_box: *mut GtkWidget, entries: Vec<GitFileEntry>) {
        self.inner_box = inner_box;
        self.entries = entries;
    }

    pub fn entry_at(&self, index: usize) -> Option<GitFileEntry> {
        self.entries.get(index).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_wt::create_agent_worktree;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

    struct TempDirGuard(PathBuf);

    impl TempDirGuard {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "akhterm_gitstat_{tag}_{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git_ok(cwd: &Path, args: &[&str]) {
        let out = run_git_cmd(cwd, args).unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo_with_commit(repo: &Path) {
        fs::create_dir_all(repo).unwrap();
        git_ok(repo, &["init", "-b", "main"]);
        fs::write(repo.join("README.md"), "line 1\nline 2\n").unwrap();
        git_ok(repo, &["add", "."]);
        git_ok(repo, &["-c", "commit.gpgsign=false", "-c", "user.name=T", "-c", "user.email=t@e", "commit", "-m", "i"]);
    }

    extern "C" fn noop_click(_w: *mut GtkWidget, _d: *mut c_void) {}
    extern "C" fn noop_key(_w: *mut GtkWidget, _e: *const GdkEvent, _d: *mut c_void) -> c_int {
        0
    }

    #[test]
    fn parses_porcelain_v1_with_cyrillic_octal_escapes_and_renames() {
        let sample = concat!(
            " M src/main.rs\n",
            "A  src/git_status.rs\n",
            " D old/removed.txt\n",
            "?? учёба/модуль 1.py\n",
            "?? \"\\321\\203\\321\\207\\321\\221\\320\\261\\320\\260/\\321\\202\\320\\265\\321\\201\\321\\202.rs\"\n",
            "R  \"старый -> файл.md\" -> \"новый -> файл.md\"\n",
            "!! target/debug/akhterm\n",
        );
        let (count, files) = parse_porcelain_status(sample);
        assert_eq!(count, 6);
        assert_eq!(files.len(), 6);
        assert_eq!(files[0], GitChangedFile { path: "src/main.rs".into(), kind: GitFileKind::Modified });
        assert_eq!(files[1], GitChangedFile { path: "src/git_status.rs".into(), kind: GitFileKind::Added });
        assert_eq!(files[2], GitChangedFile { path: "old/removed.txt".into(), kind: GitFileKind::Deleted });
        assert_eq!(files[3], GitChangedFile { path: "учёба/модуль 1.py".into(), kind: GitFileKind::Untracked });
        assert_eq!(files[4], GitChangedFile { path: "учёба/тест.rs".into(), kind: GitFileKind::Untracked });
        assert_eq!(files[5], GitChangedFile { path: "новый -> файл.md".into(), kind: GitFileKind::Modified });
    }

    #[test]
    fn parses_shortstat_english_and_localized_counts() {
        assert_eq!(parse_shortstat(""), (0, 0));
        assert_eq!(parse_shortstat(" 1 file changed, 14 insertions(+)\n"), (14, 0));
        assert_eq!(parse_shortstat(" 2 files changed, 7 deletions(-)\n"), (0, 7));
        assert_eq!(parse_shortstat(" 4 files changed, 42 insertions(+), 9 deletions(-)\n"), (42, 9));
        assert_eq!(parse_shortstat(" 2 файла изменено, 11 добавлений(+), 3 удаления(-)\n"), (11, 3));
    }

    #[test]
    fn formats_compact_dirty_badges_and_topbar_button_label() {
        let mut st = GitRepoStatus::default();
        assert_eq!(st.dirty_badge(), "");
        assert_eq!(format_git_button_label(Some(&st)), None);

        st.changed_count = 3;
        assert_eq!(st.dirty_badge(), "±3");
        assert_eq!(format_git_button_label(Some(&st)).as_deref(), Some("Git ±3 ▾"));

        st.insertions = 18;
        st.deletions = 4;
        assert_eq!(st.dirty_badge(), "+18 -4");
        assert_eq!(format_git_button_label(Some(&st)).as_deref(), Some("Git +18 -4 ▾"));
    }

    #[test]
    fn matches_ctrl_shift_g_in_latin_cyrillic_and_keycode_42() {
        let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
        assert!(is_git_popover_shortcut(u32::from(b'G'), cs, 0));
        assert!(is_git_popover_shortcut(u32::from(b'g'), cs, 0));
        assert!(is_git_popover_shortcut(0x6d0, cs, 0));
        assert!(is_git_popover_shortcut(0x6f0, cs, 0));
        assert!(is_git_popover_shortcut('П' as u32, cs, 0));
        assert!(is_git_popover_shortcut(0, cs, KEYCODE_G));
        assert!(!is_git_popover_shortcut(u32::from(b'g'), GDK_CONTROL_MASK, KEYCODE_G));
        assert!(!is_git_popover_shortcut(u32::from(b'G'), cs | GDK_MOD1_MASK, KEYCODE_G));
    }

    #[test]
    fn collects_live_status_and_unborn_head_fallback_in_cyrillic_repo() {
        let tmp = TempDirGuard::new("unborn_cyr");
        let unborn = tmp.0.join("Стенд ЦУ").join("НовыйРепо");
        fs::create_dir_all(&unborn).unwrap();
        git_ok(&unborn, &["init", "-b", "main"]);
        fs::write(unborn.join("привет мир.rs"), "fn main() {}\nlet x = 1;\n").unwrap();
        git_ok(&unborn, &["add", "привет мир.rs"]);
        fs::write(unborn.join("черновик.md"), "todo\n").unwrap();

        let st_unborn = collect_git_status(&unborn.to_string_lossy());
        assert_eq!(st_unborn.head, "main");
        assert_eq!(st_unborn.changed_count, 2);
        assert_eq!(st_unborn.insertions, 2);
        assert_eq!(st_unborn.dirty_badge(), "+2 -0");
        assert!(st_unborn.files.iter().any(|f| f.path == "привет мир.rs" && !f.kind.is_untracked()));
        assert!(st_unborn.files.iter().any(|f| f.path == "черновик.md" && f.kind.is_untracked()));
    }

    fn make_fake_repo_subdir(base: &Path) -> (String, String) {
        let home = base.join("home");
        let repo = home.join("проекты").join("akhterm");
        let sub = repo.join("src");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&sub).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        (home.to_string_lossy().into_owned(), sub.to_string_lossy().into_owned())
    }

    #[test]
    fn scheduler_respects_15s_ttl_and_force_refresh() {
        let tmp = TempDirGuard::new("sched");
        let (home_s, sub_s) = make_fake_repo_subdir(&tmp.0);
        let sched = GitStatusScheduler::default();
        let t0 = Instant::now();

        sched.sync_targets(std::slice::from_ref(&sub_s), &home_s);
        let first = sched.refresh_due_with(t0, |r| GitRepoStatus {
            root: r.to_owned(),
            changed_count: 2,
            ..Default::default()
        });
        assert_eq!(first, 1);
        assert_eq!(sched.refresh_due_with(t0 + Duration::from_secs(10), |_| GitRepoStatus::default()), 0);
        assert_eq!(lookup_git_status(&sched.snapshot(), &sub_s, &home_s).map(|s| s.changed_count), Some(2));

        let expired = sched.refresh_due_with(t0 + Duration::from_secs(16), |r| GitRepoStatus {
            root: r.to_owned(),
            insertions: 9,
            deletions: 1,
            changed_count: 1,
            ..Default::default()
        });
        assert_eq!(expired, 1);
        let badge = lookup_git_status(&sched.snapshot(), &sub_s, &home_s).map(GitRepoStatus::dirty_badge);
        assert_eq!(badge.as_deref(), Some("+9 -1"));
    }

    #[test]
    fn refuses_dirty_worktree_removal_and_removes_clean_worktree_and_branch() {
        let tmp = TempDirGuard::new("wt_guard");
        let repo = tmp.0.join("Проект ЦУ").join("Ядро");
        init_repo_with_commit(&repo);
        assert!(remove_clean_worktree(&repo).is_err());

        let wt = create_agent_worktree(&repo, "claude").unwrap();
        let dirty_file = wt.join("незакоммиченный файл.txt");
        fs::write(&dirty_file, "dirty\n").unwrap();
        let err = remove_clean_worktree(&wt).unwrap_err();
        assert!(err.contains("Нельзя удалить"), "unexpected err: {err}");
        assert!(wt.exists(), "dirty worktree must remain on disk");

        fs::remove_file(&dirty_file).unwrap();
        let done = remove_clean_worktree(&wt).unwrap();
        assert_eq!(done.main_repo, fs::canonicalize(&repo).unwrap());
        assert!(!wt.exists(), "clean worktree directory must be removed");
        assert!(done.branch.starts_with("wt/claude-"));
        assert!(done.branch_deleted, "clean wt/* branch should be deleted");
        assert!(done.success_message().contains("Удалён чистый worktree"));
    }

    fn create_git_menu_fixture() -> (*mut GtkWidget, GitMenu) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            let topbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
            let btn = gtk_button_new_with_label(text::GIT_MENU.as_ptr());
            gtk_box_pack_start(topbar, btn, 0, 0, 0);
            gtk_container_add(win, topbar);
            let menu = GitMenu::new(btn, noop_click, noop_key);
            gtk_widget_show_all(win);
            pump_gtk_events();
            (win, menu)
        }
    }

    fn sample_worktree_status() -> (GitHeadInfo, GitRepoStatus) {
        let head = GitHeadInfo { head: "wt/agy-01".into(), is_linked_worktree: true };
        let status = GitRepoStatus {
            root: "/tmp/тест-wt".into(),
            head: "wt/agy-01".into(),
            is_linked_worktree: true,
            changed_count: 2,
            insertions: 12,
            deletions: 3,
            files: vec![
                GitChangedFile { path: "src/main.rs".into(), kind: GitFileKind::Modified },
                GitChangedFile { path: "заметка.md".into(), kind: GitFileKind::Untracked },
            ],
        };
        (head, status)
    }

    #[test]
    fn builds_git_popover_widgets_and_updates_button_in_headless_gtk() {
        with_headless_gtk(|| {
            let (win, mut menu) = create_git_menu_fixture();
            let (head, status) = sample_worktree_status();
            update_git_button(menu.button(), true, Some(&status));
            let cbs = GitMenuCallbacks { on_file: noop_click, on_remove_wt: noop_click };
            let (inner, entries) = rebuild_git_widgets(
                menu.take_widgets(),
                Some(("/tmp/тест-wt", &head)),
                Some(&status),
                cbs,
            );
            menu.save_snapshot(inner, entries);
            pump_gtk_events();
            assert_eq!(menu.entry_at(0).unwrap().path, "src/main.rs");
            assert!(menu.entry_at(1).unwrap().kind.is_untracked());
            update_git_button(menu.button(), false, Some(&status));
            unsafe { gtk_widget_destroy(win) };
            pump_gtk_events();
        });
    }

    #[test]
    fn diff_pane_argv_passes_cyrillic_and_dash_paths_without_shell_injection() {
        let tmp = TempDirGuard::new("diff_argv");
        let repo = tmp.0.join("Репо ЦУ");
        init_repo_with_commit(&repo);
        let tricky = "-опасный файл; echo PWNED.txt";
        fs::write(repo.join(tricky), "новая строка\n").unwrap();
        git_ok(&repo, &["add", "--", tricky]);

        let root_s = repo.to_string_lossy().into_owned();
        let argv = build_diff_pane_argv(&root_s, tricky, "/bin/sh");
        let out = Command::new(argv[0]).args(&argv[1..]).stdin(Stdio::null()).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success());
        assert!(stdout.contains("новая строка"), "diff output: {stdout}");
        assert!(!stdout.lines().any(|l| l.trim() == "PWNED"));
    }

    #[test]
    fn git_status_and_touched_modules_meet_maintainability_targets() {
        let (_, _, git_mi) = crate::mi::compute_file_mi(include_str!("git_status.rs"));
        let (_, _, main_mi) = crate::mi::compute_file_mi(include_str!("../main.rs"));
        let (_, _, wt_mi) = crate::mi::compute_file_mi(include_str!("git_wt.rs"));
        let (_, _, side_mi) = crate::mi::compute_file_mi(include_str!("../workspace/sidebar.rs"));
        let (_, _, pal_mi) = crate::mi::compute_file_mi(include_str!("../ui/palette.rs"));
        assert!(git_mi >= 58.0, "git_status.rs MI {git_mi:.2} must be >= 58.0");
        assert!(main_mi >= 54.87, "main.rs MI {main_mi:.2} must be >= 54.87");
        assert!(wt_mi >= 58.0, "git_wt.rs MI {wt_mi:.2} must be >= 58.0");
        assert!(side_mi >= 56.08, "sidebar.rs MI {side_mi:.2} must be >= 56.08");
        assert!(pal_mi >= 58.0, "palette.rs MI {pal_mi:.2} must be >= 58.0");
    }
}
