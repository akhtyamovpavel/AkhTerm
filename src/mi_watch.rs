//! On-demand Maintainability Index project root discovery, TTL scheduler, and UI formatting.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::mi::{analyze_project_mi, FileMetrics, ProjectMiReport};

pub const DEFAULT_MI_TTL: Duration = Duration::from_secs(25);

const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "pyproject.toml",
    "package.json",
    "go.mod",
    "CMakeLists.txt",
    "Makefile",
];

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

fn canonical_dir(path: &str, home: &str) -> Option<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    fs::canonicalize(expand_with_home(trimmed, home))
        .ok()
        .filter(|dir| dir.is_dir())
}

fn is_excluded_dir(dir: &Path, home: &Path) -> bool {
    dir.as_os_str().is_empty()
        || dir == Path::new("/")
        || (!home.as_os_str().is_empty() && home.starts_with(dir))
}

fn has_project_marker(dir: &Path) -> bool {
    PROJECT_MARKERS.iter().any(|marker| dir.join(marker).exists())
}

pub fn resolve_project_root(cwd: &str, home: &str) -> Option<String> {
    let home_path = clean_home(home);
    let start = canonical_dir(cwd, home)?;
    let mut current: &Path = &start;
    for _ in 0..=3 {
        if is_excluded_dir(current, &home_path) {
            return None;
        }
        if has_project_marker(current) {
            return Some(current.to_string_lossy().into_owned());
        }
        current = current.parent()?;
    }
    None
}

fn resolve_configured_dir(dir: &str, home: &str) -> Option<String> {
    if let Some(root) = resolve_project_root(dir, home) {
        return Some(root);
    }
    let home_path = clean_home(home);
    let clean = canonical_dir(dir, home)?;
    (!is_excluded_dir(&clean, &home_path)).then(|| clean.to_string_lossy().into_owned())
}

fn push_unique(out: &mut Vec<String>, item: String) {
    if !out.contains(&item) {
        out.push(item);
    }
}

fn collect_targets(configured: &[String], local_cwds: &[String], home: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for cwd in local_cwds {
        if let Some(root) = resolve_project_root(cwd, home) {
            push_unique(&mut targets, root);
        }
    }
    for dir in configured {
        if let Some(root) = resolve_configured_dir(dir, home) {
            push_unique(&mut targets, root);
        }
    }
    targets
}

pub fn lookup_mi<'a>(
    map: &'a HashMap<String, ProjectMiReport>,
    cwd: &str,
    home: &str,
) -> Option<&'a ProjectMiReport> {
    if let Some(root) = resolve_project_root(cwd, home) {
        if let Some(rep) = map.get(&root) {
            return Some(rep);
        }
    }
    resolve_configured_dir(cwd, home).and_then(|clean| map.get(&clean))
}

pub fn resolve_active_mi<'a>(
    map: &'a HashMap<String, ProjectMiReport>,
    active_cwd: Option<&str>,
    mi_root: Option<&str>,
    home: &str,
) -> (Option<&'a ProjectMiReport>, Option<&'a ProjectMiReport>) {
    let self_mi = mi_root.and_then(|path| lookup_mi(map, path, home));
    let cur_mi = active_cwd
        .and_then(|dir| lookup_mi(map, dir, home))
        .or(self_mi);
    (cur_mi, self_mi)
}

struct SchedulerState {
    entries: HashMap<String, (Instant, ProjectMiReport)>,
    targets: Vec<String>,
    ttl: Duration,
}

impl SchedulerState {
    fn is_due(&self, root: &str, now: Instant) -> bool {
        match self.entries.get(root) {
            Some((ts, _)) => now.saturating_duration_since(*ts) >= self.ttl,
            None => true,
        }
    }

    fn due_roots(&self, now: Instant) -> Vec<String> {
        self.targets
            .iter()
            .filter(|root| self.is_due(root, now))
            .cloned()
            .collect()
    }
}

#[derive(Clone)]
pub struct MiScheduler {
    shared: Arc<(Mutex<SchedulerState>, Condvar)>,
}

impl Default for MiScheduler {
    fn default() -> Self {
        Self::new(DEFAULT_MI_TTL)
    }
}

impl MiScheduler {
    pub fn new(ttl: Duration) -> Self {
        let state = SchedulerState {
            entries: HashMap::new(),
            targets: Vec::new(),
            ttl,
        };
        Self {
            shared: Arc::new((Mutex::new(state), Condvar::new())),
        }
    }

    pub fn sync_targets(&self, configured: &[String], local_cwds: &[String], home: &str) {
        let next = collect_targets(configured, local_cwds, home);
        let (lock, cvar) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            state.targets = next;
            if !state.due_roots(Instant::now()).is_empty() {
                cvar.notify_one();
            }
        }
    }

    fn take_due(&self, now: Instant) -> Vec<String> {
        let (lock, _) = &*self.shared;
        lock.lock()
            .map(|state| state.due_roots(now))
            .unwrap_or_default()
    }

    fn store_batch(&self, now: Instant, batch: Vec<(String, ProjectMiReport)>) {
        let (lock, _) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            for (root, report) in batch {
                state.entries.insert(root, (now, report));
            }
        }
    }

    pub fn refresh_due(&self, now: Instant) -> usize {
        self.refresh_due_with(now, analyze_project_mi)
    }

    pub fn refresh_due_with<F>(&self, now: Instant, mut analyze: F) -> usize
    where
        F: FnMut(&str) -> ProjectMiReport,
    {
        let due = self.take_due(now);
        if due.is_empty() {
            return 0;
        }
        let batch: Vec<_> = due.into_iter().map(|r| (r.clone(), analyze(&r))).collect();
        let count = batch.len();
        self.store_batch(now, batch);
        count
    }

    pub fn snapshot(&self) -> HashMap<String, ProjectMiReport> {
        let (lock, _) = &*self.shared;
        let Ok(state) = lock.lock() else {
            return HashMap::new();
        };
        state
            .entries
            .iter()
            .map(|(k, (_, rep))| (k.clone(), rep.clone()))
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

fn format_worst_metric(file: &FileMetrics) -> String {
    format!(
        "{} (MI {:.1}, SLOC {}, CC {})",
        file.rel_path, file.mi, file.sloc, file.cc
    )
}

pub fn build_mi_refactor_prompt(rep: &ProjectMiReport) -> String {
    if rep.worst_files.is_empty() {
        return format!(
            "Проведи рефакторинг проекта {} (текущий MI {:.1}) до MI >= 75, сохранив прохождение всех тестов.",
            rep.root, rep.avg_mi
        );
    }
    let items: Vec<String> = rep.worst_files.iter().map(format_worst_metric).collect();
    format!(
        "Проведи рефакторинг файлов с низким Maintainability Index в {}: {}. Доведи их MI >= 75 (разбей крупные функции и снизь цикломатическую сложность), сохранив поведение и прохождение всех тестов.",
        rep.root,
        items.join(", ")
    )
}

pub fn mi_grade_class(grade: &str) -> &'static str {
    match grade {
        "A" => "btn-mi-a",
        "B" => "btn-mi-b",
        _ => "btn-mi-c",
    }
}

pub fn format_mi_button_label(rep: &ProjectMiReport) -> String {
    format!("MI {:.1} [{}] ▾", rep.avg_mi, rep.grade)
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn format_worst_markup_line(file: &FileMetrics) -> String {
    format!(
        "  • <tt>{}</tt> — <b>MI {:.1}</b> (SLOC: {}, CC: {})",
        escape_markup(&file.rel_path),
        file.mi,
        file.sloc,
        file.cc
    )
}

fn format_header_lines(rep: &ProjectMiReport) -> Vec<String> {
    vec![
        format!(
            "<b>ИНДЕКС ПОДДЕРЖИВАЕМОСТИ (Maintainability Index)</b>\n<span foreground='#a1a1aa'>Каталог: {}</span>",
            escape_markup(&rep.root)
        ),
        format!(
            "Средний MI (по функциям): <b>{:.1} / 100 (Класс {})</b>  ·  Файлов: {}  ·  SLOC: {}  ·  Средняя CC: {:.1}",
            rep.avg_mi, rep.grade, rep.files_count, rep.total_sloc, rep.avg_cc
        ),
    ]
}

pub fn format_mi_popover_markup(
    rep: &ProjectMiReport,
    self_mi: Option<&ProjectMiReport>,
) -> String {
    let mut lines = format_header_lines(rep);
    if !rep.worst_files.is_empty() {
        lines.push("\n<b>Точки внимания (файлы с минимальным MI):</b>".to_string());
        lines.extend(rep.worst_files.iter().map(format_worst_markup_line));
    }
    if let Some(ar) = self_mi {
        lines.push(format!(
            "\n<span foreground='#c4b5fd'>Ядро AkhTerm (Rust · VTE-2.91): <b>MI {:.1} [{}]</b> ({} файлов, {} SLOC)</span>",
            ar.avg_mi, ar.grade, ar.files_count, ar.total_sloc
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("akhterm_mi_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_file(path: &str, sloc: usize, cc: usize, mi: f64) -> FileMetrics {
        FileMetrics {
            rel_path: path.to_string(),
            sloc,
            cc,
            mi,
        }
    }

    fn sample_report(root: &str, avg_mi: f64) -> ProjectMiReport {
        ProjectMiReport {
            root: root.to_string(),
            avg_mi,
            grade: "C",
            files_count: 2,
            total_sloc: 320,
            avg_cc: 5.0,
            worst_files: vec![
                sample_file("src/main.rs", 240, 7, 54.8),
                sample_file("src/mi.rs", 80, 3, 61.2),
            ],
        }
    }

    #[test]
    fn resolves_project_root_up_to_three_parents() {
        let base = temp_sandbox("root_walk");
        let home = base.join("home_user");
        let repo = home.join("Проекты").join("demo_repo");
        let deep_ok = repo.join("src").join("nested").join("leaf");
        let too_deep = deep_ok.join("extra");
        fs::create_dir_all(&too_deep).unwrap();
        fs::write(repo.join("Cargo.toml"), "[package]\n").unwrap();

        let home_s = home.to_string_lossy().into_owned();
        let expected = fs::canonicalize(&repo).unwrap().to_string_lossy().into_owned();
        assert_eq!(resolve_project_root(&repo.join("src").to_string_lossy(), &home_s).as_deref(), Some(expected.as_str()));
        assert_eq!(resolve_project_root(&deep_ok.to_string_lossy(), &home_s).as_deref(), Some(expected.as_str()));
        assert_eq!(resolve_project_root(&too_deep.to_string_lossy(), &home_s), None);
        assert_eq!(resolve_project_root("~/Проекты/demo_repo/src", &home_s).as_deref(), Some(expected.as_str()));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn recognizes_all_supported_project_markers() {
        let base = temp_sandbox("markers");
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        let home_s = home.to_string_lossy().into_owned();

        for (idx, marker) in PROJECT_MARKERS.iter().enumerate() {
            let proj = home.join(format!("p_{idx}"));
            let sub = proj.join("sub");
            fs::create_dir_all(&sub).unwrap();
            fs::write(proj.join(marker), "marker\n").unwrap();
            let expected = fs::canonicalize(&proj).unwrap().to_string_lossy().into_owned();
            assert_eq!(resolve_project_root(&sub.to_string_lossy(), &home_s).as_deref(), Some(expected.as_str()));
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn excludes_home_and_root_even_if_markers_present() {
        let base = temp_sandbox("excl_home");
        let home = base.join("home_dir");
        let plain_sub = home.join("downloads").join("untracked");
        fs::create_dir_all(home.join(".git")).unwrap();
        fs::write(home.join("Makefile"), "all:\n").unwrap();
        fs::create_dir_all(&plain_sub).unwrap();

        let home_s = home.to_string_lossy().into_owned();
        assert_eq!(resolve_project_root(&home_s, &home_s), None);
        assert_eq!(resolve_project_root("~", &home_s), None);
        assert_eq!(resolve_project_root("/", &home_s), None);
        assert_eq!(resolve_project_root(&base.to_string_lossy(), &home_s), None);
        assert_eq!(resolve_project_root(&plain_sub.to_string_lossy(), &home_s), None);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn scheduler_respects_ttl_and_computes_on_cd() {
        let base = temp_sandbox("sched_ttl");
        let home = base.join("home");
        let repo_a = home.join("repo_a");
        let repo_b_src = home.join("repo_b").join("src");
        fs::create_dir_all(&repo_a).unwrap();
        fs::create_dir_all(&repo_b_src).unwrap();
        fs::write(repo_a.join("go.mod"), "module a\n").unwrap();
        fs::write(home.join("repo_b").join("CMakeLists.txt"), "project(b)\n").unwrap();

        let home_s = home.to_string_lossy().into_owned();
        let a_s = repo_a.to_string_lossy().into_owned();
        let b_src = repo_b_src.to_string_lossy().into_owned();
        let sched = MiScheduler::new(Duration::from_secs(25));
        let t0 = Instant::now();

        sched.sync_targets(std::slice::from_ref(&a_s), &[], &home_s);
        assert_eq!(sched.refresh_due_with(t0, |r| sample_report(r, 72.0)), 1);
        assert_eq!(sched.refresh_due_with(t0 + Duration::from_secs(10), |r| sample_report(r, 99.0)), 0);

        sched.sync_targets(std::slice::from_ref(&a_s), std::slice::from_ref(&b_src), &home_s);
        assert_eq!(sched.refresh_due_with(t0 + Duration::from_secs(12), |r| sample_report(r, 64.5)), 1);
        assert_eq!(lookup_mi(&sched.snapshot(), &b_src, &home_s).map(|r| r.avg_mi), Some(64.5));
        assert_eq!(sched.refresh_due_with(t0 + Duration::from_secs(26), |r| sample_report(r, 74.0)), 1);
        assert_eq!(lookup_mi(&sched.snapshot(), &a_s, &home_s).map(|r| r.avg_mi), Some(74.0));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn builds_concise_refactor_prompt_and_safe_markup() {
        let rep = sample_report("/work/R&D/akhterm", 58.0);
        let prompt = build_mi_refactor_prompt(&rep);
        assert!(!prompt.contains('\n'));
        assert!(prompt.contains("src/main.rs (MI 54.8, SLOC 240, CC 7)"));
        assert!(prompt.contains("src/mi.rs (MI 61.2, SLOC 80, CC 3)"));
        assert!(prompt.contains("MI >= 75"));
        assert!(prompt.contains("тест"));

        let mut empty_rep = rep.clone();
        empty_rep.worst_files.clear();
        assert!(build_mi_refactor_prompt(&empty_rep).contains("MI >= 75"));

        let markup = format_mi_popover_markup(&rep, Some(&rep));
        assert!(markup.contains("/work/R&amp;D/akhterm"));
        assert!(markup.contains("Класс C"));
        assert_eq!(mi_grade_class("A"), "btn-mi-a");
        assert_eq!(mi_grade_class("B"), "btn-mi-b");
        assert_eq!(mi_grade_class("C"), "btn-mi-c");
    }
}
