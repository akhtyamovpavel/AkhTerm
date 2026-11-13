//! Canonical per-pane registry, state model, and priority aggregation.

#[cfg(test)]
use std::ptr;
use std::time::Instant;

use crate::ffi::GtkWidget;
use crate::proc_radar::ProcInfo;
use crate::session::{detect_agent_name, WorkState};
use crate::telemetry::SessionTelemetry;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u64);

impl PaneId {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[cfg(test)]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

pub struct PaneRecord {
    pub id: PaneId,
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
}

impl PaneRecord {
    pub fn new(
        id: PaneId,
        vte: *mut GtkWidget,
        scroll: *mut GtkWidget,
        child_pid: i32,
        cwd: String,
    ) -> Self {
        let now = Instant::now();
        let _ = scroll;
        Self {
            id,
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
        }
    }

    #[cfg(test)]
    pub fn with_header(mut self, header_box: *mut GtkWidget) -> Self {
        self.header_box = header_box;
        self
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
        if self.state != WorkState::Working {
            self.set_state(WorkState::Working, now);
        }
        if is_focused_active {
            self.unread = false;
        }
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
        let started = self.work_started_at.unwrap_or(self.work_started);
        Some(now.saturating_duration_since(started).as_secs())
    }

    pub fn effective_agent(&self) -> String {
        if !self.proc_info.agent.is_empty() {
            return self.proc_info.agent.clone();
        }
        let tmux_agent = self.remote_tmux.as_ref().and_then(|(_, w, c, _)| {
            detect_agent_name(c).or_else(|| detect_agent_name(w))
        });
        tmux_agent
            .or_else(|| detect_agent_name(&self.osc_title))
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
        let id = self.allocate_id();
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

    #[cfg(test)]
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

    #[cfg(test)]
    pub fn live_turn_secs(&self, now: Instant) -> Option<u64> {
        self.panes.iter().filter_map(|p| p.live_turn_secs(now)).max()
    }

    pub fn last_turn_secs(&self) -> Option<u64> {
        self.panes.iter().filter_map(|p| p.last_turn_secs).max()
    }

    pub fn focus_pane(&mut self, vte: *mut GtkWidget, now: Instant) {
        let Some(pane) = self.by_vte_mut(vte) else {
            return;
        };
        pane.unread = false;
        if pane.state == WorkState::Ready {
            pane.set_state(WorkState::Idle, now);
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
