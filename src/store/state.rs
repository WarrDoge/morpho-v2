//! Derived state folded from the journal; every query the engine, composer and API need.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use super::journal::Record;
use super::vectors::{Vectors, relevance};
use crate::pyfmt::Row;

pub const TABLES: [&str; 6] = [
    "memories",
    "beliefs",
    "goals",
    "predictions",
    "entities",
    "entity_relationships",
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub rows: Vec<Row>,
    pub index: HashMap<String, usize>,
}

impl Table {
    fn upsert(&mut self, row: Row) {
        let id = row["id"].as_str().unwrap_or_default().to_string();
        match self.index.get(&id) {
            Some(&i) => self.rows[i] = row,
            None => {
                self.index.insert(id, self.rows.len());
                self.rows.push(row);
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Row> {
        self.index.get(id).map(|&i| &self.rows[i])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotRef {
    pub id: u64,
    pub ts: String,
    pub last_event_id: i64,
    pub offset: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub events: Vec<Row>,
    pub event_index: HashMap<String, usize>,
    pub tables: HashMap<String, Table>,
    pub working: Row,
    pub self_state: Row,
    pub cursors: BTreeMap<String, Row>,
    pub proposals: Vec<Row>,
    pub transitions: Vec<Row>,
    pub snapshots: Vec<SnapshotRef>,
    pub slots: HashMap<String, usize>,
}

pub fn obj(v: Value) -> Row {
    match v {
        Value::Object(m) => m,
        _ => Row::new(),
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn created(r: &Row) -> &str {
    s(&r["created_at"])
}

fn has_status(r: &Row, statuses: Option<&[&str]>) -> bool {
    statuses.is_none_or(|st| st.contains(&s(&r["status"])))
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> State {
        let ts = "1970-01-01T00:00:00.000000+00:00";
        let singleton =
            |data: Value| obj(json!({"id": 1, "data": data, "version": 1, "updated_at": ts}));
        State {
            events: Vec::new(),
            event_index: HashMap::new(),
            tables: TABLES
                .iter()
                .map(|t| (t.to_string(), Table::default()))
                .collect(),
            working: singleton(json!({
                "current_topic": null, "active_entities": [], "recent_events": [],
                "open_questions": [], "current_task": null, "current_constraints": []
            })),
            self_state: singleton(json!({
                "capabilities": [], "limitations": [], "current_objectives": [], "commitments": [],
                "recent_actions": [], "known_failures": [], "uncertainties": [],
                "predicted_future_states": []
            })),
            cursors: BTreeMap::new(),
            proposals: Vec::new(),
            transitions: Vec::new(),
            snapshots: Vec::new(),
            slots: HashMap::new(),
        }
    }

    /// The single mutation path, used both when folding the journal and after a live append.
    pub fn apply(&mut self, offset: u64, record: &Record) {
        match record {
            Record::Event(row) => {
                let event_id = s(&row["event_id"]).to_string();
                if let Some(slot) = row.get("vector").and_then(Value::as_u64) {
                    self.slots.insert(event_id.clone(), slot as usize);
                }
                self.event_index.insert(event_id, self.events.len());
                self.events.push(row.clone());
            }
            Record::Commit(c) => {
                self.proposals.push(c.proposal.clone());
                for t in &c.transitions {
                    self.transitions.push(t.clone());
                    let Some(after) = t["after"].as_object() else {
                        continue;
                    };
                    match s(&t["table_name"]) {
                        "working_state" => self.working = after.clone(),
                        "self_state" => self.self_state = after.clone(),
                        table => {
                            if let Some(tb) = self.tables.get_mut(table) {
                                tb.upsert(after.clone());
                            }
                        }
                    }
                }
                for (id, slot) in &c.vectors {
                    self.slots.insert(id.clone(), *slot);
                }
            }
            Record::Cursor(row) => {
                self.cursors
                    .insert(s(&row["consumer"]).to_string(), row.clone());
            }
            Record::Snapshot(row) => self.snapshots.push(SnapshotRef {
                id: row["id"].as_u64().unwrap_or_default(),
                ts: s(&row["ts"]).to_string(),
                last_event_id: row["last_event_id"].as_i64().unwrap_or_default(),
                offset,
            }),
        }
    }

    pub fn table(&self, name: &str) -> &Table {
        self.tables
            .get(name)
            .unwrap_or_else(|| panic!("no table {name}"))
    }

    pub fn get(&self, table: &str, id: &str) -> Option<&Row> {
        self.tables.get(table).and_then(|t| t.get(id))
    }

    /// `ORDER BY created_at DESC LIMIT n`; ties by insertion order, newest first.
    pub fn list_rows(&self, table: &str, statuses: Option<&[&str]>, limit: usize) -> Vec<Row> {
        let t = self.table(table);
        let mut idx: Vec<usize> = (0..t.rows.len())
            .filter(|&i| has_status(&t.rows[i], statuses))
            .collect();
        idx.sort_by(|&a, &b| created(&t.rows[b]).cmp(created(&t.rows[a])).then(b.cmp(&a)));
        idx.into_iter()
            .take(limit)
            .map(|i| t.rows[i].clone())
            .collect()
    }

    /// `ORDER BY embedding <=> q LIMIT k`, with `relevance = 1 - distance` attached.
    pub fn similar(
        &self,
        table: &str,
        vectors: &Vectors,
        q: &[f32],
        k: usize,
        statuses: Option<&[&str]>,
    ) -> Vec<Row> {
        let t = self.table(table);
        let mut hits: Vec<(usize, f64)> = t
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| has_status(r, statuses))
            .filter_map(|(i, r)| self.slots.get(s(&r["id"])).map(|&slot| (i, slot)))
            .map(|(i, slot)| (i, 1.0 - vectors.similarity(slot, q)))
            .collect();
        hits.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        hits.into_iter()
            .take(k)
            .map(|(i, dist)| {
                let mut r = t.rows[i].clone();
                r.insert("relevance".into(), json!(relevance(1.0 - dist)));
                r
            })
            .collect()
    }

    pub fn singleton(&self, name: &str) -> &Row {
        if name == "self_state" {
            &self.self_state
        } else {
            &self.working
        }
    }

    pub fn find_entity(&self, name: &str, kind: Option<&str>) -> Option<Row> {
        if name.starts_with("ent_") {
            return self.get("entities", name).cloned();
        }
        let lname = name.to_lowercase();
        let rows = &self.table("entities").rows;
        let matches = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| s(&r["name"]).to_lowercase() == lname);
        match kind {
            Some(k) => matches
                .filter(|(_, r)| s(&r["kind"]) == k)
                .map(|(_, r)| r.clone())
                .next(),
            None => matches
                .min_by(|(ia, a), (ib, b)| created(a).cmp(created(b)).then(ia.cmp(ib)))
                .map(|(_, r)| r.clone()),
        }
    }

    pub fn relationships_for(&self, ids: &[String]) -> Vec<Row> {
        let ents = self.table("entities");
        let mut rows: Vec<Row> = self
            .table("entity_relationships")
            .rows
            .iter()
            .filter(|r| r["valid_until"].is_null())
            .filter(|r| ids.iter().any(|i| i == s(&r["src"]) || i == s(&r["dst"])))
            .map(|r| {
                let mut r = r.clone();
                for (key, name) in [("src", "src_name"), ("dst", "dst_name")] {
                    let n = ents
                        .get(s(&r[key]))
                        .map(|e| e["name"].clone())
                        .unwrap_or(Value::Null);
                    r.insert(name.into(), n);
                }
                r
            })
            .collect();
        rows.sort_by(|a, b| {
            created(a)
                .cmp(created(b))
                .then(s(&a["id"]).cmp(s(&b["id"])))
        });
        rows
    }

    pub fn transitions_for(&self, object_id: &str, limit: usize) -> Vec<Row> {
        let by_id: HashMap<&str, &Row> = self.proposals.iter().map(|p| (s(&p["id"]), p)).collect();
        self.transitions
            .iter()
            .filter(|t| s(&t["object_id"]) == object_id)
            .take(limit)
            .map(|t| {
                let mut r = t.clone();
                if let Some(p) = by_id.get(s(&t["proposal_id"])) {
                    r.insert("proposal_agent".into(), p["agent"].clone());
                    for k in ["operation", "evidence", "confidence", "decision", "reason"] {
                        r.insert(k.into(), p[k].clone());
                    }
                }
                r
            })
            .collect()
    }

    pub fn transitions(&self, limit: usize, after: i64) -> Vec<Row> {
        self.transitions
            .iter()
            .filter(|t| t["id"].as_i64().unwrap_or_default() > after)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn proposals(&self, decision: Option<&str>, limit: usize) -> Vec<Row> {
        self.proposals
            .iter()
            .rev()
            .filter(|p| decision.is_none_or(|d| s(&p["decision"]) == d))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn cursor(&self, consumer: &str) -> i64 {
        self.cursors
            .get(consumer)
            .and_then(|c| c["last_event_id"].as_i64())
            .unwrap_or(0)
    }

    pub fn event(&self, event_id: &str) -> Option<&Row> {
        self.event_index.get(event_id).map(|&i| &self.events[i])
    }

    pub fn events_many(&self, ids: &[String]) -> Vec<Row> {
        self.events
            .iter()
            .filter(|e| ids.iter().any(|i| i == s(&e["event_id"])))
            .cloned()
            .collect()
    }

    pub fn events_after(&self, cursor: i64, limit: usize) -> Vec<Row> {
        let start = cursor.max(0) as usize;
        self.events
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn recent_events(&self, n: usize) -> Vec<Row> {
        let start = self.events.len().saturating_sub(n);
        self.events[start..].to_vec()
    }

    pub fn last_event_id(&self) -> i64 {
        self.events.len() as i64
    }
}
