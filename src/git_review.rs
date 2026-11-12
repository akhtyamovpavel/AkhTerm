//! Two-layer Git worktree review: branch commits/files vs base, FF/merge, PR, and protected cleanup.

use std::cell::RefCell;
use std::ffi::{c_int, c_void, CStr};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::{ptr, thread};

use crate::app_state::{self, AppState, WT_REMOVE_BUSY};
use crate::connect_click;
use crate::ffi::*;
use crate::git_status::{parse_porcelain_status, remove_clean_worktree, resolve_git_repo};
use crate::git_wt::{detect_git_repo, resolve_primary_worktree_root, GitHeadInfo};
use crate::i18n::UiText;
use crate::paths::{home_dir, shorten_home};
use crate::session::TermSession;
use crate::tabs::Tab;
use crate::{pane_actions, session_ops, split_pane, topbar};

pub const MAX_BRANCH_COMMITS: usize = 12;
pub const MAX_BRANCH_FILES: usize = 30;

pub const BRANCH_DIFF_SCRIPT: &str = "if command -v less >/dev/null 2>&1; then git -C \"$1\" diff --color=always \"$2\" -- \"$3\" | less -R; else git -C \"$1\" diff --color=always \"$2\" -- \"$3\"; exec \"${4:-sh}\"; fi";
pub const OPEN_PR_SCRIPT: &str = "cd \"$1\" && git push -u origin HEAD && gh pr create --fill; exec \"${2:-sh}\"";

pub const HDR_WT_ACTIONS: UiText = UiText::new(c"ДЕЙСТВИЯ WORKTREE", c"WORKTREE ACTIONS");
pub const HDR_BRANCH_COMMITS: UiText = UiText::new(c"КОММИТЫ ВЕТКИ", c"BRANCH COMMITS");
pub const HDR_BRANCH_FILES: UiText = UiText::new(c"ФАЙЛЫ ВЕТКИ", c"BRANCH FILES");
pub const BTN_MERGE_WT: UiText = UiText::new(c"Влить в базу (FF/Merge) и удалить worktree", c"Merge into base & remove worktree");
pub const TIP_MERGE_WT: UiText = UiText::new(c"Влить коммиты ветки в базовую ветку и удалить worktree", c"Merge branch into base and remove this worktree");
pub const BTN_OPEN_PR: UiText = UiText::new(c"Открыть PR (gh)", c"Open PR (gh)");
pub const TIP_OPEN_PR: UiText = UiText::new(c"Отправить ветку в origin и открыть PR через gh в сплите", c"Push branch to origin and create PR via gh in split");
pub const BTN_REMOVE_WT: UiText = UiText::new(c"Удалить worktree", c"Remove worktree");
pub const TIP_REMOVE_WT: UiText = UiText::new(c"Удалить worktree (с подтверждением при наличии правок/коммитов)", c"Remove worktree (confirms if dirty or unmerged)");
pub const BTN_CONFIRM_FORCE_REMOVE: UiText = UiText::new(c"Подтвердить принудительное удаление", c"Confirm Force Remove");
pub const TIP_CONFIRM_FORCE_REMOVE: UiText = UiText::new(c"Нажмите ещё раз для git worktree remove --force и git branch -D", c"Click again for git worktree remove --force and git branch -D");
pub const TIP_BRANCH_DIFF: UiText = UiText::new(c"Открыть git diff <base>...HEAD в вертикальном сплите", c"Open git diff <base>...HEAD in vertical split");
pub const LBL_LOCAL_ONLY: UiText = UiText::new(c"Доступно только для локальной вкладки", c"Available for local tabs only");
pub const LBL_COMMITS_WORD: UiText = UiText::new(c"коммитов в ветке", c"commits ahead of");
pub const LBL_AND_MORE: UiText = UiText::new(c"…и ещё", c"…and more");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchCommit {
    pub sha: String,
    pub subject: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchFileKind {
    Modified,
    Added,
    Deleted,
    Renamed,
}

impl BranchFileKind {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchChangedFile {
    pub path: String,
    pub kind: BranchFileKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchReview {
    pub repo_root: String,
    pub head: String,
    pub is_linked_worktree: bool,
    pub base_branch: Option<String>,
    pub merge_base: Option<String>,
    pub ahead_count: usize,
    pub commits: Vec<BranchCommit>,
    pub changed_count: usize,
    pub files: Vec<BranchChangedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeSafety {
    pub dirty_count: usize,
    pub unmerged_commits: usize,
    pub base_branch: Option<String>,
}

impl WorktreeSafety {
    pub fn needs_force_confirm(&self) -> bool {
        self.dirty_count > 0 || self.unmerged_commits > 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeMergeOutcome {
    pub main_repo: PathBuf,
    pub wt_root: PathBuf,
    pub wt_branch: String,
    pub base_branch: String,
    pub was_fast_forward: bool,
    pub branch_deleted: bool,
}

impl WorktreeMergeOutcome {
    pub fn success_message(&self) -> String {
        let mode = if self.was_fast_forward { "FF" } else { "Merge" };
        let shown = shorten_home(&self.wt_root.to_string_lossy());
        format!("⎇ {mode}: {} -> {} · удалён {shown}", self.wt_branch, self.base_branch)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForceRemovalOutcome {
    pub main_repo: PathBuf,
    pub wt_root: PathBuf,
    pub branch: String,
    pub branch_deleted: bool,
}

impl ForceRemovalOutcome {
    pub fn success_message(&self) -> String {
        let shown = shorten_home(&self.wt_root.to_string_lossy());
        if self.branch_deleted {
            format!("⎇ Принудительно удалён worktree {shown} и ветка {}", self.branch)
        } else {
            format!("⎇ Принудительно удалён worktree {shown}")
        }
    }
}

#[derive(Clone, Debug)]
struct BranchFileEntry {
    repo_root: String,
    base_branch: String,
    path: String,
}

#[derive(Default)]
struct ReviewUiState {
    popover: usize,
    wt_root: String,
    armed_force_remove: bool,
    branch_files: Vec<BranchFileEntry>,
}

thread_local! {
    static REVIEW_UI: RefCell<ReviewUiState> = RefCell::new(ReviewUiState::default());
}

pub fn run_git_c(cwd: &Path, args: &[&str]) -> Result<Output, String> {
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

fn git_stdout_if_ok(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = run_git_c(cwd, args).ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn git_ref_exists(cwd: &Path, full_ref: &str) -> bool {
    run_git_c(cwd, &["show-ref", "--verify", "--quiet", full_ref])
        .is_ok_and(|o| o.status.success())
}

fn resolve_origin_head_branch(repo_root: &Path) -> Option<String> {
    let sym = git_stdout_if_ok(repo_root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])?;
    let local = sym.strip_prefix("origin/").unwrap_or(&sym);
    if git_ref_exists(repo_root, &format!("refs/heads/{local}")) {
        return Some(local.to_owned());
    }
    Some(sym)
}

fn resolve_named_base(repo_root: &Path, name: &str) -> Option<String> {
    if git_ref_exists(repo_root, &format!("refs/heads/{name}")) {
        return Some(name.to_owned());
    }
    let remote = format!("origin/{name}");
    git_ref_exists(repo_root, &format!("refs/remotes/{remote}")).then_some(remote)
}

fn resolve_primary_branch_fallback(repo_root: &Path, current_head: &str) -> Option<String> {
    let main_root = resolve_primary_worktree_root(repo_root)?;
    let branch = git_stdout_if_ok(&main_root, &["symbolic-ref", "--short", "-q", "HEAD"])?;
    (!branch.starts_with("wt/") && branch != current_head).then_some(branch)
}

pub fn detect_base_branch(repo_root: &Path, current_head: &str) -> Option<String> {
    resolve_origin_head_branch(repo_root)
        .or_else(|| resolve_named_base(repo_root, "main"))
        .or_else(|| resolve_named_base(repo_root, "master"))
        .or_else(|| resolve_primary_branch_fallback(repo_root, current_head))
}

fn parse_commit_line(line: &str) -> Option<BranchCommit> {
    let (sha, subject) = line.trim().split_once(' ')?;
    Some(BranchCommit { sha: sha.to_owned(), subject: subject.trim().to_owned() })
}

pub fn parse_branch_commits(raw: &str) -> Vec<BranchCommit> {
    raw.lines().filter_map(parse_commit_line).take(MAX_BRANCH_COMMITS).collect()
}

fn classify_branch_status(code: &str) -> BranchFileKind {
    if code.starts_with('A') {
        BranchFileKind::Added
    } else if code.starts_with('D') {
        BranchFileKind::Deleted
    } else if code.starts_with('R') || code.starts_with('C') {
        BranchFileKind::Renamed
    } else {
        BranchFileKind::Modified
    }
}

fn parse_name_status_line(raw_line: &str) -> Option<BranchChangedFile> {
    let mut parts = raw_line.trim_end_matches('\r').split('\t');
    let kind = classify_branch_status(parts.next()?.trim());
    let first = parts.next()?.trim();
    let path = if kind == BranchFileKind::Renamed { parts.next().map_or(first, str::trim) } else { first };
    (!path.is_empty()).then(|| BranchChangedFile { path: path.to_owned(), kind })
}

pub fn parse_name_status(raw: &str) -> (usize, Vec<BranchChangedFile>) {
    let mut count = 0usize;
    let mut files = Vec::new();
    for item in raw.lines().filter_map(parse_name_status_line) {
        count += 1;
        if files.len() < MAX_BRANCH_FILES {
            files.push(item);
        }
    }
    (count, files)
}

fn count_ahead_commits(repo_root: &Path, merge_base: &str) -> usize {
    let range = format!("{merge_base}..HEAD");
    git_stdout_if_ok(repo_root, &["rev-list", "--count", &range])
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0)
}

fn fetch_branch_commits(repo_root: &Path, base: &str) -> Vec<BranchCommit> {
    let range = format!("{base}..HEAD");
    git_stdout_if_ok(repo_root, &["log", "--oneline", "-n", "12", &range])
        .map(|s| parse_branch_commits(&s))
        .unwrap_or_default()
}

fn fetch_branch_files(repo_root: &Path, base: &str) -> (usize, Vec<BranchChangedFile>) {
    let range = format!("{base}...HEAD");
    git_stdout_if_ok(repo_root, &["diff", "--name-status", &range])
        .map(|s| parse_name_status(&s))
        .unwrap_or((0, Vec::new()))
}

fn empty_branch_review(repo_root: &Path, head: &GitHeadInfo) -> BranchReview {
    BranchReview {
        repo_root: repo_root.to_string_lossy().into_owned(),
        head: head.head.clone(),
        is_linked_worktree: head.is_linked_worktree,
        ..Default::default()
    }
}

fn populate_ahead_review(repo_root: &Path, base: &str, mb: &str, review: &mut BranchReview) {
    let ahead = count_ahead_commits(repo_root, mb);
    review.ahead_count = ahead;
    if ahead > 0 {
        review.commits = fetch_branch_commits(repo_root, base);
        let (count, files) = fetch_branch_files(repo_root, base);
        review.changed_count = count;
        review.files = files;
    }
}

pub fn collect_branch_review(repo_root: &Path, head: &GitHeadInfo) -> BranchReview {
    let mut review = empty_branch_review(repo_root, head);
    if !repo_root.is_dir() {
        return review;
    }
    let Some(base) = detect_base_branch(repo_root, &head.head) else {
        return review;
    };
    let merge_base = git_stdout_if_ok(repo_root, &["merge-base", &base, "HEAD"]);
    review.base_branch = Some(base.clone());
    if let Some(ref mb) = merge_base {
        populate_ahead_review(repo_root, &base, mb, &mut review);
    }
    review.merge_base = merge_base;
    review
}

pub fn format_summary_with_branch(base_summary: &str, review: &BranchReview) -> String {
    let Some(ref base) = review.base_branch else {
        return base_summary.to_owned();
    };
    if review.ahead_count == 0 {
        return base_summary.to_owned();
    }
    format!("{base_summary}  ·  {}: {} ({base}..HEAD)", LBL_COMMITS_WORD.as_str(), review.ahead_count)
}

pub fn build_branch_diff_pane_argv<'a>(
    repo_root: &'a str,
    range: &'a str,
    rel_path: &'a str,
    shell: &'a str,
) -> [&'a str; 8] {
    ["sh", "-c", BRANCH_DIFF_SCRIPT, "akhterm-branch-diff", repo_root, range, rel_path, shell]
}

pub fn build_open_pr_argv<'a>(repo_root: &'a str, shell: &'a str) -> [&'a str; 6] {
    ["sh", "-c", OPEN_PR_SCRIPT, "akhterm-open-pr", repo_root, shell]
}

fn count_dirty_files(root: &Path) -> Result<usize, String> {
    let out = run_git_c(root, &["status", "--porcelain=v1", "-uall"])?;
    if !out.status.success() {
        return Err("⚠ Не удалось проверить статус Git".to_owned());
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let (count, _) = parse_porcelain_status(&raw);
    Ok(count.max(usize::from(!raw.trim().is_empty())))
}

pub fn assess_worktree_removal_safety(wt_dir: &Path) -> Result<WorktreeSafety, String> {
    let canon = fs::canonicalize(wt_dir).map_err(|_| "⚠ Каталог вкладки не найден".to_owned())?;
    let (wt_root, head) = detect_git_repo(&canon)
        .ok_or_else(|| "⚠ Каталог вкладки не является Git-репозиторием".to_owned())?;
    let dirty_count = count_dirty_files(&wt_root)?;
    let review = collect_branch_review(&wt_root, &head);
    Ok(WorktreeSafety { dirty_count, unmerged_commits: review.ahead_count, base_branch: review.base_branch })
}

fn resolve_linked_wt_and_primary(wt_dir: &Path) -> Result<(PathBuf, GitHeadInfo, PathBuf), String> {
    let canon = fs::canonicalize(wt_dir).map_err(|_| "⚠ Каталог вкладки не найден".to_owned())?;
    let (wt_root, head) = detect_git_repo(&canon)
        .ok_or_else(|| "⚠ Каталог вкладки не является Git-репозиторием".to_owned())?;
    if !head.is_linked_worktree {
        return Err("⚠ Операция доступна только для дополнительного Git worktree".to_owned());
    }
    let main_repo = resolve_primary_worktree_root(&wt_root)
        .ok_or_else(|| "⚠ Не найден основной репозиторий для worktree".to_owned())?;
    if main_repo == wt_root {
        return Err("⚠ Нельзя выполнить операцию над основным репозиторием".to_owned());
    }
    Ok((wt_root, head, main_repo))
}

fn verify_merge_preconditions(wt_root: &Path, main_repo: &Path, wt_branch: &str) -> Result<String, String> {
    if count_dirty_files(wt_root)? > 0 {
        return Err("⚠ Нельзя влить нечистый worktree: зафиксируйте изменения".to_owned());
    }
    if count_dirty_files(main_repo)? > 0 {
        return Err("⚠ В основном репозитории есть незакоммиченные изменения".to_owned());
    }
    resolve_local_merge_base(wt_root, main_repo, wt_branch)
}

fn resolve_local_merge_base(wt_root: &Path, main_repo: &Path, wt_branch: &str) -> Result<String, String> {
    let detected = detect_base_branch(wt_root, wt_branch)
        .ok_or_else(|| "⚠ Не найдена базовая ветка (main / master)".to_owned())?;
    let base = detected.strip_prefix("origin/").unwrap_or(&detected).to_owned();
    let main_head = git_stdout_if_ok(main_repo, &["symbolic-ref", "--short", "-q", "HEAD"])
        .ok_or_else(|| "⚠ Основной репозиторий находится в detached HEAD".to_owned())?;
    if main_head != base {
        return Err(format!("⚠ Основной репозиторий на ветке '{main_head}', нужна '{base}'"));
    }
    Ok(base)
}

fn is_fast_forward_merge(main_repo: &Path, pre_head: &str, wt_branch: &str) -> bool {
    run_git_c(main_repo, &["merge-base", "--is-ancestor", pre_head, wt_branch])
        .is_ok_and(|o| o.status.success())
}

fn first_stderr_line(out: &Output, fallback: &str) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(fallback).to_owned()
}

fn run_git_merge_or_abort(main_repo: &Path, wt_branch: &str, base_branch: &str) -> Result<(), String> {
    let out = run_git_c(main_repo, &["merge", "--no-edit", wt_branch])?;
    if out.status.success() {
        return Ok(());
    }
    let _ = run_git_c(main_repo, &["merge", "--abort"]);
    let detail = first_stderr_line(&out, "conflict");
    Err(format!("⚠ Конфликт при слиянии '{wt_branch}' в '{base_branch}' (выполнен abort): {detail}"))
}

fn exec_worktree_remove(main_repo: &Path, wt_root: &Path, force: bool) -> Result<(), String> {
    let wt_str = wt_root.to_str().ok_or_else(|| "⚠ Некорректный путь worktree".to_owned())?;
    let args: &[&str] = if force { &["worktree", "remove", "--force", "--", wt_str] } else { &["worktree", "remove", "--", wt_str] };
    let out = run_git_c(main_repo, args)?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!("⚠ Не удалось удалить worktree: {}", first_stderr_line(&out, "git error")))
}

fn remove_wt_and_delete_branch(main_repo: &Path, wt_root: &Path, branch: &str, force: bool) -> Result<bool, String> {
    exec_worktree_remove(main_repo, wt_root, force)?;
    let del_flag = if force { "-D" } else { "-d" };
    let can_delete = force || branch.starts_with("wt/");
    Ok(can_delete && run_git_c(main_repo, &["branch", del_flag, "--", branch]).is_ok_and(|o| o.status.success()))
}

pub fn merge_worktree_into_base(wt_dir: &Path) -> Result<WorktreeMergeOutcome, String> {
    let (wt_root, _, main_repo) = resolve_linked_wt_and_primary(wt_dir)?;
    let wt_branch = git_stdout_if_ok(&wt_root, &["symbolic-ref", "--short", "-q", "HEAD"])
        .ok_or_else(|| "⚠ Нельзя влить detached HEAD worktree".to_owned())?;
    let base_branch = verify_merge_preconditions(&wt_root, &main_repo, &wt_branch)?;
    let pre_head = git_stdout_if_ok(&main_repo, &["rev-parse", "HEAD"]).unwrap_or_default();
    let was_fast_forward = is_fast_forward_merge(&main_repo, &pre_head, &wt_branch);
    run_git_merge_or_abort(&main_repo, &wt_branch, &base_branch)?;
    let branch_deleted = remove_wt_and_delete_branch(&main_repo, &wt_root, &wt_branch, false)?;
    Ok(WorktreeMergeOutcome { main_repo, wt_root, wt_branch, base_branch, was_fast_forward, branch_deleted })
}

pub fn force_remove_worktree(wt_dir: &Path) -> Result<ForceRemovalOutcome, String> {
    let (wt_root, head_info, main_repo) = resolve_linked_wt_and_primary(wt_dir)?;
    let branch = git_stdout_if_ok(&wt_root, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or(head_info.head);
    let branch_deleted = remove_wt_and_delete_branch(&main_repo, &wt_root, &branch, true)?;
    Ok(ForceRemovalOutcome { main_repo, wt_root, branch, branch_deleted })
}

pub fn reset_review_ui_state(popover: *mut GtkWidget, root: &str, review: &BranchReview) {
    let branch_files = build_branch_file_entries(root, review);
    REVIEW_UI.with(|cell| {
        let mut st = cell.borrow_mut();
        st.popover = popover as usize;
        st.wt_root = root.to_owned();
        st.armed_force_remove = false;
        st.branch_files = branch_files;
    });
}

fn build_branch_file_entries(root: &str, review: &BranchReview) -> Vec<BranchFileEntry> {
    let Some(ref base) = review.base_branch else {
        return Vec::new();
    };
    review.files.iter().map(|f| BranchFileEntry {
        repo_root: root.to_owned(),
        base_branch: base.clone(),
        path: f.path.clone(),
    }).collect()
}

fn make_review_section_lbl(title: &CStr) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(title.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, "section-header");
        lbl
    }
}

fn make_review_muted_lbl(content: &str) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        gtk_label_set_ellipsize(lbl, PANGO_ELLIPSIZE_END);
        gtk_label_set_max_width_chars(lbl, 58);
        add_class(lbl, "ssh-count");
        set_label_text(lbl, content);
        lbl
    }
}

fn make_action_btn(label: UiText, tip: UiText, cb: extern "C" fn(*mut GtkWidget, *mut c_void)) -> *mut GtkWidget {
    unsafe {
        let btn = gtk_button_new_with_label(label.as_ptr());
        gtk_widget_set_tooltip_text(btn, tip.as_ptr());
        connect_click(btn, cb, ptr::null_mut());
        btn
    }
}

pub fn pack_worktree_actions(vbox: *mut GtkWidget) {
    unsafe {
        gtk_box_pack_start(vbox, make_review_section_lbl(HDR_WT_ACTIONS.cstr()), 0, 0, 0);
        let merge_btn = make_action_btn(BTN_MERGE_WT, TIP_MERGE_WT, on_merge_wt_clicked);
        add_class(merge_btn, "btn-mi-a");
        gtk_box_pack_start(vbox, merge_btn, 0, 1, 1);
        gtk_box_pack_start(vbox, make_action_btn(BTN_OPEN_PR, TIP_OPEN_PR, on_open_pr_clicked), 0, 1, 1);
        gtk_box_pack_start(vbox, make_action_btn(BTN_REMOVE_WT, TIP_REMOVE_WT, on_remove_wt_clicked), 0, 1, 1);
    }
}

pub fn pack_branch_review(vbox: *mut GtkWidget, review: &BranchReview) {
    if review.ahead_count == 0 {
        return;
    }
    if let Some(ref base) = review.base_branch {
        pack_branch_commits_section(vbox, base, review);
        pack_branch_files_section(vbox, base, review);
    }
}

fn pack_branch_commits_section(vbox: *mut GtkWidget, base: &str, review: &BranchReview) {
    let header = format!("{} ({base}..HEAD): {}", HDR_BRANCH_COMMITS.as_str(), review.ahead_count);
    let hdr_lbl = make_review_section_lbl(c"");
    set_label_text(hdr_lbl, &header);
    unsafe { gtk_box_pack_start(vbox, hdr_lbl, 0, 0, 0) };
    for commit in &review.commits {
        let line = format!("• {}  {}", commit.sha, commit.subject);
        unsafe { gtk_box_pack_start(vbox, make_review_muted_lbl(&line), 0, 1, 0) };
    }
}

fn make_branch_file_btn(file: &BranchChangedFile, idx: usize) -> *mut GtkWidget {
    let row_txt = format!("[{}]  {}", file.kind.tag(), file.path);
    unsafe {
        let btn = gtk_button_new();
        let lbl = gtk_label_new(c"".as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        gtk_label_set_ellipsize(lbl, PANGO_ELLIPSIZE_MIDDLE);
        gtk_label_set_max_width_chars(lbl, 56);
        set_label_text(lbl, &row_txt);
        gtk_container_add(btn, lbl);
        gtk_widget_set_tooltip_text(btn, TIP_BRANCH_DIFF.as_ptr());
        connect_click(btn, on_branch_file_clicked, idx as *mut c_void);
        btn
    }
}

fn wrap_branch_files_scroll(list: *mut GtkWidget, count: usize) -> *mut GtkWidget {
    if count <= 8 {
        return list;
    }
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_width(scroll, 420);
        gtk_scrolled_window_set_min_content_height(scroll, 220);
        gtk_scrolled_window_set_max_content_height(scroll, 320);
        gtk_container_add(scroll, list);
        scroll
    }
}

fn pack_branch_files_section(vbox: *mut GtkWidget, base: &str, review: &BranchReview) {
    let header = format!("{} ({base}...HEAD): {}", HDR_BRANCH_FILES.as_str(), review.changed_count);
    let hdr_lbl = make_review_section_lbl(c"");
    set_label_text(hdr_lbl, &header);
    unsafe { gtk_box_pack_start(vbox, hdr_lbl, 0, 0, 0) };
    let list_box = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 3) };
    for (idx, file) in review.files.iter().enumerate() {
        unsafe { gtk_box_pack_start(list_box, make_branch_file_btn(file, idx), 0, 1, 0) };
    }
    unsafe { gtk_box_pack_start(vbox, wrap_branch_files_scroll(list_box, review.files.len()), 1, 1, 0) };
    if review.changed_count > review.files.len() {
        let extra = format!("{} {}", LBL_AND_MORE.as_str(), review.changed_count - review.files.len());
        unsafe { gtk_box_pack_start(vbox, make_review_muted_lbl(&extra), 0, 1, 0) };
    }
}

fn popdown_review_popover() {
    let pop = REVIEW_UI.with(|cell| cell.borrow().popover as *mut GtkWidget);
    if !pop.is_null() {
        unsafe { gtk_popover_popdown(pop) };
    }
    pane_actions::dismiss_git_popover();
}

fn active_local_split_ctx() -> Option<(usize, String, f64)> {
    let local_ok = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.id == app.active_id)?.active_local_cwd()
    })
    .flatten();
    let (sid, _, _, shell, scale) = local_ok.and_then(|_| session_ops::local_split_context(None, ptr::null_mut()))?;
    Some((sid, shell, scale))
}

fn prepare_review_split(
    sid: usize,
    new_scroll: *mut GtkWidget,
    new_vte: *mut GtkWidget,
    new_pid: i32,
    repo_root: &str,
) -> Option<split_pane::SplitOp> {
    app_state::with_app_mut(|app| {
        let s = app.sessions.iter_mut().find(|s| s.id == sid)?;
        Some(s.panes.prepare_split(
            s.scroll_wrap, s.vte, new_scroll, new_vte, new_pid, repo_root.to_owned(), GTK_ORIENTATION_HORIZONTAL,
        ))
    })
    .flatten()
}

fn launch_split_with_argv(sid: usize, scale: f64, repo_root: &str, argv: &[&str], msg: &str) {
    let (new_vte, new_pid) = session_ops::create_terminal(repo_root, argv, sid, scale);
    let new_scroll = split_pane::wrap_vte_scroll(new_vte);
    let split_op = prepare_review_split(sid, new_scroll, new_vte, new_pid, repo_root);
    split_pane::finish_split(split_op, new_scroll, new_vte);
    topbar::refresh_ui();
    app_state::set_footer_message(msg);
}

extern "C" fn on_branch_file_clicked(_btn: *mut GtkWidget, idx_ptr: *mut c_void) {
    let entry = REVIEW_UI.with(|cell| cell.borrow().branch_files.get(idx_ptr as usize).cloned());
    popdown_review_popover();
    let Some(item) = entry else {
        return;
    };
    let Some((sid, shell, scale)) = active_local_split_ctx() else {
        return app_state::set_footer_message(LBL_LOCAL_ONLY.as_str());
    };
    let range = format!("{}...HEAD", item.base_branch);
    let argv = build_branch_diff_pane_argv(&item.repo_root, &range, &item.path, &shell);
    launch_split_with_argv(sid, scale, &item.repo_root, &argv, &format!("⎇ Diff ({range}): {}", item.path));
}

extern "C" fn on_open_pr_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    let wt_root = REVIEW_UI.with(|cell| cell.borrow().wt_root.clone());
    popdown_review_popover();
    if wt_root.is_empty() {
        return;
    }
    let Some((sid, shell, scale)) = active_local_split_ctx() else {
        return app_state::set_footer_message(LBL_LOCAL_ONLY.as_str());
    };
    let argv = build_open_pr_argv(&wt_root, &shell);
    launch_split_with_argv(sid, scale, &wt_root, &argv, "⎇ PR: git push + gh pr create");
}

struct WtActionDone {
    sid: usize,
    profile: String,
    cwd: String,
    outcome: Result<(PathBuf, PathBuf, String), String>,
}

fn profile_key_for_wt(app: &AppState, profile: &str) -> String {
    if app.config.profile(profile).monitor { app.config.profile("shell").key } else { profile.to_owned() }
}

fn active_wt_target() -> Option<(usize, String, String)> {
    app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == app.active_id)?;
        let Tab::Local { ref profile, .. } = s.tab else {
            return None;
        };
        Some((s.id, profile_key_for_wt(app, profile), s.active_local_cwd()?))
    })
    .flatten()
}

fn find_main_repo_tab(sessions: &[TermSession], skip_sid: usize, main_root: &str, home: &str) -> Option<usize> {
    sessions.iter().find_map(|s| {
        if s.id == skip_sid || s.remote().is_some() {
            return None;
        }
        let (root, _) = resolve_git_repo(&s.active_local_cwd()?, home)?;
        (root == main_root).then_some(s.id)
    })
}

fn finish_wt_tab_cleanup(sid: usize, profile: &str, wt_root: &Path, main_repo: &Path, msg: &str) {
    let wt_str = wt_root.to_string_lossy().into_owned();
    let main_str = main_repo.to_string_lossy().into_owned();
    let nav = app_state::with_app(|app| {
        app.git_scheduler.remove_root(&wt_str);
        app.git_scheduler.request_refresh(&main_str);
        let has_tab = app.sessions.iter().any(|s| s.id == sid);
        (has_tab, has_tab && app.sessions.len() == 1, find_main_repo_tab(&app.sessions, sid, &main_str, &home_dir()))
    });
    if let Some((has_tab, sole_tab, main_sid)) = nav {
        apply_post_wt_nav(sid, profile, &main_str, has_tab, sole_tab, main_sid);
    }
    app_state::set_footer_message(msg);
}

fn apply_post_wt_nav(sid: usize, profile: &str, main_str: &str, has_tab: bool, sole_tab: bool, main_sid: Option<usize>) {
    if sole_tab {
        session_ops::open_local_tab(profile.to_owned(), main_str);
        session_ops::close_session(sid);
    } else if has_tab {
        session_ops::close_session(sid);
        if let Some(target_sid) = main_sid {
            session_ops::select_session(target_sid);
        }
    }
}

extern "C" fn on_wt_action_done(data: *mut c_void) -> c_int {
    WT_REMOVE_BUSY.with(|b| b.set(false));
    if data.is_null() {
        return 0;
    }
    let done = unsafe { *Box::from_raw(data as *mut WtActionDone) };
    match done.outcome {
        Ok((wt_root, main_repo, msg)) => finish_wt_tab_cleanup(done.sid, &done.profile, &wt_root, &main_repo, &msg),
        Err(err) => {
            if let Some((root, _)) = resolve_git_repo(&done.cwd, &home_dir()) {
                app_state::with_app(|app| app.git_scheduler.request_refresh(&root));
            }
            app_state::set_footer_message(&err);
            app_state::focus_active_vte();
        }
    }
    0
}

fn dispatch_wt_bg<F>(op: F)
where
    F: FnOnce(PathBuf) -> Result<(PathBuf, PathBuf, String), String> + Send + 'static,
{
    if WT_REMOVE_BUSY.with(|b| b.replace(true)) {
        return;
    }
    let Some((sid, profile, cwd)) = active_wt_target() else {
        WT_REMOVE_BUSY.with(|b| b.set(false));
        let wt_root = REVIEW_UI.with(|cell| cell.borrow().wt_root.clone());
        let _ = op(PathBuf::from(wt_root));
        return;
    };
    app_state::focus_active_vte();
    thread::spawn(move || {
        let outcome = op(PathBuf::from(&cwd));
        let payload = Box::new(WtActionDone { sid, profile, cwd, outcome });
        unsafe { g_idle_add(on_wt_action_done, Box::into_raw(payload) as *mut c_void) };
    });
}

extern "C" fn on_merge_wt_clicked(_btn: *mut GtkWidget, _data: *mut c_void) {
    popdown_review_popover();
    dispatch_wt_bg(|path| {
        let out = merge_worktree_into_base(&path)?;
        let msg = out.success_message();
        Ok((out.wt_root, out.main_repo, msg))
    });
}

fn arm_remove_button(btn: *mut GtkWidget) {
    REVIEW_UI.with(|cell| cell.borrow_mut().armed_force_remove = true);
    unsafe {
        gtk_button_set_label(btn, BTN_CONFIRM_FORCE_REMOVE.as_ptr());
        gtk_widget_set_tooltip_text(btn, TIP_CONFIRM_FORCE_REMOVE.as_ptr());
        add_class(btn, "btn-mi-c");
    }
}

extern "C" fn on_remove_wt_clicked(btn: *mut GtkWidget, _data: *mut c_void) {
    let (wt_root, armed) = REVIEW_UI.with(|cell| {
        let st = cell.borrow();
        (st.wt_root.clone(), st.armed_force_remove)
    });
    if wt_root.is_empty() {
        return;
    }
    if armed {
        return run_force_remove();
    }
    let needs_confirm = assess_worktree_removal_safety(Path::new(&wt_root)).map_or(true, |s| s.needs_force_confirm());
    if needs_confirm { arm_remove_button(btn); } else { run_clean_remove(); }
}

fn run_clean_remove() {
    popdown_review_popover();
    dispatch_wt_bg(|path| {
        let out = remove_clean_worktree(&path)?;
        let msg = out.success_message();
        Ok((out.wt_root, out.main_repo, msg))
    });
}

fn run_force_remove() {
    REVIEW_UI.with(|cell| cell.borrow_mut().armed_force_remove = false);
    popdown_review_popover();
    dispatch_wt_bg(|path| {
        let out = force_remove_worktree(&path)?;
        let msg = out.success_message();
        Ok((out.wt_root, out.main_repo, msg))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_status::{rebuild_git_widgets, GitMenu, GitMenuCallbacks, GitRepoStatus};
    use crate::git_wt::create_task_worktree;
    use crate::i18n::UiLocale;
    use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};

    struct TempGuard(PathBuf);

    impl TempGuard {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("akhterm_rev_{tag}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git_ok(cwd: &Path, args: &[&str]) {
        let out = run_git_c(cwd, args).unwrap();
        assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
    }

    fn git_commit_all(cwd: &Path, msg: &str) {
        git_ok(cwd, &["add", "."]);
        git_ok(cwd, &["-c", "commit.gpgsign=false", "-c", "user.name=T", "-c", "user.email=t@e", "commit", "-m", msg]);
    }

    fn init_repo(repo: &Path) {
        fs::create_dir_all(repo).unwrap();
        git_ok(repo, &["init", "-b", "main"]);
        fs::write(repo.join("README.md"), "base line\n").unwrap();
        git_commit_all(repo, "init");
    }

    extern "C" {
        fn gtk_container_get_children(container: *mut GtkWidget) -> *mut GList;
        fn g_list_free(list: *mut GList);
    }

    #[repr(C)]
    struct GList {
        data: *mut c_void,
        next: *mut GList,
        prev: *mut GList,
    }

    fn button_label_if_matches(widget: *mut GtkWidget, needle: &str) -> Option<*mut GtkWidget> {
        let kind = unsafe { CStr::from_ptr(g_type_name_from_instance(widget)).to_string_lossy() };
        if kind != "GtkButton" {
            return None;
        }
        let lbl = unsafe { gtk_button_get_label(widget) };
        let direct = unsafe { !lbl.is_null() && CStr::from_ptr(lbl).to_string_lossy().contains(needle) };
        if direct || walk_children_for_label(widget, needle) { Some(widget) } else { None }
    }

    fn walk_find_button(root: *mut GtkWidget, needle: &str) -> Option<*mut GtkWidget> {
        if let Some(found) = button_label_if_matches(root, needle) {
            return Some(found);
        }
        let children = unsafe { gtk_container_get_children(root) };
        let mut cur = children;
        while !cur.is_null() {
            if let Some(found) = walk_find_button(unsafe { (*cur).data as *mut GtkWidget }, needle) {
                unsafe { g_list_free(children) };
                return Some(found);
            }
            cur = unsafe { (*cur).next };
        }
        unsafe { g_list_free(children) };
        None
    }

    fn has_label_text(root: *mut GtkWidget, needle: &str) -> bool {
        let text_match = unsafe {
            let kind = CStr::from_ptr(g_type_name_from_instance(root)).to_string_lossy();
            let ptr = if kind == "GtkLabel" { gtk_label_get_text(root) } else { ptr::null() };
            !ptr.is_null() && CStr::from_ptr(ptr).to_string_lossy().contains(needle)
        };
        text_match || walk_children_for_label(root, needle)
    }

    fn walk_children_for_label(root: *mut GtkWidget, needle: &str) -> bool {
        let children = unsafe { gtk_container_get_children(root) };
        let mut cur = children;
        while !cur.is_null() {
            if has_label_text(unsafe { (*cur).data as *mut GtkWidget }, needle) {
                unsafe { g_list_free(children) };
                return true;
            }
            cur = unsafe { (*cur).next };
        }
        unsafe { g_list_free(children) };
        false
    }

    extern "C" fn noop_click(_w: *mut GtkWidget, _d: *mut c_void) {}
    extern "C" fn noop_key(_w: *mut GtkWidget, _e: *const GdkEvent, _d: *mut c_void) -> c_int { 0 }

    fn build_popover_for_repo(wt: &Path, branch: &str) -> (*mut GtkWidget, GitMenu, *mut GtkWidget) {
        unsafe {
            let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
            let btn = gtk_button_new_with_label(c"Git".as_ptr());
            gtk_container_add(win, btn);
            let mut menu = GitMenu::new(btn, noop_click, noop_key);
            let head = GitHeadInfo { head: branch.to_owned(), is_linked_worktree: true };
            let status = GitRepoStatus { root: wt.to_string_lossy().into_owned(), head: branch.to_owned(), is_linked_worktree: true, ..Default::default() };
            let cbs = GitMenuCallbacks { on_file: noop_click, on_remove_wt: noop_click };
            let (inner, entries) = rebuild_git_widgets(menu.take_widgets(), Some((&status.root, &head)), Some(&status), cbs);
            menu.save_snapshot(inner, entries);
            gtk_widget_show_all(win);
            pump_gtk_events();
            (win, menu, inner)
        }
    }

    #[test]
    fn committed_agent_worktree_shows_branch_commits_and_files_vs_main() {
        let tmp = TempGuard::new("layer2");
        let repo = tmp.0.join("Проект");
        init_repo(&repo);
        let (wt, branch) = create_task_worktree(&repo, "claude", Some("auth-fix")).unwrap();
        fs::write(wt.join("auth.rs"), "pub fn ok() {}\n").unwrap();
        fs::write(wt.join("README.md"), "updated\n").unwrap();
        git_commit_all(&wt, "feat: add auth");

        let head = GitHeadInfo { head: branch, is_linked_worktree: true };
        let review = collect_branch_review(&wt, &head);
        assert_eq!(review.base_branch.as_deref(), Some("main"));
        assert_eq!(review.ahead_count, 1);
        assert_eq!(review.commits[0].subject, "feat: add auth");
        assert!(review.files.iter().any(|f| f.path == "auth.rs" && f.kind == BranchFileKind::Added));
        assert!(review.files.iter().any(|f| f.path == "README.md" && f.kind == BranchFileKind::Modified));
    }

    #[test]
    fn headless_popover_shows_committed_wt_layer2_even_when_working_tree_is_clean() {
        with_headless_gtk(|| {
            let tmp = TempGuard::new("gtk_layer2");
            let repo = tmp.0.join("Repo");
            init_repo(&repo);
            let (wt, branch) = create_task_worktree(&repo, "codex", Some("clean-commit")).unwrap();
            fs::write(wt.join("new_mod.rs"), "pub fn hi() {}\n").unwrap();
            git_commit_all(&wt, "add new_mod");

            let (win, menu, inner) = build_popover_for_repo(&wt, &branch);
            assert!(menu.entry_at(0).is_none());
            assert!(has_label_text(inner, "add new_mod"));
            assert!(walk_find_button(inner, "new_mod.rs").is_some());
            assert!(walk_find_button(inner, "Merge into base").or_else(|| walk_find_button(inner, "Влить в базу")).is_some());
            assert!(walk_find_button(inner, "Open PR (gh)").or_else(|| walk_find_button(inner, "Открыть PR")).is_some());
            assert_eq!(BTN_MERGE_WT.for_locale(UiLocale::En).to_str().unwrap(), "Merge into base & remove worktree");
            assert_eq!(BTN_CONFIRM_FORCE_REMOVE.for_locale(UiLocale::Ru).to_str().unwrap(), "Подтвердить принудительное удаление");
            unsafe { gtk_widget_destroy(win) };
        });
    }

    #[test]
    fn merges_ff_and_removes_worktree_and_aborts_safely_on_conflict() {
        let tmp = TempGuard::new("merge_ff");
        let repo = tmp.0.join("Repo");
        init_repo(&repo);
        let (wt, branch) = create_task_worktree(&repo, "codex", Some("clean-feat")).unwrap();
        fs::write(wt.join("feature.txt"), "hello\n").unwrap();
        git_commit_all(&wt, "add feature");

        let merged = merge_worktree_into_base(&wt).unwrap();
        assert!(merged.was_fast_forward && merged.branch_deleted && !wt.exists());
        assert!(repo.join("feature.txt").exists());
        assert!(!git_ref_exists(&repo, &format!("refs/heads/{branch}")));

        let (wt_c, _) = create_task_worktree(&repo, "agy", Some("conflict")).unwrap();
        fs::write(wt_c.join("README.md"), "wt change\n").unwrap();
        git_commit_all(&wt_c, "wt edit");
        fs::write(repo.join("README.md"), "main change\n").unwrap();
        git_commit_all(&repo, "main edit");

        assert!(merge_worktree_into_base(&wt_c).unwrap_err().contains("abort"));
        assert!(wt_c.exists());
        assert_eq!(count_dirty_files(&repo).unwrap(), 0);
    }

    #[test]
    fn merges_diverged_non_conflicting_branch_with_merge_commit() {
        let tmp = TempGuard::new("merge_3way");
        let repo = tmp.0.join("Repo");
        init_repo(&repo);
        git_ok(&repo, &["config", "user.name", "Tester"]);
        git_ok(&repo, &["config", "user.email", "t@example.com"]);
        let (wt, branch) = create_task_worktree(&repo, "claude", Some("side-file")).unwrap();
        fs::write(wt.join("side.txt"), "side\n").unwrap();
        git_commit_all(&wt, "add side");
        fs::write(repo.join("main.txt"), "main\n").unwrap();
        git_commit_all(&repo, "add main");

        let merged = merge_worktree_into_base(&wt).unwrap();
        assert!(!merged.was_fast_forward && merged.branch_deleted && !wt.exists());
        assert!(repo.join("side.txt").exists() && repo.join("main.txt").exists());
        assert!(!git_ref_exists(&repo, &format!("refs/heads/{branch}")));
    }

    #[test]
    fn two_step_force_remove_confirmation_and_immediate_clean_remove_in_gtk() {
        with_headless_gtk(|| {
            let tmp = TempGuard::new("gtk_force");
            let repo = tmp.0.join("GtkRepo");
            init_repo(&repo);
            let (wt_clean, br_clean) = create_task_worktree(&repo, "agy", Some("empty-wt")).unwrap();
            let (win_c, _, inner_c) = build_popover_for_repo(&wt_clean, &br_clean);
            let rm_clean = walk_find_button(inner_c, "Remove worktree").or_else(|| walk_find_button(inner_c, "Удалить worktree")).unwrap();
            unsafe { gtk_button_clicked(rm_clean) };
            pump_gtk_events();
            assert!(!wt_clean.exists(), "clean worktree with 0 commits removes immediately");
            unsafe { gtk_widget_destroy(win_c) };

            let (wt, branch) = create_task_worktree(&repo, "claude", Some("unmerged")).unwrap();
            fs::write(wt.join("work.rs"), "fn f() {}\n").unwrap();
            git_commit_all(&wt, "unmerged commit");
            let (win, _, inner) = build_popover_for_repo(&wt, &branch);
            let rm_btn = walk_find_button(inner, "Remove worktree").or_else(|| walk_find_button(inner, "Удалить worktree")).unwrap();
            unsafe { gtk_button_clicked(rm_btn) };
            pump_gtk_events();
            assert!(wt.exists(), "first click arms confirmation");
            unsafe { gtk_button_clicked(rm_btn) };
            pump_gtk_events();
            assert!(!wt.exists() && !git_ref_exists(&repo, &format!("refs/heads/{branch}")));
            unsafe { gtk_widget_destroy(win) };
        });
    }

    #[test]
    fn open_pr_and_branch_diff_argv_escape_paths_safely() {
        let diff = build_branch_diff_pane_argv("/tmp/репо", "main...HEAD", "-weird;file.rs", "/bin/sh");
        assert_eq!((diff[4], diff[5], diff[6]), ("/tmp/репо", "main...HEAD", "-weird;file.rs"));
        assert!(build_open_pr_argv("/tmp/репо", "/bin/sh")[2].contains("git push -u origin HEAD && gh pr create --fill"));
    }

    #[test]
    fn git_review_module_meets_maintainability_target() {
        let (_, _, score) = crate::mi::compute_file_mi(include_str!("git_review.rs"));
        assert!(score >= 60.0, "git_review.rs MI {score:.2} < 60.0");
    }
}
