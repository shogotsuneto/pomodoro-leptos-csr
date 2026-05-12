use idb::{Database, DatabaseEvent, Factory, ObjectStoreParams, TransactionMode};
use serde_wasm_bindgen::{from_value, to_value};

use super::{PhaseKind, SessionRecord, StorageResult};

const DB_NAME: &str = "pomodoro";
const DB_VERSION: u32 = 1;
const STORE_SESSIONS: &str = "sessions";

pub struct IndexedDbStorage {
    db: Database,
}

impl IndexedDbStorage {
    pub async fn open() -> StorageResult<Self> {
        let factory = Factory::new()?;
        let mut open_request = factory.open(DB_NAME, Some(DB_VERSION))?;

        // Runs on first open and on version bump. When adding a new object
        // store or migration step, bump DB_VERSION and extend this handler
        // — IndexedDB will replay it for users on the old version.
        open_request.on_upgrade_needed(|event| {
            let db = event.database().expect("database in upgrade event");
            let mut params = ObjectStoreParams::new();
            params.auto_increment(true);
            db.create_object_store(STORE_SESSIONS, params)
                .expect("create sessions object store");
        });

        let db = open_request.await?;
        Ok(Self { db })
    }

    pub async fn add_session(&self, rec: &SessionRecord) -> StorageResult<()> {
        let tx = self
            .db
            .transaction(&[STORE_SESSIONS], TransactionMode::ReadWrite)?;
        let store = tx.object_store(STORE_SESSIONS)?;
        let value = to_value(rec)?;
        store.add(&value, None)?.await?;
        tx.commit()?.await?;
        Ok(())
    }

    pub async fn all_sessions(&self) -> StorageResult<Vec<SessionRecord>> {
        let tx = self
            .db
            .transaction(&[STORE_SESSIONS], TransactionMode::ReadOnly)?;
        let store = tx.object_store(STORE_SESSIONS)?;
        let values = store.get_all(None, None)?.await?;
        let mut out = Vec::with_capacity(values.len());
        for v in values {
            out.push(from_value(v)?);
        }
        Ok(out)
    }

    pub async fn completed_work_count(&self) -> StorageResult<u32> {
        let all = self.all_sessions().await?;
        Ok(all.iter().filter(|s| s.phase == PhaseKind::Work).count() as u32)
    }
}
