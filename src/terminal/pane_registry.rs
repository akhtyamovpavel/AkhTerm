//! Canonical per-pane registry, state model, and priority aggregation.

use std::collections::HashMap;
#[cfg(test)]
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::ctl_protocol::HookState;
use crate::ffi::GtkWidget;
use crate::proc_radar::ProcInfo;
use crate::session::{detect_agent_name, WorkState};
use crate::telemetry::SessionTelemetry;

const HOOK_WORKING_TTL: Duration = Duration::from_secs(900);
const HOOK_WAITING_TTL: Duration = Duration::from_secs(1800);
const HOOK_DONE_TTL: Duration = Duration::from_secs(120);
const HOOK_ERROR_TTL: Duration = Duration::from_secs(1800);
const HOOK_OUTPUT_GRACE: Duration = Duration::from_secs(3);

static NEXT_GLOBAL_PANE: AtomicU64 = AtomicU64::new(1);
static SPAWNED_PANE_IDS: OnceLock<Mutex<HashMap<usize, PaneId>>> = OnceLock::new();

pub fn allocate_global_pane_id() -> PaneId {
    PaneId::new(NEXT_GLOBAL_PANE.fetch_add(1, Ordering::Relaxed))
}

pub fn remember_spawned_pane_id(vte: *mut GtkWidget, id: PaneId) {
    if vte.is_null() {
        return;
    }
    let map = SPAWNED_PANE_IDS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut guard) = map.lock() {
        guard.insert(vte as usize, id);
    }
}

pub fn take_spawned_pane_id(vte: *mut GtkWidget) -> Option<PaneId> {
    if vte.is_null() {
        return None;
    }
    SPAWNED_PANE_IDS.get()?.lock().ok()?.remove(&(vte as usize))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneHookState {
    pub state: HookState,
    pub title: Option<String>,
    pub message: Option<String>,
    pub updated_at: Instant,
    pub expires_at: Instant,
    pub saw_non_shell: bool,
}

impl PaneHookState {
    pub fn ttl_for(state: HookState) -> Duration {
        match state {
            HookState::Working => HOOK_WORKING_TTL,
            HookState::Waiting => HOOK_WAITING_TTL,
            HookState::Done => HOOK_DONE_TTL,
            HookState::Error => HOOK_ERROR_TTL,
        }
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u64);

impl PaneId {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

pub struct PaneRecord {
    pub id: PaneId,
    pub probe_id: usize,
    pub ssh_directory: Option<(i32, String)>,
    pub tmux_server: Option<u32>,
    pub tmux_pane: Option<String>,
    pub vte: *mut GtkWidget,
    #[cfg(test)]
    pub header_box: *mut GtkWidget,
    pub child_pid: i32,
    pub cwd: String,
    pub proc_info: ProcInfo,
    pub state: WorkState,
    pub unread: bool,
    pub last_output: Instant,
    pub work_started: Instant,
    pub work_started_at: Option<Instant>,
    pub last_turn_secs: Option<u64>,
    pub last_notified: Option<Instant>,
    pub osc_title: String,
    pub remote_tmux: Option<(String, String, String, String)>,
    pub approval_snippet: Option<String>,
    pub telemetry: SessionTelemetry,
    pub last_exit_code: Option<i32>,
    pub hook_state: Option<PaneHookState>,
    pub cmd_marks: crate::cmd_marks::CmdMarkHistory,
}

impl PaneRecord {
    pub fn new(
        id: PaneId,
        vte: *mut GtkWidget,
        scroll: *mut GtkWidget,
        child_pid: i32,
        cwd: String,
    ) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT_PROBE: AtomicUsize = AtomicUsize::new(1);
        let now = Instant::now();
        let _ = scroll;
        Self {
            id,
            probe_id: NEXT_PROBE.fetch_add(1, Ordering::Relaxed),
            ssh_directory: None,
            tmux_server: None,
            tmux_pane: None,
            vte,
            #[cfg(test)]
            header_box: ptr::null_mut(),
            child_pid,
            cwd,
            proc_info: ProcInfo::default(),
            state: WorkState::Idle,
            unread: false,
            last_output: now,
            work_started: now,
            work_started_at: None,
            last_turn_secs: None,
            last_notified: None,
            osc_title: String::new(),
            remote_tmux: None,
            approval_snippet: None,
            telemetry: SessionTelemetry::default(),
            last_exit_code: None,
            hook_state: None,
            cmd_marks: crate::cmd_marks::CmdMarkHistory::default(),
        }
    }

    #[cfg(test)]
    pub fn with_header(mut self, header_box: *mut GtkWidget) -> Self {
        self.header_box = header_box;
        self
    }

    pub fn sync_connection(&mut self, next: &ProcInfo) {
        if self.proc_info.ssh_host != next.ssh_host || self.proc_info.ssh_pid != next.ssh_pid {
            self.remote_tmux = None;
            self.tmux_pane = None;
            self.tmux_server = None;
            self.ssh_directory = self.ssh_directory.take().filter(|(pid, _)| *pid == next.ssh_pid);
            self.telemetry.reset_transport(self.vte);
            self.osc_title.clear();
            self.cmd_marks.clear();
        }
    }

    pub fn set_state(&mut self, next: WorkState, now: Instant) {
        if next == WorkState::Working && self.work_started_at.is_none() {
            self.work_started = now;
            self.work_started_at = Some(now);
        }
        if next != WorkState::Working {
            if let Some(started) = self.work_started_at.take() {
                self.last_turn_secs = Some(now.saturating_duration_since(started).as_secs());
            }
        }
        if next != WorkState::NeedsApproval {
            self.approval_snippet = None;
        }
        self.state = next;
    }

    pub fn mark_output(&mut self, now: Instant, is_focused_active: bool) {
        self.last_output = now;
        if let Some(hook) = &self.hook_state {
            if !hook.is_expired(now) && hook.state != HookState::Working {
                if now.saturating_duration_since(hook.updated_at) < HOOK_OUTPUT_GRACE {
                    return;
                }
                if hook.state == HookState::Done {
                    self.hook_state = None;
                } else {
                    return;
                }
            }
        }
        if self.state != WorkState::Working {
            self.set_state(WorkState::Working, now);
        }
        if is_focused_active {
            self.unread = false;
        }
    }

    pub fn apply_hook_state(
        &mut self,
        state: HookState,
        title: Option<String>,
        message: Option<String>,
        is_focused: bool,
        now: Instant,
    ) {
        let saw_non_shell = !self.proc_info.shell_owns_pane(self.remote_tmux.as_ref());
        let ttl = PaneHookState::ttl_for(state);
        self.apply_hook_work_state(state, &title, &message, is_focused, now);
        self.hook_state = Some(PaneHookState {
            state,
            title,
            message,
            updated_at: now,
            expires_at: now + ttl,
            saw_non_shell,
        });
    }

    fn apply_hook_work_state(
        &mut self,
        state: HookState,
        title: &Option<String>,
        message: &Option<String>,
        is_focused: bool,
        now: Instant,
    ) {
        match state {
            HookState::Working => {
                self.set_state(WorkState::Working, now);
                self.unread = false;
                self.last_exit_code = None;
            }
            HookState::Waiting => {
                let snippet = message.clone().or_else(|| title.clone());
                self.set_approval(snippet, !is_focused, now);
            }
            HookState::Done => {
                self.set_state(WorkState::Ready, now);
                self.unread = !is_focused;
                self.last_exit_code = Some(0);
            }
            HookState::Error => {
                let snippet = message.clone().or_else(|| title.clone());
                self.set_approval(snippet, !is_focused, now);
                self.last_exit_code = Some(1);
            }
        }
    }

    pub fn expire_hook_state(&mut self, now: Instant) -> bool {
        let Some(hook) = &self.hook_state else {
            return false;
        };
        if !hook.is_expired(now) {
            return false;
        }
        let prev = hook.state;
        self.hook_state = None;
        if prev == HookState::Working && self.state == WorkState::Working {
            self.set_state(WorkState::Idle, now);
        }
        true
    }

    pub fn active_hook_state(&self, now: Instant) -> Option<&PaneHookState> {
        self.hook_state.as_ref().filter(|h| !h.is_expired(now))
    }

    pub fn has_hook_error(&self) -> bool {
        self.hook_state.as_ref().is_some_and(|h| h.state == HookState::Error)
    }

    pub fn clear_hook_state(&mut self) {
        self.hook_state = None;
    }

    pub fn set_approval(&mut self, snippet: Option<String>, unread: bool, now: Instant) {
        self.set_state(WorkState::NeedsApproval, now);
        self.approval_snippet = snippet;
        self.unread = unread;
    }

    pub fn record_exit(&mut self, code: i32) {
        self.last_exit_code = Some(code);
    }

    pub fn live_turn_secs(&self, now: Instant) -> Option<u64> {
        if self.state != WorkState::Working || self.child_pid <= 0 {
            return None;
        }
        self.work_started_at
            .map(|started| now.saturating_duration_since(started).as_secs())
    }

    pub fn sync_agent(&mut self, info: &mut ProcInfo) {
        if info.shell_owns_pane(self.remote_tmux.as_ref()) {
            let hook_agent_exited = self.hook_state.as_ref().is_some_and(|h| h.saw_non_shell);
            if !self.proc_info.agent.is_empty()
                || self.agent_title_hint().is_some()
                || self.telemetry.detected_agent_label().is_some()
                || hook_agent_exited
            {
                self.clear_finished_agent();
            }
        } else if info.agent.is_empty() && !crate::tmux_local::is_local_tmux_pane(info) {
            if let Some(name) = self.agent_title_hint() {
                info.agent = name.to_owned();
            }
        }
    }

    fn agent_title_hint(&self) -> Option<&'static str> {
        self.remote_tmux.as_ref().and_then(|(_, _, cmd, _)| detect_agent_name(cmd))
            .or_else(|| detect_agent_name(&self.osc_title))
    }

    pub fn should_poll_usage(&self, is_remote: bool) -> bool {
        self.child_pid > 0 && (is_remote || self.proc_info.ssh_pid > 0
            || !self.proc_info.shell_owns_pane(self.remote_tmux.as_ref()))
    }

    pub fn clear_finished_agent(&mut self) {
        self.telemetry = SessionTelemetry::default();
        self.osc_title.clear();
        self.approval_snippet = None;
        self.state = WorkState::Idle;
        self.work_started_at = None;
        self.unread = false;
        self.hook_state = None;
    }

    pub fn apply_telemetry(&mut self, next: SessionTelemetry) {
        if next.remote_tailer.foreground_shell && !self.telemetry.remote_tailer.foreground_shell {
            self.clear_finished_agent();
            self.proc_info.agent.clear();
        }
        self.telemetry = next;
    }

    pub fn effective_agent(&self) -> String {
        if self.child_pid <= 0 || self.proc_info.shell_owns_pane(self.remote_tmux.as_ref()) {
            return String::new();
        }
        if !self.proc_info.agent.is_empty() {
            return self.proc_info.agent.clone();
        }
        self.agent_title_hint()
            .or_else(|| self.telemetry.detected_agent_label())
            .unwrap_or("")
            .to_owned()
    }

    pub fn is_active_agent(&self) -> bool {
        self.child_pid > 0 && !self.effective_agent().is_empty()
    }
}

pub fn work_state_priority(state: WorkState) -> u8 {
    match state {
        WorkState::NeedsApproval => 3,
        WorkState::Working => 2,
        WorkState::Ready => 1,
        WorkState::Idle => 0,
    }
}

pub fn pick_higher_state(current: WorkState, candidate: WorkState) -> WorkState {
    if work_state_priority(candidate) > work_state_priority(current) {
        candidate
    } else {
        current
    }
}

pub fn aggregate_work_states<'a>(states: impl IntoIterator<Item = &'a WorkState>) -> WorkState {
    states
        .into_iter()
        .copied()
        .fold(WorkState::Idle, pick_higher_state)
}

pub fn aggregate_pane_state(panes: &[PaneRecord]) -> WorkState {
    aggregate_work_states(panes.iter().map(|pane| &pane.state))
}

#[derive(Default)]
pub struct PaneRegistry {
    next_id: u64,
    panes: Vec<PaneRecord>,
}

impl PaneRegistry {
    pub fn allocate_id(&mut self) -> PaneId {
        self.next_id += 1;
        PaneId::new(self.next_id)
    }

    pub fn register(
        &mut self,
        vte: *mut GtkWidget,
        scroll: *mut GtkWidget,
        child_pid: i32,
        cwd: String,
    ) -> PaneId {
        let id = match take_spawned_pane_id(vte) {
            Some(global_id) => {
                if global_id.0 > self.next_id {
                    self.next_id = global_id.0;
                }
                global_id
            }
            None => self.allocate_id(),
        };
        self.panes.push(PaneRecord::new(id, vte, scroll, child_pid, cwd));
        id
    }

    #[cfg(test)]
    pub fn insert(&mut self, record: PaneRecord) {
        if record.id.raw() > self.next_id {
            self.next_id = record.id.raw();
        }
        self.panes.push(record);
    }

    pub fn remove_by_vte(&mut self, vte: *mut GtkWidget) -> Option<PaneRecord> {
        let idx = self.panes.iter().position(|pane| pane.vte == vte)?;
        Some(self.panes.remove(idx))
    }

    #[cfg(test)]
    pub fn remove_by_id(&mut self, id: PaneId) -> Option<PaneRecord> {
        let idx = self.panes.iter().position(|pane| pane.id == id)?;
        Some(self.panes.remove(idx))
    }

    pub fn get(&self, id: PaneId) -> Option<&PaneRecord> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    pub fn get_mut(&mut self, id: PaneId) -> Option<&mut PaneRecord> {
        self.panes.iter_mut().find(|pane| pane.id == id)
    }

    pub fn by_vte(&self, vte: *mut GtkWidget) -> Option<&PaneRecord> {
        self.panes.iter().find(|pane| pane.vte == vte)
    }

    pub fn by_vte_mut(&mut self, vte: *mut GtkWidget) -> Option<&mut PaneRecord> {
        self.panes.iter_mut().find(|pane| pane.vte == vte)
    }

    pub fn panes(&self) -> &[PaneRecord] {
        &self.panes
    }

    pub fn panes_mut(&mut self) -> &mut [PaneRecord] {
        &mut self.panes
    }

    pub fn len(&self) -> usize {
        self.panes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    pub fn aggregated_state(&self) -> WorkState {
        aggregate_pane_state(&self.panes)
    }

    pub fn has_unread(&self) -> bool {
        self.panes.iter().any(|p| p.unread)
    }

    pub fn active_agent_count(&self) -> usize {
        self.panes.iter().filter(|p| p.is_active_agent()).count()
    }

    pub fn live_turn_secs(&self, now: Instant) -> Option<u64> {
        self.panes.iter().filter_map(|p| p.live_turn_secs(now)).max()
    }

    pub fn last_turn_secs(&self) -> Option<u64> {
        self.panes.iter().filter_map(|p| p.last_turn_secs).max()
    }

    pub fn has_hook_error(&self) -> bool {
        self.panes.iter().any(PaneRecord::has_hook_error)
    }

    pub fn focus_pane(&mut self, vte: *mut GtkWidget, now: Instant) {
        let Some(pane) = self.by_vte_mut(vte) else {
            return;
        };
        pane.unread = false;
        if pane.state == WorkState::Ready {
            pane.set_state(WorkState::Idle, now);
            if pane.hook_state.as_ref().is_some_and(|h| h.state == HookState::Done) {
                pane.clear_hook_state();
            }
        }
    }

    #[cfg(test)]
    pub fn first_attention_pane(&self) -> Option<&PaneRecord> {
        let approval = self.panes.iter().find(|p| p.state == WorkState::NeedsApproval);
        approval.or_else(|| self.panes.iter().find(|p| p.unread || p.state == WorkState::Ready))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_widget(tag: usize) -> *mut GtkWidget {
        tag as *mut GtkWidget
    }

    fn sample_pane(reg: &mut PaneRegistry, tag: usize, state: WorkState) -> PaneId {
        let id = reg.register(fake_widget(tag), fake_widget(tag + 100), tag as i32, "/tmp".into());
        if let Some(rec) = reg.get_mut(id) {
            rec.state = state;
        }
        id
    }

    #[test]
    fn priority_aggregation_respects_needs_approval_over_working_ready_idle() {
        assert_eq!(aggregate_work_states(&[]), WorkState::Idle);
        assert_eq!(
            aggregate_work_states(&[WorkState::Idle, WorkState::Ready]),
            WorkState::Ready
        );
        assert_eq!(
            aggregate_work_states(&[WorkState::Ready, WorkState::Working]),
            WorkState::Working
        );
        assert_eq!(
            aggregate_work_states(&[WorkState::Working, WorkState::NeedsApproval]),
            WorkState::NeedsApproval
        );
    }

    #[test]
    fn registry_allocates_unique_ids_and_tracks_panes() {
        let mut reg = PaneRegistry::default();
        let id1 = sample_pane(&mut reg, 1, WorkState::Working);
        let id2 = sample_pane(&mut reg, 2, WorkState::NeedsApproval);
        assert_ne!(id1, id2);
        assert_eq!(reg.len(), 2);
        assert_eq!(reg.aggregated_state(), WorkState::NeedsApproval);
        assert_eq!(reg.first_attention_pane().map(|p| p.id), Some(id2));
        assert!(reg.by_vte(fake_widget(1)).is_some());
    }

    #[test]
    fn pane_record_tracks_turn_duration_and_exit_code() {
        let mut reg = PaneRegistry::default();
        let id = sample_pane(&mut reg, 3, WorkState::Idle);
        let t0 = Instant::now();
        let rec = reg.get_mut(id).expect("pane exists");
        rec.set_state(WorkState::Working, t0);
        assert_eq!(rec.work_started_at, Some(t0));
        rec.set_state(WorkState::Ready, t0 + std::time::Duration::from_secs(7));
        assert_eq!(rec.last_turn_secs, Some(7));
        rec.record_exit(130);
        assert_eq!(rec.last_exit_code, Some(130));
        assert!(reg.remove_by_vte(fake_widget(3)).is_some());
        assert!(reg.is_empty());
    }

    #[test]
    fn pane_live_turn_secs_requires_active_turn_start_not_creation_time() {
        let mut reg = PaneRegistry::default();
        let id = sample_pane(&mut reg, 9, WorkState::Idle);
        let t0 = Instant::now();
        let rec = reg.get_mut(id).expect("pane exists");
        rec.state = WorkState::Working;
        rec.work_started_at = None;
        assert_eq!(rec.live_turn_secs(t0 + std::time::Duration::from_secs(32_400)), None);
        rec.set_state(WorkState::Working, t0);
        assert_eq!(reg.live_turn_secs(t0 + std::time::Duration::from_secs(12)), Some(12));
    }

    #[test]
    fn registry_insert_advances_id_counter_and_header_attaches() {
        let mut reg = PaneRegistry::default();
        let rec = PaneRecord::new(PaneId::new(10), fake_widget(5), fake_widget(6), 42, "/app".into())
            .with_header(fake_widget(7));
        assert_eq!(rec.header_box, fake_widget(7));
        reg.insert(rec);
        let next = reg.allocate_id();
        assert_eq!(next.raw(), 11);
        assert!(reg.remove_by_id(PaneId::new(10)).is_some());
    }
}
