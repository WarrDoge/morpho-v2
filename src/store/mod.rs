//! One embedded libSQL database and privately staged, atomic state changes.

pub mod journal;
pub mod state;
pub mod vectors;

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Result, ensure};
use futures_executor::block_on;
use libsql::{Builder, Connection};
use serde_json::{Value, json};

use crate::config::settings;
use crate::ids::IdGen;
use crate::pyfmt::{Row, iso, now};
pub use journal::{Commit, Journal, Record};
pub use state::{State, obj};
pub use vectors::Vectors;

pub struct Store {
    pub state: State,
    pub journal: Journal,
    pub vectors: Vectors,
    pub ids: IdGen,
    pub db: Arc<Connection>,
    _owner: Arc<std::fs::File>,
}

pub type Shared = Arc<Mutex<Store>>;

impl Store {
    pub fn open(dir: &Path, ids: IdGen) -> Result<Store> {
        std::fs::create_dir_all(dir)?;
        let owner = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("coordinator.lock"))?;
        owner
            .try_lock()
            .map_err(|e| anyhow::anyhow!("another coordinator owns {}: {e}", dir.display()))?;
        let db = Arc::new(block_on(Builder::new_local(dir.join("morpho.db")).build())?.connect()?);
        block_on(db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS records(seq INTEGER PRIMARY KEY, record TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS vectors(slot INTEGER PRIMARY KEY, embedding F32_BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS inbox(seq INTEGER PRIMARY KEY AUTOINCREMENT, request_id TEXT NOT NULL UNIQUE,
              text TEXT NOT NULL, speaker TEXT NOT NULL, session TEXT, reply TEXT, attempts INTEGER NOT NULL DEFAULT 0,
              error TEXT, retry_at INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
        "))?;
        let model = format!("{}:{}", settings().embed_model, settings().embed_dim);
        let mut rows = block_on(db.query("SELECT value FROM metadata WHERE key='embedding'", ()))?;
        if let Some(row) = block_on(rows.next())? {
            ensure!(
                row.get::<String>(0)? == model,
                "embedding model/dimensions differ from stored database; explicit re-embedding required"
            );
        } else {
            let tx = block_on(db.transaction())?;
            let legacy = journal::read_legacy(&dir.join("journal.jsonl"))?;
            let vector_path = dir.join("vectors.f32");
            let bytes = if vector_path.exists() {
                std::fs::read(vector_path)?
            } else {
                Vec::new()
            };
            let stride = settings().embed_dim * 4;
            ensure!(stride > 0, "EMBED_DIM must be positive");
            let slots = bytes.len() / stride;
            for (i, chunk) in bytes.chunks_exact(stride).enumerate() {
                block_on(tx.execute(
                    "INSERT INTO vectors VALUES (?,vector32(?))",
                    libsql::params![i as i64, chunk.to_vec()],
                ))?;
            }
            for (seq, e) in legacy {
                let referenced: Vec<usize> = match &e.record {
                    Record::Event(r) => r
                        .get("vector")
                        .and_then(Value::as_u64)
                        .map(|v| v as usize)
                        .into_iter()
                        .collect(),
                    Record::Commit(c) => c.vectors.values().copied().collect(),
                    _ => Vec::new(),
                };
                ensure!(
                    referenced.iter().all(|s| *s < slots),
                    "legacy record {seq} references missing vector"
                );
                block_on(tx.execute(
                    "INSERT INTO records VALUES (?,?)",
                    libsql::params![seq as i64, serde_json::to_string(&e.record)?],
                ))?;
            }
            block_on(tx.execute("INSERT INTO metadata VALUES ('embedding',?)", [model]))?;
            block_on(tx.commit())?;
        }
        drop(rows);
        let entries = Journal::entries(&db)?;
        let journal = Journal {
            db: db.clone(),
            seq: entries.last().map_or(0, |(seq, _)| *seq),
            pending: None,
        };
        let mut rows = block_on(db.query("SELECT count(*) FROM vectors", ()))?;
        let slots = block_on(rows.next())?.unwrap().get::<i64>(0)? as usize;
        let vectors = Vectors {
            db: db.clone(),
            dim: settings().embed_dim,
            slots,
            pending: None,
        };
        let mut state = State::new();
        for (seq, e) in entries {
            state.apply(seq, &e.record);
        }
        Ok(Store {
            state,
            journal,
            vectors,
            ids,
            db,
            _owner: Arc::new(owner),
        })
    }

    /// A private projection: no database writes and no speculative state visible to readers.
    pub fn fork(&self) -> Store {
        let mut journal = self.journal.clone();
        journal.pending.get_or_insert_with(Vec::new);
        let mut vectors = self.vectors.clone();
        vectors.pending.get_or_insert_with(Vec::new);
        // ponytail: clone the in-memory projection; use incremental projections if memory pressure warrants it.
        Store {
            state: self.state.clone(),
            journal,
            vectors,
            ids: self.ids.clone(),
            db: self.db.clone(),
            _owner: self._owner.clone(),
        }
    }

    pub fn publish(&mut self, mut staged: Store, reply: Option<(&str, &Value)>) -> Result<()> {
        if self.journal.pending.is_some() {
            *self = staged;
            return Ok(());
        }
        let records = staged.journal.pending.as_ref().expect("staged store");
        let base = staged.journal.seq - records.len() as u64;
        ensure!(
            self.journal.seq == base,
            "state changed while turn was staged"
        );
        let vectors = staged.vectors.pending.as_ref().expect("staged vectors");
        let tx = block_on(self.db.transaction())?;
        for (i, v) in vectors.iter().enumerate() {
            block_on(tx.execute(
                "INSERT INTO vectors VALUES (?,vector32(?))",
                libsql::params![(self.vectors.slots + i) as i64, vectors::bytes(v)],
            ))?;
        }
        for (i, r) in records.iter().enumerate() {
            block_on(tx.execute(
                "INSERT INTO records VALUES (?,?)",
                libsql::params![(base + i as u64 + 1) as i64, serde_json::to_string(r)?],
            ))?;
        }
        if let Some((id, result)) = reply {
            let changed = block_on(tx.execute(
                "UPDATE inbox SET reply=?,error=NULL WHERE request_id=? AND reply IS NULL",
                libsql::params![serde_json::to_string(result)?, id],
            ))?;
            ensure!(changed == 1, "request already completed or missing");
        }
        block_on(tx.commit())?;
        staged.journal.pending = None;
        staged.vectors.pending = None;
        *self = staged;
        Ok(())
    }

    pub fn metadata(&self, key: &str) -> Result<Option<String>> {
        block_on(async {
            let mut rows = self
                .db
                .query("SELECT value FROM metadata WHERE key=?", [key])
                .await?;
            Ok(rows.next().await?.map(|r| r.get::<String>(0)).transpose()?)
        })
    }

    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        block_on(self.db.execute(
            "INSERT INTO metadata VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [key, value],
        ))?;
        Ok(())
    }

    pub fn shared(self) -> Shared {
        Arc::new(Mutex::new(self))
    }

    pub fn append(&mut self, record: Record) -> Result<()> {
        let (_, offset) = self.journal.append(&record)?;
        self.state.apply(offset, &record);
        Ok(())
    }

    pub fn append_event(
        &mut self,
        kind: &str,
        source: &str,
        payload: Value,
        session_id: Option<&str>,
        vector: Option<usize>,
    ) -> Result<Row> {
        let row = obj(json!({
            "id": self.state.last_event_id() + 1,
            "event_id": self.ids.next("evt"),
            "ts": iso(&now()),
            "type": kind,
            "source": source,
            "session_id": session_id,
            "payload": payload,
            "vector": vector,
        }));
        self.append(Record::Event(row.clone()))?;
        Ok(row)
    }

    pub fn set_cursor(&mut self, consumer: &str, last_event_id: i64) -> Result<()> {
        let row = obj(json!({
            "consumer": consumer, "last_event_id": last_event_id, "failures": 0,
            "updated_at": iso(&now()),
        }));
        self.append(Record::Cursor(row))
    }

    pub fn bump_failures(&mut self, consumer: &str) -> Result<i64> {
        let prev = self.state.cursors.get(consumer).cloned();
        let failures = prev
            .as_ref()
            .and_then(|c| c["failures"].as_i64())
            .unwrap_or(0)
            + 1;
        let row = obj(json!({
            "consumer": consumer,
            "last_event_id": prev.as_ref().and_then(|c| c["last_event_id"].as_i64()).unwrap_or(0),
            "failures": failures,
            "updated_at": prev.as_ref().map_or_else(|| json!(iso(&now())), |c| c["updated_at"].clone()),
        }));
        self.append(Record::Cursor(row))?;
        Ok(failures)
    }

    pub fn take_snapshot(&mut self, data: Value) -> Result<Row> {
        let row = obj(json!({
            "id": self.state.snapshots.len() as u64 + 1,
            "ts": iso(&now()),
            "last_event_id": self.state.last_event_id(),
            "data": data,
        }));
        self.append(Record::Snapshot(row.clone()))?;
        Ok(row)
    }

    /// Newest first, read back from the journal.
    pub fn snapshots(&self, limit: usize) -> Result<Vec<Row>> {
        self.state
            .snapshots
            .iter()
            .rev()
            .take(limit)
            .map(|s| match self.journal.read_at(s.offset)?.record {
                Record::Snapshot(row) => Ok(row),
                _ => anyhow::bail!("journal offset {} is not a snapshot", s.offset),
            })
            .collect()
    }

    pub fn similar(
        &self,
        table: &str,
        q: &[f32],
        k: usize,
        statuses: Option<&[&str]>,
    ) -> Result<Vec<Row>> {
        self.state.similar(table, &self.vectors, q, k, statuses)
    }
}
