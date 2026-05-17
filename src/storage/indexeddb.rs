use idb::{
    Database, DatabaseEvent, Factory, KeyPath, ObjectStoreParams, Query, TransactionMode,
};
use serde_wasm_bindgen::{from_value, to_value};
use wasm_bindgen::JsValue;

use super::{
    ActiveSession, PauseRecord, PhaseKind, SessionRecord, Settings, StorageError, StorageResult,
    Task,
};

const DB_NAME: &str = "pomodoro";
// Bump this when the schema changes and extend `on_upgrade_needed` accordingly.
const DB_VERSION: u32 = 5;
const STORE_SESSIONS: &str = "sessions";
const STORE_PAUSES: &str = "pauses";
const STORE_SETTINGS: &str = "settings";
const STORE_TASKS: &str = "tasks";
const INDEX_PAUSES_BY_SESSION: &str = "by_session";
// `settings` is a singleton store keyed by this fixed value.
const SETTINGS_KEY: f64 = 1.0;

pub struct IndexedDbStorage {
    db: Database,
}

impl IndexedDbStorage {
    pub async fn open() -> StorageResult<Self> {
        let factory = Factory::new()?;
        let mut req = factory.open(DB_NAME, Some(DB_VERSION))?;

        // Runs on first open and on every version bump. Idempotent — only
        // creates stores that don't already exist, so users upgrading keep
        // their session/pause history. For a real schema change later,
        // branch on `event.old_version()`.
        req.on_upgrade_needed(|event| {
            let db = event.database().expect("database in upgrade event");
            let existing = db.store_names();
            let has = |n: &str| existing.iter().any(|s| s == n);

            if !has(STORE_SESSIONS) {
                let mut params = ObjectStoreParams::new();
                params.auto_increment(true);
                db.create_object_store(STORE_SESSIONS, params)
                    .expect("create sessions store");
            }

            if !has(STORE_PAUSES) {
                let mut params = ObjectStoreParams::new();
                params.auto_increment(true);
                let pauses = db
                    .create_object_store(STORE_PAUSES, params)
                    .expect("create pauses store");
                pauses
                    .create_index(
                        INDEX_PAUSES_BY_SESSION,
                        KeyPath::new_single("session_id"),
                        None,
                    )
                    .expect("create pauses index");
            }

            if !has(STORE_SETTINGS) {
                let params = ObjectStoreParams::new();
                db.create_object_store(STORE_SETTINGS, params)
                    .expect("create settings store");
            }

            if !has(STORE_TASKS) {
                let mut params = ObjectStoreParams::new();
                params.auto_increment(true);
                db.create_object_store(STORE_TASKS, params)
                    .expect("create tasks store");
            }
        });

        let db = req.await?;
        Ok(Self { db })
    }

    // -- sessions -----------------------------------------------------------

    pub async fn start_session(&self, rec: &SessionRecord) -> StorageResult<u64> {
        let tx = self
            .db
            .transaction(&[STORE_SESSIONS], TransactionMode::ReadWrite)?;
        let store = tx.object_store(STORE_SESSIONS)?;
        let key = store.add(&to_value(rec)?, None)?.await?;
        tx.commit()?.await?;
        Ok(key.as_f64().unwrap_or(0.0) as u64)
    }

    pub async fn complete_session(&self, id: u64, completed_at_ms: i64) -> StorageResult<()> {
        self.terminate_session(id, |rec| rec.completed_at_ms = Some(completed_at_ms))
            .await
    }

    pub async fn abandon_session(&self, id: u64, abandoned_at_ms: i64) -> StorageResult<()> {
        self.terminate_session(id, |rec| rec.abandoned_at_ms = Some(abandoned_at_ms))
            .await
    }

    async fn terminate_session(
        &self,
        id: u64,
        mutate: impl FnOnce(&mut SessionRecord),
    ) -> StorageResult<()> {
        let tx = self
            .db
            .transaction(&[STORE_SESSIONS], TransactionMode::ReadWrite)?;
        let store = tx.object_store(STORE_SESSIONS)?;
        let key = JsValue::from_f64(id as f64);
        let v = store
            .get(key.clone())?
            .await?
            .ok_or_else(|| StorageError(format!("session {id} not found")))?;
        let mut rec: SessionRecord = from_value(v)?;
        mutate(&mut rec);
        store.put(&to_value(&rec)?, Some(&key))?.await?;
        tx.commit()?.await?;
        Ok(())
    }

    // -- pauses -------------------------------------------------------------

    pub async fn start_pause(&self, session_id: u64, paused_at_ms: i64) -> StorageResult<u64> {
        let rec = PauseRecord {
            session_id,
            paused_at_ms,
            resumed_at_ms: None,
        };
        let tx = self
            .db
            .transaction(&[STORE_PAUSES], TransactionMode::ReadWrite)?;
        let store = tx.object_store(STORE_PAUSES)?;
        let key = store.add(&to_value(&rec)?, None)?.await?;
        tx.commit()?.await?;
        Ok(key.as_f64().unwrap_or(0.0) as u64)
    }

    pub async fn end_pause(&self, pause_id: u64, resumed_at_ms: i64) -> StorageResult<()> {
        let tx = self
            .db
            .transaction(&[STORE_PAUSES], TransactionMode::ReadWrite)?;
        let store = tx.object_store(STORE_PAUSES)?;
        let key = JsValue::from_f64(pause_id as f64);
        let v = store
            .get(key.clone())?
            .await?
            .ok_or_else(|| StorageError(format!("pause {pause_id} not found")))?;
        let mut rec: PauseRecord = from_value(v)?;
        rec.resumed_at_ms = Some(resumed_at_ms);
        store.put(&to_value(&rec)?, Some(&key))?.await?;
        tx.commit()?.await?;
        Ok(())
    }

    // -- queries ------------------------------------------------------------

    /// Loads the active (uncompleted) session, if any, along with its pauses
    /// already aggregated into `closed_paused_ms` + `open_pause`.
    pub async fn load_active(&self) -> StorageResult<Option<ActiveSession>> {
        let tx = self
            .db
            .transaction(&[STORE_SESSIONS, STORE_PAUSES], TransactionMode::ReadOnly)?;
        let sessions = tx.object_store(STORE_SESSIONS)?;

        let s_keys = sessions.get_all_keys(None, None)?.await?;
        let s_values = sessions.get_all(None, None)?.await?;

        let mut active: Option<(u64, SessionRecord)> = None;
        for (k, v) in s_keys.into_iter().zip(s_values) {
            let rec: SessionRecord = from_value(v)?;
            if !rec.is_active() {
                continue;
            }
            let id = k.as_f64().unwrap_or(0.0) as u64;
            // Defensive against stragglers from a crash: keep the latest.
            if active
                .as_ref()
                .is_none_or(|(_, r)| rec.started_at_ms > r.started_at_ms)
            {
                active = Some((id, rec));
            }
        }

        let Some((session_id, session)) = active else {
            return Ok(None);
        };

        let pauses = tx.object_store(STORE_PAUSES)?;
        let index = pauses.index(INDEX_PAUSES_BY_SESSION)?;
        let sid_jv = JsValue::from_f64(session_id as f64);
        let p_keys = index
            .get_all_keys(Some(Query::Key(sid_jv.clone())), None)?
            .await?;
        let p_values = index
            .get_all(Some(Query::Key(sid_jv)), None)?
            .await?;

        let mut closed_paused_ms: i64 = 0;
        let mut open_pause: Option<(u64, i64)> = None;
        for (k, v) in p_keys.into_iter().zip(p_values) {
            let pr: PauseRecord = from_value(v)?;
            match pr.resumed_at_ms {
                Some(r) => closed_paused_ms += (r - pr.paused_at_ms).max(0),
                None => {
                    let pid = k.as_f64().unwrap_or(0.0) as u64;
                    open_pause = Some((pid, pr.paused_at_ms));
                }
            }
        }

        Ok(Some(ActiveSession {
            session_id,
            session,
            closed_paused_ms,
            open_pause,
        }))
    }

    // -- settings -----------------------------------------------------------

    pub async fn load_settings(&self) -> StorageResult<Settings> {
        let tx = self
            .db
            .transaction(&[STORE_SETTINGS], TransactionMode::ReadOnly)?;
        let store = tx.object_store(STORE_SETTINGS)?;
        let key = JsValue::from_f64(SETTINGS_KEY);
        match store.get(key)?.await? {
            Some(v) => Ok(from_value(v)?),
            None => Ok(Settings::default()),
        }
    }

    pub async fn save_settings(&self, settings: &Settings) -> StorageResult<()> {
        let tx = self
            .db
            .transaction(&[STORE_SETTINGS], TransactionMode::ReadWrite)?;
        let store = tx.object_store(STORE_SETTINGS)?;
        let key = JsValue::from_f64(SETTINGS_KEY);
        store.put(&to_value(settings)?, Some(&key))?.await?;
        tx.commit()?.await?;
        Ok(())
    }

    // -- tasks --------------------------------------------------------------

    pub async fn create_task(&self, task: &Task) -> StorageResult<u64> {
        let tx = self
            .db
            .transaction(&[STORE_TASKS], TransactionMode::ReadWrite)?;
        let store = tx.object_store(STORE_TASKS)?;
        let key = store.add(&to_value(task)?, None)?.await?;
        tx.commit()?.await?;
        Ok(key.as_f64().unwrap_or(0.0) as u64)
    }

    /// Returns all tasks (including archived), paired with their ids.
    pub async fn list_tasks(&self) -> StorageResult<Vec<(u64, Task)>> {
        let tx = self
            .db
            .transaction(&[STORE_TASKS], TransactionMode::ReadOnly)?;
        let store = tx.object_store(STORE_TASKS)?;
        let keys = store.get_all_keys(None, None)?.await?;
        let values = store.get_all(None, None)?.await?;
        let mut out = Vec::with_capacity(keys.len());
        for (k, v) in keys.into_iter().zip(values) {
            let id = k.as_f64().unwrap_or(0.0) as u64;
            let task: Task = from_value(v)?;
            out.push((id, task));
        }
        Ok(out)
    }

    pub async fn rename_task(&self, id: u64, name: &str) -> StorageResult<()> {
        self.mutate_task(id, |t| t.name = name.to_string()).await
    }

    pub async fn set_task_archived(&self, id: u64, archived: bool) -> StorageResult<()> {
        self.mutate_task(id, |t| t.archived = archived).await
    }

    async fn mutate_task(&self, id: u64, mutate: impl FnOnce(&mut Task)) -> StorageResult<()> {
        let tx = self
            .db
            .transaction(&[STORE_TASKS], TransactionMode::ReadWrite)?;
        let store = tx.object_store(STORE_TASKS)?;
        let key = JsValue::from_f64(id as f64);
        let v = store
            .get(key.clone())?
            .await?
            .ok_or_else(|| StorageError(format!("task {id} not found")))?;
        let mut task: Task = from_value(v)?;
        mutate(&mut task);
        store.put(&to_value(&task)?, Some(&key))?.await?;
        tx.commit()?.await?;
        Ok(())
    }

    /// Returns the most recent terminated (completed OR abandoned) sessions,
    /// ordered by `started_at_ms` descending and capped at `limit`. Active
    /// sessions are excluded — they're shown live on the main screen.
    pub async fn list_session_history(
        &self,
        limit: usize,
    ) -> StorageResult<Vec<(u64, SessionRecord)>> {
        let tx = self
            .db
            .transaction(&[STORE_SESSIONS], TransactionMode::ReadOnly)?;
        let store = tx.object_store(STORE_SESSIONS)?;
        let keys = store.get_all_keys(None, None)?.await?;
        let values = store.get_all(None, None)?.await?;
        let mut entries: Vec<(u64, SessionRecord)> = Vec::with_capacity(keys.len());
        for (k, v) in keys.into_iter().zip(values) {
            let id = k.as_f64().unwrap_or(0.0) as u64;
            let rec: SessionRecord = from_value(v)?;
            if rec.completed_at_ms.is_some() || rec.abandoned_at_ms.is_some() {
                entries.push((id, rec));
            }
        }
        entries.sort_by(|a, b| b.1.started_at_ms.cmp(&a.1.started_at_ms));
        entries.truncate(limit);
        Ok(entries)
    }

    /// Returns `(total, since)` counts of naturally-completed Work sessions.
    /// `since` is the subset whose `completed_at_ms >= since_ms` — pass the
    /// start of today's local midnight to get a "today" count. Single scan
    /// to avoid two transactions.
    pub async fn completed_work_counts(&self, since_ms: i64) -> StorageResult<(u32, u32)> {
        let tx = self
            .db
            .transaction(&[STORE_SESSIONS], TransactionMode::ReadOnly)?;
        let store = tx.object_store(STORE_SESSIONS)?;
        let values = store.get_all(None, None)?.await?;
        let mut total: u32 = 0;
        let mut since: u32 = 0;
        for v in values {
            let rec: SessionRecord = from_value(v)?;
            if rec.phase != PhaseKind::Work {
                continue;
            }
            if let Some(at) = rec.completed_at_ms {
                total += 1;
                if at >= since_ms {
                    since += 1;
                }
            }
        }
        Ok((total, since))
    }
}
