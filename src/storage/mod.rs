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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub phase: PhaseKind,
    pub started_at_ms: i64,
    pub duration_secs: u32,
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
