//! Canonical per-pane registry, state model, and priority aggregation.

#![allow(dead_code)]

use std::ptr;
use std::time::Instant;

use crate::ffi::GtkWidget;
use crate::proc_radar::ProcInfo;
use crate::session::WorkState;
use crate::telemetry::SessionTelemetry;

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

#[allow(dead_code)]
pub struct PaneRecord {
    pub id: PaneId,
    pub vte: *mut GtkWidget,
    pub scroll: *mut GtkWidget,
    pub header_box: *mut GtkWidget,
    pub child_pid: i32,
    pub cwd: String,
    pub proc_info: ProcInfo,
    pub state: WorkState,
    pub work_started_at: Option<Instant>,
    pub last_turn_secs: Option<u64>,
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
        Self {
            id,
            vte,
            scroll,
            header_box: ptr::null_mut(),
            child_pid,
            cwd,
            proc_info: ProcInfo::default(),
            state: WorkState::Idle,
            work_started_at: None,
            last_turn_secs: None,
            telemetry: SessionTelemetry::default(),
            last_exit_code: None,
        }
    }

    pub fn with_header(mut self, header_box: *mut GtkWidget) -> Self {
        self.header_box = header_box;
        self
    }

    pub fn set_state(&mut self, next: WorkState, now: Instant) {
        if next == WorkState::Working && self.work_started_at.is_none() {
            self.work_started_at = Some(now);
        }
        if next != WorkState::Working {
            if let Some(started) = self.work_started_at.take() {
                self.last_turn_secs = Some(now.saturating_duration_since(started).as_secs());
            }
        }
        self.state = next;
    }

    pub fn record_exit(&mut self, code: i32) {
        self.last_exit_code = Some(code);
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

#[allow(dead_code)]
#[derive(Default)]
pub struct PaneRegistry {
    next_id: u64,
    panes: Vec<PaneRecord>,
}

#[allow(dead_code)]
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

    pub fn first_attention_pane(&self) -> Option<&PaneRecord> {
        let approval = self.panes.iter().find(|p| p.state == WorkState::NeedsApproval);
        approval.or_else(|| self.panes.iter().find(|p| p.state == WorkState::Ready))
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
