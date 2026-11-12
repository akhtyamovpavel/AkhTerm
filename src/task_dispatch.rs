//! 1-step multi-agent task dispatch modal, atomic `0600` brief files, and worktree fan-out.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;

use crate::app_state;
use crate::approval::read_vte_visible_text;
use crate::ffi::*;
use crate::git_wt::agent_slug_from_cmd;
use crate::handoff::{current_unix_secs, discover_handoff_files};
use crate::i18n::{current_locale, UiLocale, UiText};
use crate::paths::{shorten_home, state_dir, write_state_file};
use crate::proc_radar::inspect_proc_tree;
use crate::session_ops;
use crate::ui_text as text;

const GTK_WIN_POS_CENTER_ON_PARENT: c_int = 4;
const GTK_WRAP_WORD_CHAR: c_int = 3;
const KEYCODE_N: u16 = 57;
const MAX_SLUG_LEN: usize = 36;
const MAX_GH_ISSUES: usize = 15;
const DELIVERY_TICK_MS: c_uint = 150;

static DISPATCH_WT_SEQ: AtomicU32 = AtomicU32::new(1);

#[repr(C)]
struct GtkTextIterBuf([u64; 16]);

impl GtkTextIterBuf {
    fn zeroed() -> Self {
        Self([0; 16])
    }
}

extern "C" {
    fn gtk_window_set_transient_for(window: *mut GtkWidget, parent: *mut GtkWidget);
    fn gtk_window_set_modal(window: *mut GtkWidget, modal: c_int);
    fn gtk_window_set_position(window: *mut GtkWidget, position: c_int);
    fn gtk_text_view_new() -> *mut GtkWidget;
    fn gtk_text_view_get_buffer(text_view: *mut GtkWidget) -> *mut c_void;
    fn gtk_text_view_set_wrap_mode(text_view: *mut GtkWidget, wrap_mode: c_int);
    fn gtk_text_view_set_left_margin(text_view: *mut GtkWidget, margin: c_int);
    fn gtk_text_view_set_right_margin(text_view: *mut GtkWidget, margin: c_int);
    fn gtk_text_view_set_top_margin(text_view: *mut GtkWidget, margin: c_int);
    fn gtk_text_view_set_bottom_margin(text_view: *mut GtkWidget, margin: c_int);
    fn gtk_text_buffer_get_start_iter(buffer: *mut c_void, iter: *mut GtkTextIterBuf);
    fn gtk_text_buffer_get_end_iter(buffer: *mut c_void, iter: *mut GtkTextIterBuf);
    fn gtk_text_buffer_get_text(
        buffer: *mut c_void,
        start: *const GtkTextIterBuf,
        end: *const GtkTextIterBuf,
        include_hidden: c_int,
    ) -> *mut c_char;
    fn gtk_text_buffer_set_text(buffer: *mut c_void, text: *const c_char, len: c_int);
    fn gtk_combo_box_text_new() -> *mut GtkWidget;
    fn gtk_combo_box_text_append_text(combo: *mut GtkWidget, text: *const c_char);
    fn gtk_combo_box_text_remove_all(combo: *mut GtkWidget);
    fn gtk_combo_box_get_active(combo: *mut GtkWidget) -> c_int;
    fn gtk_combo_box_set_active(combo: *mut GtkWidget, index: c_int);
    fn gtk_check_button_new_with_label(label: *const c_char) -> *mut GtkWidget;
    fn gtk_widget_set_margin_start(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_margin_end(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_margin_top(widget: *mut GtkWidget, margin: c_int);
    fn gtk_widget_set_margin_bottom(widget: *mut GtkWidget, margin: c_int);
}

const TITLE_TEXT: UiText = UiText::new(
    c"Диспетчер задач агентов (Ctrl+Shift+N)",
    c"Agent Task Dispatch (Ctrl+Shift+N)",
);
const HINT_TEXT: UiText = UiText::new(
    c"Ctrl+Enter запуск · Esc отмена",
    c"Ctrl+Enter launch · Esc cancel",
);
const SOURCE_HEADER: UiText = UiText::new(
    c"ИСТОЧНИК (БРИФЫ .MD / GITHUB ISSUES)",
    c"SOURCE (.MD BRIEFS / GITHUB ISSUES)",
);
const SOURCE_NONE: UiText = UiText::new(
    c"Вручную (ввести промпт ниже)",
    c"Manual entry (type prompt below)",
);
const PROMPT_HEADER: UiText = UiText::new(
    c"ЗАДАНИЕ / ПРОМПТ ДЛЯ АГЕНТОВ",
    c"TASK PROMPT / BRIEF",
);
const SLUG_HEADER: UiText = UiText::new(
    c"SLUG ВЕТКИ (WT/<АГЕНТ>-<SLUG>)",
    c"BRANCH SLUG (WT/<AGENT>-<SLUG>)",
);
const SLUG_PLACEHOLDER: UiText = UiText::new(c"fix-auth-race", c"fix-auth-race");
const AGENTS_HEADER: UiText = UiText::new(
    c"АГЕНТЫ (ВЕЕРНЫЙ ЗАПУСК В ОТДЕЛЬНЫХ WORKTREE)",
    c"AGENTS (PARALLEL WORKTREE FAN-OUT)",
);
const CANCEL_BTN: UiText = UiText::new(c"Отмена", c"Cancel");
const LAUNCH_BTN: UiText = UiText::new(c"Запустить в worktree", c"Dispatch & Run");
const ERR_EMPTY_PROMPT: UiText = UiText::new(
    c"Введите текст задания или выберите бриф.",
    c"Enter a task prompt or choose a brief.",
);
const ERR_NO_AGENT: UiText = UiText::new(
    c"Выберите хотя бы одного агента.",
    c"Select at least one agent.",
);
const GH_LOADING: UiText = UiText::new(
    c"Загрузка открытых GitHub Issues…",
    c"Loading open GitHub Issues…",
);
const GH_OFFLINE: UiText = UiText::new(
    c"GitHub Issues недоступны (офлайн или не GitHub-репо).",
    c"GitHub Issues unavailable (offline or non-GitHub repo).",
);
const GH_LOADED: UiText = UiText::new(c"GitHub Issues загружены", c"GitHub Issues loaded");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubIssue {
    pub number: u32,
    pub title: String,
    pub labels: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchSourceItem {
    BriefFile(PathBuf),
    Issue(GitHubIssue),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchSubmit {
    pub prompt: String,
    pub slug: String,
    pub agent_indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchRequest {
    pub prompt: String,
    pub slug: String,
    pub agent_indices: Vec<usize>,
    pub base_cwd: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentLaunchSpec {
    pub agent_idx: usize,
    pub agent_slug: String,
    pub agent_cmd: String,
    pub branch_name: String,
    pub worktree_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FanoutPlan {
    pub slug: String,
    pub dispatch_file: PathBuf,
    pub delivery_prompt: String,
    pub launches: Vec<AgentLaunchSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryStage {
    WaitShell,
    WaitAgent,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryAction {
    Wait,
    SendAgentCmd,
    SendPromptAndFinish,
    GiveUp,
}

pub fn is_dispatch_shortcut(keyval: c_uint, state: c_uint, keycode: u16) -> bool {
    let ctrl = (state & GDK_CONTROL_MASK) != 0;
    let shift = (state & GDK_SHIFT_MASK) != 0;
    let no_alt = (state & GDK_MOD1_MASK) == 0;
    let key_n = matches!(keyval, 0x4e | 0x6e | 0x6d4 | 0x6f4) || keycode == KEYCODE_N;
    ctrl && shift && no_alt && key_n
}

fn cyrillic_to_latin(ch: char) -> Option<&'static str> {
    let mapped = match ch {
        'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' | 'ё' => "e",
        'ж' => "zh", 'з' => "z", 'и' | 'й' => "i", 'к' => "k", 'л' => "l", 'м' => "m",
        'н' => "n", 'о' => "o", 'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t",
        'у' => "u", 'ф' => "f", 'х' => "kh", 'ц' => "ts", 'ч' => "ch", 'ш' => "sh",
        'щ' => "shch", 'ъ' | 'ь' => "", 'ы' => "y", 'э' => "e", 'ю' => "yu", 'я' => "ya",
        _ => return None,
    };
    Some(mapped)
}

fn push_slug_char(out: &mut String, ch: char) {
    if ch.is_ascii_alphanumeric() {
        out.push(ch.to_ascii_lowercase());
    } else if let Some(lat) = ch.to_lowercase().next().and_then(cyrillic_to_latin) {
        out.push_str(lat);
    } else if !out.is_empty() && !out.ends_with('-') {
        out.push('-');
    }
}

fn truncate_slug(slug: &str) -> String {
    let trimmed = slug.trim_matches('-');
    if trimmed.len() <= MAX_SLUG_LEN {
        return trimmed.to_owned();
    }
    let slice = &trimmed[..MAX_SLUG_LEN];
    let cut = slice.rfind('-').filter(|&i| i >= 12).unwrap_or(MAX_SLUG_LEN);
    trimmed[..cut].trim_matches('-').to_owned()
}

pub fn sanitize_slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(64));
    for ch in raw.trim().chars() {
        push_slug_char(&mut out, ch);
        if out.len() >= MAX_SLUG_LEN + 12 {
            break;
        }
    }
    truncate_slug(&out)
}

fn first_meaningful_line(raw: &str) -> &str {
    raw.lines()
        .map(|l| l.trim().trim_start_matches(['#', '*', '-', ' ', '\t']))
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

pub fn derive_branch_slug(raw: &str) -> String {
    let clean = sanitize_slug(first_meaningful_line(raw));
    if clean.is_empty() { "task".to_owned() } else { clean }
}

pub fn format_worktree_branch(agent_slug: &str, task_slug: &str) -> String {
    format!("wt/{agent_slug}-{task_slug}")
}

pub fn dispatches_dir() -> PathBuf {
    state_dir().join("dispatches")
}

pub fn unique_dispatch_path(dir: &Path, slug: &str, ts: u64) -> PathBuf {
    let clean = sanitize_slug(slug);
    let tag = if clean.is_empty() { "task" } else { clean.as_str() };
    let primary = dir.join(format!("task_{tag}_{ts}.md"));
    if !primary.exists() {
        return primary;
    }
    (2..1000u32)
        .map(|seq| dir.join(format!("task_{tag}_{ts}_{seq}.md")))
        .find(|cand| !cand.exists())
        .unwrap_or(primary)
}

pub fn format_dispatch_markdown(slug: &str, prompt: &str, ts: u64) -> String {
    let trimmed = prompt.trim();
    if trimmed.starts_with('#') {
        format!("{trimmed}\n")
    } else {
        format!("# Task: {slug}\n\n- Timestamp: {ts}\n\n{trimmed}\n")
    }
}

pub fn write_dispatch_file(dir: &Path, slug: &str, prompt: &str, ts: u64) -> io::Result<PathBuf> {
    let path = unique_dispatch_path(dir, slug, ts);
    let body = format_dispatch_markdown(slug, prompt, ts);
    write_state_file(&path, &body)?;
    Ok(path)
}

pub fn format_delivery_prompt(path: &Path, locale: UiLocale) -> String {
    match locale {
        UiLocale::Ru => format!("Прочитай файл {} и действуй по нему.\n", path.display()),
        UiLocale::En | UiLocale::Auto => {
            format!("Read file {} and execute the task.\n", path.display())
        }
    }
}

fn parse_gh_issue_line(line: &str) -> Option<GitHubIssue> {
    let cols: Vec<&str> = line.split('\t').collect();
    let number = cols.first()?.trim().trim_start_matches('#').parse::<u32>().ok()?;
    let (title, labels) = match cols.as_slice() {
        [_, _, t, l, ..] => (t.trim(), l.trim()),
        [_, t] => (t.trim(), ""),
        _ => return None,
    };
    (!title.is_empty()).then(|| GitHubIssue {
        number,
        title: title.to_owned(),
        labels: labels.to_owned(),
    })
}

pub fn parse_gh_issue_list(raw: &str) -> Vec<GitHubIssue> {
    raw.lines()
        .filter_map(parse_gh_issue_line)
        .take(MAX_GH_ISSUES)
        .collect()
}

pub fn is_github_repo(cwd: &Path) -> bool {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["remote", "-v"])
        .stdin(Stdio::null())
        .output()
    else {
        return false;
    };
    out.status.success()
        && String::from_utf8_lossy(&out.stdout)
            .to_ascii_lowercase()
            .contains("github")
}

pub fn fetch_gh_issues(cwd: &Path) -> Result<Vec<GitHubIssue>, String> {
    if !cwd.is_dir() || !is_github_repo(cwd) {
        return Err("not-github".to_owned());
    }
    let out = Command::new("gh")
        .args(["issue", "list", "--limit", "15"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("gh unavailable: {e}"))?;
    if !out.status.success() {
        return Err("gh-failed".to_owned());
    }
    Ok(parse_gh_issue_list(&String::from_utf8_lossy(&out.stdout)))
}

pub fn format_issue_prompt(issue: &GitHubIssue) -> String {
    if issue.labels.is_empty() {
        format!("# Issue #{}: {}\n\nResolve GitHub issue #{}.\n", issue.number, issue.title, issue.number)
    } else {
        format!(
            "# Issue #{}: {}\n\nLabels: {}\n\nResolve GitHub issue #{}.\n",
            issue.number, issue.title, issue.labels, issue.number
        )
    }
}

fn build_launch_spec(
    idx: usize,
    repo_root: &Path,
    slug: &str,
) -> Option<AgentLaunchSpec> {
    let (_, cmd) = text::AGENT_ACTIONS.get(idx)?;
    let agent_slug = agent_slug_from_cmd(cmd);
    let parent = repo_root.parent().unwrap_or(repo_root);
    let repo_name = repo_root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let wt_dir = parent.join(format!("{repo_name}-wt-{agent_slug}-{slug}"));
    Some(AgentLaunchSpec {
        agent_idx: idx,
        agent_slug: agent_slug.clone(),
        agent_cmd: (*cmd).to_owned(),
        branch_name: format_worktree_branch(&agent_slug, slug),
        worktree_dir: wt_dir,
    })
}

pub fn build_fanout_plan(
    req: &DispatchRequest,
    repo_root: &Path,
    dispatch_file: PathBuf,
    locale: UiLocale,
) -> Result<FanoutPlan, String> {
    if req.prompt.trim().is_empty() {
        return Err(ERR_EMPTY_PROMPT.for_locale(locale).to_string_lossy().into_owned());
    }
    let chosen_slug = sanitize_slug(&req.slug);
    let slug = if chosen_slug.is_empty() { derive_branch_slug(&req.prompt) } else { chosen_slug };
    let launches: Vec<AgentLaunchSpec> = req
        .agent_indices
        .iter()
        .filter_map(|&idx| build_launch_spec(idx, repo_root, &slug))
        .collect();
    if launches.is_empty() {
        return Err(ERR_NO_AGENT.for_locale(locale).to_string_lossy().into_owned());
    }
    let delivery_prompt = format_delivery_prompt(&dispatch_file, locale);
    Ok(FanoutPlan { slug, dispatch_file, delivery_prompt, launches })
}

fn run_git_cmd(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git failed: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

pub fn resolve_repo_root(cwd: &Path) -> Result<PathBuf, String> {
    if !cwd.is_dir() {
        return Err("Directory not found".to_owned());
    }
    let root = run_git_cmd(cwd, &["rev-parse", "--show-toplevel"])?;
    (!root.is_empty())
        .then(|| PathBuf::from(root))
        .ok_or_else(|| "Not a Git repository".to_owned())
}

fn next_wt_suffix() -> String {
    let seq = DISPATCH_WT_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{:04x}", (current_unix_secs() as u32 & 0xfff) << 4 | (seq & 0xf))
}

fn try_git_worktree_add(repo_root: &Path, wt_path: &Path, branch: &str) -> Result<(), String> {
    let wt_str = wt_path.to_str().ok_or_else(|| "Invalid worktree path".to_owned())?;
    run_git_cmd(repo_root, &["worktree", "add", "-b", branch, wt_str]).map(|_| ())
}

pub fn create_task_worktree(
    repo_root: &Path,
    agent_slug: &str,
    task_slug: &str,
) -> Result<(PathBuf, String), String> {
    let spec = build_launch_spec_by_slug(repo_root, agent_slug, task_slug);
    if !spec.0.exists() && try_git_worktree_add(repo_root, &spec.0, &spec.1).is_ok() {
        return Ok(spec);
    }
    let suffixed = format!("{task_slug}-{}", next_wt_suffix());
    let fallback = build_launch_spec_by_slug(repo_root, agent_slug, &suffixed);
    try_git_worktree_add(repo_root, &fallback.0, &fallback.1)?;
    Ok(fallback)
}

fn build_launch_spec_by_slug(repo_root: &Path, agent_slug: &str, slug: &str) -> (PathBuf, String) {
    let parent = repo_root.parent().unwrap_or(repo_root);
    let name = repo_root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let wt_path = parent.join(format!("{name}-wt-{agent_slug}-{slug}"));
    (wt_path, format_worktree_branch(agent_slug, slug))
}

pub fn evaluate_delivery_step(
    stage: DeliveryStage,
    ticks: u8,
    vte_text: &str,
    proc_agent: &str,
    target_slug: &str,
) -> DeliveryAction {
    match stage {
        DeliveryStage::WaitShell => evaluate_shell_wait(ticks, vte_text),
        DeliveryStage::WaitAgent => evaluate_agent_wait(ticks, vte_text, proc_agent, target_slug),
        DeliveryStage::Done => DeliveryAction::GiveUp,
    }
}

fn evaluate_shell_wait(ticks: u8, vte_text: &str) -> DeliveryAction {
    let has_prompt = ticks >= 2 && !vte_text.trim().is_empty();
    if has_prompt || ticks >= 8 {
        DeliveryAction::SendAgentCmd
    } else {
        DeliveryAction::Wait
    }
}

fn evaluate_agent_wait(ticks: u8, vte_text: &str, proc_agent: &str, slug: &str) -> DeliveryAction {
    let agent_alive = proc_agent.eq_ignore_ascii_case(slug) && !vte_text.trim().is_empty();
    if (agent_alive && ticks >= 4) || ticks >= 14 {
        DeliveryAction::SendPromptAndFinish
    } else {
        DeliveryAction::Wait
    }
}

struct PendingDelivery {
    sid: usize,
    vte: *mut GtkWidget,
    agent_cmd: String,
    agent_slug: String,
    delivery_prompt: String,
    stage: DeliveryStage,
    ticks: u8,
}

fn lookup_pane_pid(sid: usize, vte: *mut GtkWidget) -> Option<i32> {
    app_state::with_app(|app| {
        let s = app.sessions.iter().find(|s| s.id == sid && s.has_vte(vte))?;
        Some(s.pid_for_vte(vte))
    })
    .flatten()
}

fn feed_vte_line(vte: *mut GtkWidget, text: &str) {
    if let Ok(c) = CString::new(text) {
        unsafe { vte_terminal_feed_child(vte, c.as_ptr(), text.len() as isize) };
    }
}

fn step_pending_delivery(pending: &mut PendingDelivery) -> bool {
    let Some(pid) = lookup_pane_pid(pending.sid, pending.vte) else {
        return false;
    };
    let visible = read_vte_visible_text(pending.vte);
    let proc_agent = inspect_proc_tree(pid).agent;
    pending.ticks = pending.ticks.saturating_add(1);
    match evaluate_delivery_step(pending.stage, pending.ticks, &visible, &proc_agent, &pending.agent_slug) {
        DeliveryAction::Wait => true,
        DeliveryAction::SendAgentCmd => {
            feed_vte_line(pending.vte, &pending.agent_cmd);
            pending.stage = DeliveryStage::WaitAgent;
            pending.ticks = 0;
            true
        }
        DeliveryAction::SendPromptAndFinish => {
            feed_vte_line(pending.vte, &pending.delivery_prompt);
            pending.stage = DeliveryStage::Done;
            false
        }
        DeliveryAction::GiveUp => false,
    }
}

extern "C" fn on_delivery_tick(data: *mut c_void) -> c_int {
    let pending = unsafe { &mut *(data as *mut PendingDelivery) };
    if step_pending_delivery(pending) {
        return 1;
    }
    unsafe { drop(Box::from_raw(data as *mut PendingDelivery)) };
    0
}

pub fn schedule_agent_delivery(
    sid: usize,
    vte: *mut GtkWidget,
    agent_cmd: String,
    agent_slug: String,
    delivery_prompt: String,
) {
    let boxed = Box::new(PendingDelivery {
        sid,
        vte,
        agent_cmd,
        agent_slug,
        delivery_prompt,
        stage: DeliveryStage::WaitShell,
        ticks: 0,
    });
    let raw = Box::into_raw(boxed) as *mut c_void;
    unsafe { g_timeout_add(DELIVERY_TICK_MS, on_delivery_tick, raw) };
}

fn launch_single_fanout_tab(
    repo_root: &Path,
    profile: &str,
    launch: &AgentLaunchSpec,
    delivery_prompt: &str,
) -> Result<String, String> {
    let (wt_path, branch) = create_task_worktree(repo_root, &launch.agent_slug, &launch.branch_name_slug())?;
    let wt_str = wt_path.to_string_lossy().into_owned();
    let sid = session_ops::open_local_tab(profile.to_owned(), &wt_str);
    let vte = app_state::with_app(|app| {
        app.sessions.iter().find(|s| s.id == sid).map(|s| s.active_vte())
    })
    .flatten()
    .unwrap_or(ptr::null_mut());
    if !vte.is_null() {
        schedule_agent_delivery(sid, vte, launch.agent_cmd.clone(), launch.agent_slug.clone(), delivery_prompt.to_owned());
    }
    Ok(branch)
}

impl AgentLaunchSpec {
    fn branch_name_slug(&self) -> String {
        let prefix = format!("wt/{}-", self.agent_slug);
        self.branch_name.strip_prefix(&prefix).unwrap_or(&self.branch_name).to_owned()
    }
}

pub fn execute_dispatch_in_app(base_cwd: &Path, submit: DispatchSubmit) {
    let locale = current_locale();
    let repo_root = match resolve_repo_root(base_cwd) {
        Ok(root) => root,
        Err(err) => return app_state::set_footer_message(&format!("⚠ {err}")),
    };
    let slug = if submit.slug.is_empty() { derive_branch_slug(&submit.prompt) } else { sanitize_slug(&submit.slug) };
    let Ok(dispatch_path) = write_dispatch_file(&dispatches_dir(), &slug, &submit.prompt, current_unix_secs()) else {
        return app_state::set_footer_message("⚠ Failed to write dispatch brief");
    };
    let req = DispatchRequest {
        prompt: submit.prompt,
        slug,
        agent_indices: submit.agent_indices,
        base_cwd: base_cwd.to_path_buf(),
    };
    match build_fanout_plan(&req, &repo_root, dispatch_path, locale) {
        Ok(plan) => run_fanout_launches(&repo_root, &plan),
        Err(err) => app_state::set_footer_message(&format!("⚠ {err}")),
    }
}

fn run_fanout_launches(repo_root: &Path, plan: &FanoutPlan) {
    let mut created = Vec::new();
    for launch in &plan.launches {
        if let Ok(branch) = launch_single_fanout_tab(repo_root, "shell", launch, &plan.delivery_prompt) {
            created.push(branch);
        }
    }
    if created.is_empty() {
        app_state::set_footer_message("⚠ Failed to spawn worktree tabs");
    } else {
        app_state::set_footer_message(&format!("⎇ Dispatched {}: {}", created.len(), created.join(", ")));
    }
}

struct DispatchModalCtx {
    window: *mut GtkWidget,
    source_combo: *mut GtkWidget,
    source_status: *mut GtkWidget,
    text_buffer: *mut c_void,
    slug_entry: *mut GtkWidget,
    error_label: *mut GtkWidget,
    checks: Vec<*mut GtkWidget>,
    sources: RefCell<Vec<DispatchSourceItem>>,
    slug_manual: RefCell<bool>,
    syncing_slug: RefCell<bool>,
    alive: Arc<AtomicBool>,
    on_submit: Box<dyn Fn(DispatchSubmit)>,
}

#[derive(Clone)]
pub struct TaskDispatchHandle {
    #[cfg(test)]
    ctx: Rc<DispatchModalCtx>,
}

struct GhAsyncPayload {
    ctx_ptr: usize,
    alive: Arc<AtomicBool>,
    result: Result<Vec<GitHubIssue>, String>,
}

fn cstring_lossy(raw: &str) -> CString {
    CString::new(raw.replace('\0', "")).unwrap_or_else(|_| CString::new("").unwrap())
}

fn set_entry_text_str(entry: *mut GtkWidget, val: &str) {
    let c = cstring_lossy(val);
    unsafe { gtk_entry_set_text(entry, c.as_ptr()) };
}

fn read_entry_str(entry: *mut GtkWidget) -> String {
    unsafe {
        let ptr = gtk_entry_get_text(entry);
        if ptr.is_null() { String::new() } else { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
    }
}

fn read_buffer_text(buffer: *mut c_void) -> String {
    let mut start = GtkTextIterBuf::zeroed();
    let mut end = GtkTextIterBuf::zeroed();
    unsafe {
        gtk_text_buffer_get_start_iter(buffer, &mut start);
        gtk_text_buffer_get_end_iter(buffer, &mut end);
        let ptr = gtk_text_buffer_get_text(buffer, &start, &end, 0);
        if ptr.is_null() {
            return String::new();
        }
        let out = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        g_free(ptr as *mut c_void);
        out
    }
}

fn set_buffer_text(buffer: *mut c_void, text: &str) {
    let c = cstring_lossy(text);
    unsafe { gtk_text_buffer_set_text(buffer, c.as_ptr(), -1) };
}

fn make_ui_label(ui: UiText, cls: &str) -> *mut GtkWidget {
    unsafe {
        let lbl = gtk_label_new(ui.as_ptr());
        gtk_label_set_xalign(lbl, 0.0);
        add_class(lbl, cls);
        lbl
    }
}

fn apply_box_margins(widget: *mut GtkWidget, pad: c_int) {
    unsafe {
        gtk_widget_set_margin_start(widget, pad);
        gtk_widget_set_margin_end(widget, pad);
        gtk_widget_set_margin_top(widget, pad);
        gtk_widget_set_margin_bottom(widget, pad);
    }
}

fn build_prompt_editor() -> (*mut GtkWidget, *mut GtkWidget, *mut c_void) {
    unsafe {
        let scroll = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
        gtk_scrolled_window_set_policy(scroll, GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
        gtk_scrolled_window_set_min_content_height(scroll, 140);
        let tv = gtk_text_view_new();
        gtk_text_view_set_wrap_mode(tv, GTK_WRAP_WORD_CHAR);
        gtk_text_view_set_left_margin(tv, 8);
        gtk_text_view_set_right_margin(tv, 8);
        gtk_text_view_set_top_margin(tv, 6);
        gtk_text_view_set_bottom_margin(tv, 6);
        add_class(tv, "dispatch-textview");
        gtk_container_add(scroll, tv);
        let buf = gtk_text_view_get_buffer(tv);
        (scroll, tv, buf)
    }
}

fn build_agent_checkboxes() -> (*mut GtkWidget, Vec<*mut GtkWidget>) {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10);
        let mut checks = Vec::new();
        for (idx, (_, cmd)) in text::AGENT_ACTIONS.iter().enumerate() {
            let slug = cstring_lossy(&agent_slug_from_cmd(cmd));
            let chk = gtk_check_button_new_with_label(slug.as_ptr());
            gtk_toggle_button_set_active(chk, i32::from(idx == 0));
            gtk_box_pack_start(row, chk, 0, 0, 0);
            checks.push(chk);
        }
        (row, checks)
    }
}

fn build_modal_header() -> *mut GtkWidget {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        let title = make_ui_label(TITLE_TEXT, "palette-title");
        let hint = make_ui_label(HINT_TEXT, "palette-hint");
        gtk_box_pack_start(row, title, 1, 1, 0);
        gtk_box_pack_end(row, hint, 0, 0, 0);
        row
    }
}

fn build_action_buttons() -> (*mut GtkWidget, *mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
        let cancel = gtk_button_new_with_label(CANCEL_BTN.as_ptr());
        let launch = gtk_button_new_with_label(LAUNCH_BTN.as_ptr());
        add_class(launch, "palette-row-active");
        gtk_box_pack_end(row, launch, 0, 0, 0);
        gtk_box_pack_end(row, cancel, 0, 0, 0);
        (row, cancel, launch)
    }
}

fn build_modal_window(parent: *mut GtkWidget) -> *mut GtkWidget {
    unsafe {
        let win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        gtk_window_set_title(win, TITLE_TEXT.as_ptr());
        gtk_window_set_default_size(win, 580, 480);
        gtk_window_set_resizable(win, 1);
        add_class(win, "main-bg");
        add_class(win, "dispatch-dialog");
        if !parent.is_null() {
            gtk_window_set_transient_for(win, parent);
            gtk_window_set_modal(win, 1);
            gtk_window_set_position(win, GTK_WIN_POS_CENTER_ON_PARENT);
        }
        win
    }
}

fn build_source_widgets() -> (*mut GtkWidget, *mut GtkWidget) {
    unsafe {
        let combo = gtk_combo_box_text_new();
        gtk_combo_box_text_append_text(combo, SOURCE_NONE.as_ptr());
        gtk_combo_box_set_active(combo, 0);
        let status = make_ui_label(GH_LOADING, "palette-hint");
        (combo, status)
    }
}

fn populate_source_combo(ctx: &DispatchModalCtx, items: Vec<DispatchSourceItem>) {
    unsafe { gtk_combo_box_text_remove_all(ctx.source_combo) };
    unsafe { gtk_combo_box_text_append_text(ctx.source_combo, SOURCE_NONE.as_ptr()) };
    for item in &items {
        let label = format_source_label(item);
        let c = cstring_lossy(&label);
        unsafe { gtk_combo_box_text_append_text(ctx.source_combo, c.as_ptr()) };
    }
    unsafe { gtk_combo_box_set_active(ctx.source_combo, 0) };
    *ctx.sources.borrow_mut() = items;
}

fn format_source_label(item: &DispatchSourceItem) -> String {
    match item {
        DispatchSourceItem::BriefFile(p) => {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("brief.md");
            format!("📄 {name} ({})", shorten_home(&p.display().to_string()))
        }
        DispatchSourceItem::Issue(issue) => format!("🐙 #{} · {}", issue.number, issue.title),
    }
}

fn sync_auto_slug(ctx: &DispatchModalCtx) {
    if *ctx.slug_manual.borrow() {
        return;
    }
    let prompt = read_buffer_text(ctx.text_buffer);
    let derived = if prompt.trim().is_empty() { String::new() } else { derive_branch_slug(&prompt) };
    *ctx.syncing_slug.borrow_mut() = true;
    set_entry_text_str(ctx.slug_entry, &derived);
    *ctx.syncing_slug.borrow_mut() = false;
}

fn selected_agent_indices(ctx: &DispatchModalCtx) -> Vec<usize> {
    ctx.checks
        .iter()
        .enumerate()
        .filter(|(_, chk)| unsafe { gtk_toggle_button_get_active(**chk) } != 0)
        .map(|(idx, _)| idx)
        .collect()
}

fn show_modal_error(ctx: &DispatchModalCtx, msg: UiText) {
    set_label_text(ctx.error_label, msg.as_str());
    unsafe { gtk_widget_set_visible(ctx.error_label, 1) };
}

fn submit_modal(ctx: &DispatchModalCtx) {
    let prompt = read_buffer_text(ctx.text_buffer);
    if prompt.trim().is_empty() {
        return show_modal_error(ctx, ERR_EMPTY_PROMPT);
    }
    let agent_indices = selected_agent_indices(ctx);
    if agent_indices.is_empty() {
        return show_modal_error(ctx, ERR_NO_AGENT);
    }
    let raw_slug = read_entry_str(ctx.slug_entry);
    let slug = if raw_slug.trim().is_empty() { derive_branch_slug(&prompt) } else { sanitize_slug(&raw_slug) };
    close_modal(ctx);
    (ctx.on_submit)(DispatchSubmit { prompt, slug, agent_indices });
}

fn close_modal(ctx: &DispatchModalCtx) {
    if ctx.alive.swap(false, Ordering::SeqCst) && !ctx.window.is_null() {
        unsafe { gtk_widget_destroy(ctx.window) };
    }
}

fn apply_selected_source(ctx: &DispatchModalCtx, active_idx: c_int) {
    if active_idx <= 0 {
        return;
    }
    let item = ctx.sources.borrow().get((active_idx - 1) as usize).cloned();
    let Some(item) = item else { return };
    let text = match item {
        DispatchSourceItem::BriefFile(p) => fs::read_to_string(&p).unwrap_or_default(),
        DispatchSourceItem::Issue(issue) => format_issue_prompt(&issue),
    };
    *ctx.slug_manual.borrow_mut() = false;
    set_buffer_text(ctx.text_buffer, &text);
}

extern "C" fn on_buffer_changed(_buf: *mut c_void, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const DispatchModalCtx) };
    sync_auto_slug(ctx);
}

extern "C" fn on_slug_changed(_entry: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const DispatchModalCtx) };
    if !*ctx.syncing_slug.borrow() {
        *ctx.slug_manual.borrow_mut() = !read_entry_str(ctx.slug_entry).trim().is_empty();
    }
}

extern "C" fn on_source_changed(combo: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const DispatchModalCtx) };
    let idx = unsafe { gtk_combo_box_get_active(combo) };
    apply_selected_source(ctx, idx);
}

extern "C" fn on_cancel_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const DispatchModalCtx) };
    close_modal(ctx);
}

extern "C" fn on_launch_clicked(_btn: *mut GtkWidget, data: *mut c_void) {
    let ctx = unsafe { &*(data as *const DispatchModalCtx) };
    submit_modal(ctx);
}

extern "C" fn on_modal_destroy(_win: *mut GtkWidget, data: *mut c_void) {
    let raw = data as *const DispatchModalCtx;
    unsafe {
        (*raw).alive.store(false, Ordering::SeqCst);
        Rc::decrement_strong_count(raw);
    }
}

extern "C" fn on_modal_key_press(
    _win: *mut GtkWidget,
    event: *const GdkEvent,
    data: *mut c_void,
) -> c_int {
    let (mut keyval, mut state) = (0u32, 0u32);
    unsafe {
        if gdk_event_get_keyval(event, &mut keyval) == 0 {
            return 0;
        }
        gdk_event_get_state(event, &mut state);
    }
    let ctx = unsafe { &*(data as *const DispatchModalCtx) };
    handle_modal_key(ctx, keyval, state)
}

fn handle_modal_key(ctx: &DispatchModalCtx, keyval: c_uint, state: c_uint) -> c_int {
    if keyval == 0xff1b {
        close_modal(ctx);
        return 1;
    }
    if (state & GDK_CONTROL_MASK) != 0 && matches!(keyval, 0xff0d | 0xff8d) {
        submit_modal(ctx);
        return 1;
    }
    0
}

extern "C" fn on_gh_issues_ready(data: *mut c_void) -> c_int {
    let payload = unsafe { Box::from_raw(data as *mut GhAsyncPayload) };
    if !payload.alive.load(Ordering::SeqCst) {
        return 0;
    }
    let ctx = unsafe { &*(payload.ctx_ptr as *const DispatchModalCtx) };
    apply_gh_issues_result(ctx, payload.result);
    0
}

fn apply_gh_issues_result(ctx: &DispatchModalCtx, result: Result<Vec<GitHubIssue>, String>) {
    match result {
        Ok(issues) if !issues.is_empty() => {
            let mut items = ctx.sources.borrow().clone();
            items.extend(issues.into_iter().map(DispatchSourceItem::Issue));
            populate_source_combo(ctx, items);
            set_label_text(ctx.source_status, GH_LOADED.as_str());
        }
        Ok(_) | Err(_) => set_label_text(ctx.source_status, GH_OFFLINE.as_str()),
    }
}

fn spawn_gh_issue_loader(ctx: &Rc<DispatchModalCtx>, base_cwd: PathBuf) {
    let alive = Arc::clone(&ctx.alive);
    let ctx_ptr = Rc::as_ptr(ctx) as usize;
    thread::spawn(move || {
        let result = fetch_gh_issues(&base_cwd);
        let payload = Box::new(GhAsyncPayload { ctx_ptr, alive, result });
        unsafe { g_idle_add(on_gh_issues_ready, Box::into_raw(payload) as *mut c_void) };
    });
}

fn wire_modal_signals(ctx: &Rc<DispatchModalCtx>, cancel_btn: *mut GtkWidget, launch_btn: *mut GtkWidget) {
    let ptr = Rc::as_ptr(ctx) as *mut c_void;
    unsafe { Rc::increment_strong_count(Rc::as_ptr(ctx)) };
    unsafe {
        g_signal_connect_data(ctx.window, c"destroy".as_ptr(), on_modal_destroy as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(ctx.window, c"key-press-event".as_ptr(), on_modal_key_press as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(ctx.text_buffer, c"changed".as_ptr(), on_buffer_changed as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(ctx.slug_entry, c"changed".as_ptr(), on_slug_changed as *const c_void, ptr, ptr::null(), 0);
        g_signal_connect_data(ctx.source_combo, c"changed".as_ptr(), on_source_changed as *const c_void, ptr, ptr::null(), 0);
    }
    app_state::connect_click(cancel_btn, on_cancel_clicked, ptr);
    app_state::connect_click(launch_btn, on_launch_clicked, ptr);
}

fn pack_modal_layout(
    vbox: *mut GtkWidget,
    source_combo: *mut GtkWidget,
    source_status: *mut GtkWidget,
    scroll: *mut GtkWidget,
    slug_entry: *mut GtkWidget,
    agents_row: *mut GtkWidget,
    error_label: *mut GtkWidget,
    actions_row: *mut GtkWidget,
) {
    unsafe {
        gtk_box_pack_start(vbox, build_modal_header(), 0, 1, 0);
        gtk_box_pack_start(vbox, make_ui_label(SOURCE_HEADER, "section-header"), 0, 1, 0);
        gtk_box_pack_start(vbox, source_combo, 0, 1, 0);
        gtk_box_pack_start(vbox, source_status, 0, 1, 0);
        gtk_box_pack_start(vbox, make_ui_label(PROMPT_HEADER, "section-header"), 0, 1, 0);
        gtk_box_pack_start(vbox, scroll, 1, 1, 0);
        gtk_box_pack_start(vbox, make_ui_label(SLUG_HEADER, "section-header"), 0, 1, 0);
        gtk_box_pack_start(vbox, slug_entry, 0, 1, 0);
        gtk_box_pack_start(vbox, make_ui_label(AGENTS_HEADER, "section-header"), 0, 1, 0);
        gtk_box_pack_start(vbox, agents_row, 0, 1, 0);
        gtk_box_pack_start(vbox, error_label, 0, 1, 0);
        gtk_box_pack_end(vbox, actions_row, 0, 1, 0);
    }
}

fn build_slug_entry() -> *mut GtkWidget {
    unsafe {
        let entry = gtk_entry_new();
        add_class(entry, "search-entry");
        gtk_entry_set_placeholder_text(entry, SLUG_PLACEHOLDER.as_ptr());
        entry
    }
}

fn build_error_label() -> *mut GtkWidget {
    let lbl = make_ui_label(ERR_EMPTY_PROMPT, "term-search-status-err");
    unsafe { gtk_widget_set_no_show_all(lbl, 1) };
    lbl
}

pub fn open_task_dispatch_modal(
    parent: *mut GtkWidget,
    base_cwd: PathBuf,
    briefs_dir: Option<PathBuf>,
    on_submit: impl Fn(DispatchSubmit) + 'static,
) -> TaskDispatchHandle {
    let window = build_modal_window(parent);
    let vbox = unsafe { gtk_box_new(GTK_ORIENTATION_VERTICAL, 6) };
    apply_box_margins(vbox, 14);
    let (source_combo, source_status) = build_source_widgets();
    let (scroll, text_view, text_buffer) = build_prompt_editor();
    let slug_entry = build_slug_entry();
    let (agents_row, checks) = build_agent_checkboxes();
    let error_label = build_error_label();
    let (actions_row, cancel_btn, launch_btn) = build_action_buttons();
    pack_modal_layout(vbox, source_combo, source_status, scroll, slug_entry, agents_row, error_label, actions_row);
    unsafe {
        gtk_container_add(window, vbox);
        gtk_widget_show_all(window);
        gtk_widget_grab_focus(text_view);
    }
    let ctx = Rc::new(DispatchModalCtx {
        window, source_combo, source_status, text_buffer, slug_entry, error_label,
        checks, sources: RefCell::new(Vec::new()), slug_manual: RefCell::new(false),
        syncing_slug: RefCell::new(false), alive: Arc::new(AtomicBool::new(true)),
        on_submit: Box::new(on_submit),
    });
    let briefs: Vec<DispatchSourceItem> = discover_handoff_files(briefs_dir.as_deref(), Some(&state_dir().join("captures")))
        .into_iter().map(DispatchSourceItem::BriefFile).collect();
    populate_source_combo(&ctx, briefs);
    wire_modal_signals(&ctx, cancel_btn, launch_btn);
    spawn_gh_issue_loader(&ctx, base_cwd);
    #[cfg(test)]
    return TaskDispatchHandle { ctx };
    #[cfg(not(test))]
    TaskDispatchHandle {}
}

pub fn open_for_app() {
    let info = app_state::with_app(|app| {
        let cwd = app
            .sessions
            .iter()
            .find(|s| s.id == app.active_id)
            .and_then(|s| s.active_local_cwd())
            .unwrap_or_else(|| crate::paths::home_dir());
        (app.win, PathBuf::from(cwd), app.config.briefs_dir.clone())
    });
    let Some((win, cwd, briefs_dir)) = info else {
        return;
    };
    let cwd_submit = cwd.clone();
    open_task_dispatch_modal(win, cwd, briefs_dir, move |submit| {
        execute_dispatch_in_app(&cwd_submit, submit);
    });
}

#[cfg(test)]
impl TaskDispatchHandle {
    pub fn set_prompt_for_test(&self, text: &str) {
        set_buffer_text(self.ctx.text_buffer, text);
    }

    pub fn slug_text_for_test(&self) -> String {
        read_entry_str(self.ctx.slug_entry)
    }

    pub fn set_agent_active_for_test(&self, idx: usize, active: bool) {
        if let Some(&chk) = self.ctx.checks.get(idx) {
            unsafe { gtk_toggle_button_set_active(chk, i32::from(active)) };
        }
    }

    pub fn trigger_submit_for_test(&self) {
        submit_modal(&self.ctx);
    }

    pub fn is_alive_for_test(&self) -> bool {
        self.ctx.alive.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("akhterm_td_{tag}_{}", next_wt_suffix()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn derives_ascii_slug_from_english_and_cyrillic_prompts() {
        assert_eq!(derive_branch_slug("# Fix Auth Race Condition\nDetails"), "fix-auth-race-condition");
        assert_eq!(derive_branch_slug("Починить гонку в авторизации"), "pochinit-gonku-v-avtorizatsii");
        assert_eq!(derive_branch_slug("   !!!   "), "task");
        assert_eq!(format_worktree_branch("claude", "fix-auth"), "wt/claude-fix-auth");
    }

    #[test]
    fn recognizes_ctrl_shift_n_shortcut() {
        let cs = GDK_CONTROL_MASK | GDK_SHIFT_MASK;
        assert!(is_dispatch_shortcut(b'N' as u32, cs, 0));
        assert!(is_dispatch_shortcut(0x6d4, cs, KEYCODE_N));
        assert!(!is_dispatch_shortcut(b'n' as u32, GDK_CONTROL_MASK, KEYCODE_N));
        assert!(!is_dispatch_shortcut(b'N' as u32, cs | GDK_MOD1_MASK, KEYCODE_N));
    }

    #[test]
    fn parses_gh_issue_list_and_builds_bilingual_delivery_prompts() {
        let raw = "42\tOPEN\tFix auth race\tbug, p0\t2026-11-12\n7\tOPEN\tAdd tests\t\t2026-11-11\n";
        let issues = parse_gh_issue_list(raw);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].number, 42);
        assert_eq!(issues[0].title, "Fix auth race");
        assert_eq!(format_delivery_prompt(Path::new("/tmp/t.md"), UiLocale::Ru), "Прочитай файл /tmp/t.md и действуй по нему.\n");
        assert_eq!(format_delivery_prompt(Path::new("/tmp/t.md"), UiLocale::En), "Read file /tmp/t.md and execute the task.\n");
    }

    #[test]
    fn writes_dispatch_file_atomically_with_0600_mode_and_builds_fanout_plan() {
        let tmp = TempDir::new("fanout");
        let path = write_dispatch_file(&tmp.0, "fix-auth", "Resolve race in login", 1700).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let req = DispatchRequest {
            prompt: "Resolve race in login".into(),
            slug: "fix-auth".into(),
            agent_indices: vec![1, 2],
            base_cwd: tmp.0.clone(),
        };
        let plan = build_fanout_plan(&req, &tmp.0.join("akhterm"), path, UiLocale::En).unwrap();
        assert_eq!(plan.launches.len(), 2);
        assert_eq!(plan.launches[0].branch_name, "wt/codex-fix-auth");
        assert_eq!(plan.launches[1].branch_name, "wt/claude-fix-auth");
    }

    #[test]
    fn evaluates_delivery_readiness_stages_deterministically() {
        assert_eq!(evaluate_delivery_step(DeliveryStage::WaitShell, 1, "$ ", "", "claude"), DeliveryAction::Wait);
        assert_eq!(evaluate_delivery_step(DeliveryStage::WaitShell, 2, "$ ", "", "claude"), DeliveryAction::SendAgentCmd);
        assert_eq!(evaluate_delivery_step(DeliveryStage::WaitAgent, 3, "> ", "claude", "claude"), DeliveryAction::Wait);
        assert_eq!(evaluate_delivery_step(DeliveryStage::WaitAgent, 4, "> ", "claude", "claude"), DeliveryAction::SendPromptAndFinish);
        assert_eq!(evaluate_delivery_step(DeliveryStage::WaitAgent, 14, "", "", "claude"), DeliveryAction::SendPromptAndFinish);
    }

    #[test]
    fn headless_broadway_task_dispatch_modal_auto_slug_and_fanout_submit() {
        if std::env::var_os("AKHTERM_DISPATCH_BWAY_CHILD").is_some() {
            run_broadway_dispatch_checks();
            return;
        }
        let exe = std::env::current_exe().expect("test exe");
        let st = Command::new(exe)
            .args(["--exact", "task_dispatch::tests::headless_broadway_task_dispatch_modal_auto_slug_and_fanout_submit"])
            .env("AKHTERM_DISPATCH_BWAY_CHILD", "1")
            .status()
            .expect("spawn dispatch broadway test");
        assert!(st.success());
    }

    fn run_broadway_dispatch_checks() {
        let dir = TempDir::new("bway");
        let Some(mut child) = spawn_dispatch_broadway(&dir.0) else { return };
        let submitted: Rc<RefCell<Option<DispatchSubmit>>> = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&submitted);
        let parent = unsafe { gtk_window_new(GTK_WINDOW_TOPLEVEL) };
        let handle = open_task_dispatch_modal(parent, dir.0.clone(), None, move |s| *sink.borrow_mut() = Some(s));
        handle.set_prompt_for_test("Fix worktree merge conflict");
        pump_dispatch_events();
        assert_eq!(handle.slug_text_for_test(), "fix-worktree-merge-conflict");
        handle.set_agent_active_for_test(2, true);
        handle.trigger_submit_for_test();
        pump_dispatch_events();
        let got = submitted.borrow().clone().expect("submitted");
        assert_eq!(got.agent_indices, vec![0, 2]);
        assert!(!handle.is_alive_for_test());
        unsafe { gtk_widget_destroy(parent) };
        let _ = child.kill();
        let _ = child.wait();
    }

    fn pump_dispatch_events() {
        for _ in 0..50 {
            if unsafe { gtk_events_pending() } == 0 {
                break;
            }
            unsafe { gtk_main_iteration_do(0) };
        }
    }

    fn spawn_dispatch_broadway(dir: &Path) -> Option<std::process::Child> {
        let sock = dir.join("http.sock");
        let child = Command::new("broadwayd")
            .args(["-u", &sock.to_string_lossy(), ":18"])
            .env("XDG_RUNTIME_DIR", dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let gdk_sock = dir.join("broadway19.socket");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !gdk_sock.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        std::env::set_var("XDG_RUNTIME_DIR", dir);
        std::env::set_var("GDK_BACKEND", "broadway");
        std::env::set_var("BROADWAY_DISPLAY", ":18");
        (unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } != 0).then_some(child)
    }
}
