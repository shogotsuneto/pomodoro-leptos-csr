// Storage layer.
//
// `mod.rs` defines the backend-agnostic domain types and error surface.
// Concrete backends live in sibling modules (currently only `indexeddb`).
// To swap or add a backend, expose a struct with the same shape of methods
// as `IndexedDbStorage` — there is no trait yet because we only have one
// implementation, but the seam is here when we need it.

pub mod indexeddb;

use serde::{Deserialize, Serialize};

use crate::timer::Phase;

/// One Work or Break attempt. Created on Start; mutated only at the terminal
/// transition (natural completion or user abandonment). All pause/resume
/// events live in the separate `PauseRecord` store, keyed back here by
/// `session_id`.
///
/// Terminal state is mutually exclusive: at most one of `completed_at_ms`
/// or `abandoned_at_ms` is `Some`. Both `None` means the session is still
/// active (running or paused).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub phase: PhaseKind,
    pub started_at_ms: i64,
    pub duration_secs: u32,
    pub completed_at_ms: Option<i64>,
    /// Set when the user explicitly bailed out (e.g., hit "New Work" mid-Work
    /// or skipped a Break). Distinct from completion so analytics can tell
    /// them apart. `#[serde(default)]` keeps old records (without this field)
    /// loadable.
    #[serde(default)]
    pub abandoned_at_ms: Option<i64>,
    /// Work session may be associated with a Task. Always `None` for Break.
    /// `#[serde(default)]` keeps pre-tasks records loadable.
    #[serde(default)]
    pub task_id: Option<u64>,
}

impl SessionRecord {
    pub fn new(
        phase: PhaseKind,
        started_at_ms: i64,
        duration_secs: u32,
        task_id: Option<u64>,
    ) -> Self {
        Self {
            phase,
            started_at_ms,
            duration_secs,
            completed_at_ms: None,
            abandoned_at_ms: None,
            task_id,
        }
    }

    pub fn is_active(&self) -> bool {
        self.completed_at_ms.is_none() && self.abandoned_at_ms.is_none()
    }
}

/// A user-defined task that Work sessions can be attributed to. Stored in
/// its own object store; `Settings.selected_task_id` is the currently chosen
/// one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub name: String,
    pub created_at_ms: i64,
    /// Archived tasks are hidden from the picker but still resolve when an
    /// older session references them.
    #[serde(default)]
    pub archived: bool,
}

/// One pause interval within a session. `resumed_at_ms` is `None` while the
/// user is still paused; it fills in when they hit Start again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PauseRecord {
    pub session_id: u64,
    pub paused_at_ms: i64,
    pub resumed_at_ms: Option<i64>,
}

/// Combined view of an active (uncompleted) session and its pause history.
/// Built by `IndexedDbStorage::load_active`; everything the timer UI needs
/// to compute remaining time is in here, so the tick loop never re-queries.
#[derive(Debug, Clone)]
pub struct ActiveSession {
    pub session_id: u64,
    pub session: SessionRecord,
    /// Sum of `(resumed_at - paused_at)` for every closed pause.
    pub closed_paused_ms: i64,
    /// `(pause_id, paused_at_ms)` of the currently-open pause, if any.
    pub open_pause: Option<(u64, i64)>,
}

impl ActiveSession {
    pub fn fresh(session_id: u64, session: SessionRecord) -> Self {
        Self {
            session_id,
            session,
            closed_paused_ms: 0,
            open_pause: None,
        }
    }

    /// Milliseconds of timer progress so far. `now_ms` is only consulted when
    /// the session is currently running (not paused, not completed).
    pub fn elapsed_ms(&self, now_ms: i64) -> i64 {
        let endpoint = self
            .session
            .completed_at_ms
            .or(self.open_pause.map(|(_, t)| t))
            .unwrap_or(now_ms);
        (endpoint - self.session.started_at_ms - self.closed_paused_ms).max(0)
    }

    pub fn remaining_secs(&self, now_ms: i64) -> u32 {
        let elapsed_s = (self.elapsed_ms(now_ms) / 1000) as u32;
        self.session.duration_secs.saturating_sub(elapsed_s)
    }

    pub fn is_paused(&self) -> bool {
        self.open_pause.is_some()
    }
}

/// User-configurable preferences. Persisted as a singleton record in the
/// `settings` object store.
///
/// `#[serde(default)]` at struct level means: any missing field in a stored
/// record is filled in from `Settings::default()`. This is how new fields
/// added in later versions stay backwards-compatible — old records simply
/// adopt today's defaults for whatever they didn't store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub work_minutes: u32,
    pub break_minutes: u32,
    pub beep_volume: f32,
    pub auto_start_next: bool,
    /// The task to attribute the next Work session to. `None` means no task.
    pub selected_task_id: Option<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            work_minutes: 25,
            break_minutes: 5,
            beep_volume: 0.4,
            auto_start_next: true,
            selected_task_id: None,
        }
    }
}

impl Settings {
    pub fn duration_secs(&self, phase: PhaseKind) -> u32 {
        match phase {
            PhaseKind::Work => self.work_minutes.saturating_mul(60),
            PhaseKind::Break => self.break_minutes.saturating_mul(60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhaseKind {
    Work,
    Break,
}

impl From<Phase> for PhaseKind {
    fn from(p: Phase) -> Self {
        match p {
            Phase::Work => PhaseKind::Work,
            Phase::Break => PhaseKind::Break,
        }
    }
}

impl From<PhaseKind> for Phase {
    fn from(p: PhaseKind) -> Self {
        match p {
            PhaseKind::Work => Phase::Work,
            PhaseKind::Break => Phase::Break,
        }
    }
}

pub type StorageResult<T> = Result<T, StorageError>;

#[derive(Debug)]
pub struct StorageError(pub String);

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StorageError {}

impl From<idb::Error> for StorageError {
    fn from(e: idb::Error) -> Self {
        StorageError(e.to_string())
    }
}

impl From<serde_wasm_bindgen::Error> for StorageError {
    fn from(e: serde_wasm_bindgen::Error) -> Self {
        StorageError(e.to_string())
    }
}
