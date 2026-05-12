use idb::{
    Database, DatabaseEvent, Factory, KeyPath, ObjectStoreParams, Query, TransactionMode,
};
use serde_wasm_bindgen::{from_value, to_value};
use wasm_bindgen::JsValue;

use super::{
    ActiveSession, PauseRecord, PhaseKind, SessionRecord, StorageError, StorageResult,
};

const DB_NAME: &str = "pomodoro";
// Bump this when the schema changes and extend `on_upgrade_needed` accordingly.
const DB_VERSION: u32 = 3;
const STORE_SESSIONS: &str = "sessions";
const STORE_PAUSES: &str = "pauses";
const INDEX_PAUSES_BY_SESSION: &str = "by_session";

pub struct IndexedDbStorage {
    db: Database,
}

impl IndexedDbStorage {
    pub async fn open() -> StorageResult<Self> {
        let factory = Factory::new()?;
        let mut req = factory.open(DB_NAME, Some(DB_VERSION))?;

        // Runs on first open and on every version bump. For a real migration
        // later, branch on `event.old_version()`. For now we just rebuild.
        req.on_upgrade_needed(|event| {
            let db = event.database().expect("database in upgrade event");
            for store in [STORE_SESSIONS, STORE_PAUSES] {
                if db.store_names().iter().any(|n| n == store) {
                    db.delete_object_store(store)
                        .expect("drop old object store");
                }
            }

            let mut params = ObjectStoreParams::new();
            params.auto_increment(true);
            db.create_object_store(STORE_SESSIONS, params)
                .expect("create sessions store");

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
        rec.completed_at_ms = Some(completed_at_ms);
        store.put(&to_value(&rec)?, Some(&key))?.await?;
        tx.commit()?.await?;
        Ok(())
    }

    /// Deletes the session and all of its pause records.
    pub async fn delete_session(&self, id: u64) -> StorageResult<()> {
        let tx = self.db.transaction(
            &[STORE_SESSIONS, STORE_PAUSES],
            TransactionMode::ReadWrite,
        )?;
        let sessions = tx.object_store(STORE_SESSIONS)?;
        let pauses = tx.object_store(STORE_PAUSES)?;

        let sid = JsValue::from_f64(id as f64);
        sessions.delete(sid.clone())?.await?;

        let index = pauses.index(INDEX_PAUSES_BY_SESSION)?;
        let pause_keys = index
            .get_all_keys(Some(Query::Key(sid)), None)?
            .await?;
        for k in pause_keys {
            pauses.delete(k)?.await?;
        }
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
            if rec.completed_at_ms.is_some() {
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

    pub async fn completed_work_count(&self) -> StorageResult<u32> {
        let tx = self
            .db
            .transaction(&[STORE_SESSIONS], TransactionMode::ReadOnly)?;
        let store = tx.object_store(STORE_SESSIONS)?;
        let values = store.get_all(None, None)?.await?;
        let mut n: u32 = 0;
        for v in values {
            let rec: SessionRecord = from_value(v)?;
            if rec.phase == PhaseKind::Work && rec.completed_at_ms.is_some() {
                n += 1;
            }
        }
        Ok(n)
    }
}
