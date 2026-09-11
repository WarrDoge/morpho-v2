//! State snapshots (SPEC §18) and deterministic replay from the journal (SPEC §2.2).

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use crate::Services;
use crate::pyfmt::Row;
use crate::state::models::{LIVE_GOAL, LIVE_MEMORY};
use crate::store::journal::{Journal, Record};
use crate::store::state::State;

pub fn take_snapshot(svc: &Services) -> Result<Row> {
    let mut st = svc.store.lock().unwrap();
    let s = &st.state;
    let data = json!({
        "working_state": s.working.get("data"),
        "self_state": s.self_state.get("data"),
        "memories": s.list_rows("memories", Some(LIVE_MEMORY), 1000),
        "beliefs": s.list_rows("beliefs", Some(&["hypothesis", "active", "uncertain"]), 1000),
        "goals": s.list_rows("goals", Some(LIVE_GOAL), 1000),
        "predictions": s.list_rows("predictions", None, 1000).into_iter()
            .filter(|p| p["verified"].is_null()).collect::<Vec<_>>(),
        "entities": s.list_rows("entities", None, 1000),
    });
    st.take_snapshot(data)
}

/// Pure fold of the journal into a fresh state, stopping after transition `up_to` if given.
pub fn replay(dir: &Path, up_to: Option<i64>) -> Result<(State, usize)> {
    let (_, entries) = Journal::open(&dir.join("journal.jsonl"))?;
    let mut state = State::new();
    let mut applied = 0;
    for (offset, e) in entries {
        let record = match e.record {
            Record::Commit(mut c) => {
                if let Some(n) = up_to {
                    c.transitions.retain(|t| t["id"].as_i64().unwrap_or(0) <= n);
                }
                applied += c.transitions.len();
                Record::Commit(c)
            }
            r => r,
        };
        state.apply(offset, &record);
    }
    Ok((state, applied))
}
