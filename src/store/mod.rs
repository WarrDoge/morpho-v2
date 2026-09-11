//! The storage engine: journal + vector file on disk, folded state in memory.

pub mod journal;
pub mod state;
pub mod vectors;

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
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
}

pub type Shared = Arc<Mutex<Store>>;

impl Store {
    pub fn open(dir: &Path, ids: IdGen) -> Result<Store> {
        std::fs::create_dir_all(dir)?;
        let (journal, entries) = Journal::open(&dir.join("journal.jsonl"))?;
        let vectors = Vectors::open(&dir.join("vectors.f32"), settings().embed_dim)?;
        let mut state = State::new();
        for (offset, e) in &entries {
            state.apply(*offset, &e.record);
        }
        Ok(Store {
            state,
            journal,
            vectors,
            ids,
        })
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

    pub fn similar(&self, table: &str, q: &[f32], k: usize, statuses: Option<&[&str]>) -> Vec<Row> {
        self.state.similar(table, &self.vectors, q, k, statuses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    pub fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("morpho-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn fold_equals_live_and_torn_tail_is_dropped() {
        let dir = temp_dir("store");
        let dim = settings().embed_dim;
        let mut st = Store::open(&dir, IdGen::seeded("t")).unwrap();
        st.append_event("user_message", "user", json!({"text": "hi"}), None, None)
            .unwrap();
        let slot = st.vectors.append(&vec![0.5; dim]).unwrap();
        let after = obj(json!({"id": "mem_1", "summary": "x", "status": "active",
            "created_at": "2026-09-11T10:00:00.000000+00:00"}));
        st.append(Record::Commit(Commit {
            proposal: obj(json!({"id": "prop_1", "decision": "accepted"})),
            transitions: vec![obj(
                json!({"id": 1, "proposal_id": "prop_1", "table_name": "memories",
                "object_id": "mem_1", "before": null, "after": after}),
            )],
            vectors: [("mem_1".to_string(), slot)].into(),
        }))
        .unwrap();
        st.set_cursor("memory", 1).unwrap();
        st.take_snapshot(json!({"memories": []})).unwrap();
        let live = st.state.clone();
        drop(st);

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("journal.jsonl"))
            .unwrap();
        f.write_all(b"{\"seq\":9,\"event\":{\"id\":2,\"event_id\":\"evt_torn")
            .unwrap();
        let mut v = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("vectors.f32"))
            .unwrap();
        v.write_all(&[1, 2, 3]).unwrap();

        let st = Store::open(&dir, IdGen::seeded("t")).unwrap();
        assert_eq!(st.state, live);
        assert_eq!(st.vectors.slots, 1);
        assert_eq!(st.state.get("memories", "mem_1").unwrap()["summary"], "x");
        assert_eq!(st.snapshots(5).unwrap().len(), 1);
        assert_eq!(st.state.cursor("memory"), 1);
        let hits = st.similar("memories", &vec![0.5; dim], 5, Some(&["active"]));
        assert_eq!(hits[0]["relevance"], json!(1.0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
