//! State snapshots (SPEC §18) and deterministic replay from the journal (SPEC §2.2).

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use crate::Services;
use crate::pyfmt::Row;

use crate::store::journal::{Journal, Record};
use crate::store::state::State;

pub fn take_snapshot(svc: &Services) -> Result<Row> {
    let mut st = svc.store.lock().unwrap();
    let s = &st.state;
    let data = json!({
        "working_state": s.working, "self_state": s.self_state, "narrative": s.narrative,
        "tables": s.tables.iter().map(|(k,t)| (k, &t.rows)).collect::<std::collections::BTreeMap<_,_>>(),
        "cursors": s.cursors, "slots": s.slots,
        "last_transition_id": s.transitions.len(),
    });
    st.take_snapshot(data)
}

/// Pure fold of the journal into a fresh state, stopping after transition `up_to` if given.
pub fn replay(dir: &Path, up_to: Option<i64>) -> Result<(State, usize)> {
    let entries = if dir.join("morpho.db").exists() {
        let db = futures_executor::block_on(
            libsql::Builder::new_local(dir.join("morpho.db"))
                .flags(libsql::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .build(),
        )?;
        Journal::entries(&db.connect()?)?
    } else {
        crate::store::journal::read_legacy(&dir.join("journal.jsonl"))?
    };
    let mut state = State::new();
    let mut applied = 0;
    for (offset, e) in entries {
        if up_to.is_some_and(|n| applied as i64 >= n) {
            break;
        }
        let record = match e.record {
            Record::Commit(mut c) => {
                if let Some(n) = up_to {
                    c.transitions.retain(|t| t["id"].as_i64().unwrap_or(0) <= n);
                    c.vectors
                        .retain(|id, _| c.transitions.iter().any(|t| t["object_id"] == *id));
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
