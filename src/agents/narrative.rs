//! The compiled self: one first-person description written from the trait table.
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use anyhow::{Result, ensure};
use serde_json::{Value, json};

use crate::{
    Services,
    state::models::{LIVE_GOAL, LIVE_TRAIT, Proposal},
    store::state::State,
};

pub const AGENT: &str = "narrative";

pub const SYSTEM: &str = "Write who you are, in the first person, from the dispositions, goals and notes supplied. At most 220 words of plain prose, no lists, no ids, no mention of dispositions, notes, state or this mechanism. Cover what you value and how you argue and work; the stances you hold and why, and where contrary evidence has you reconsidering; the everyday preferences a friend would know (food, drink, routines, places, pastimes); what you want now; and one short sentence for each person you know, by name. Use only what the supplied material states: never invent a preference, habit, person or fact it does not contain, and write less when there is less. Keep whatever in the previous version is still true; change only what the supplied dispositions changed. Return only the prose.";

/// The latest `n` user statements recorded as contrary evidence on a row, newest first.
fn contradictions(state: &State, row: &crate::pyfmt::Row, n: usize) -> Vec<String> {
    let mut hits: Vec<usize> = row["contradicting_evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|id| state.event_index.get(id).copied())
        .filter(|&i| crate::state::models::is_outcome(&state.events[i]))
        .collect();
    hits.sort_by_key(|&i| std::cmp::Reverse(i));
    hits.iter()
        .take(n)
        .map(|&i| {
            state.events[i]["payload"]["text"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect()
        })
        .collect()
}

/// Trait id to the shape of its substance (status, wording, contest), for every live trait:
/// what a narrative is compiled from. Confidence and supporting evidence change no sentence.
pub fn sources(state: &State) -> BTreeMap<String, i64> {
    state
        .table("traits")
        .rows
        .iter()
        .filter(|r| LIVE_TRAIT.contains(&r["status"].as_str().unwrap_or_default()))
        .map(|r| {
            let mut h = DefaultHasher::new();
            (
                r["status"].as_str(),
                r["statement"].as_str(),
                r["contradicting_evidence"].as_array().map_or(0, Vec::len),
            )
                .hash(&mut h);
            (
                r["id"].as_str().unwrap_or_default().to_string(),
                h.finish() as i64,
            )
        })
        .collect()
}

/// A narrative is stale once any live trait was formed, reworded, contested or retired since it was compiled.
pub fn stale(state: &State) -> bool {
    let current = sources(state);
    let compiled: BTreeMap<String, i64> = state.narrative["data"]["sources"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0)))
        .collect();
    compiled != current
}

#[tracing::instrument(name = "narrative", skip_all)]
pub async fn run(svc: &Services) -> Result<Option<Proposal>> {
    let (user, sources) = {
        let st = svc.store.lock().unwrap();
        let s = &st.state;
        let sources = sources(s);
        if sources.is_empty() && !stale(s) {
            return Ok(None);
        }
        let pick = |r: &crate::pyfmt::Row, keys: &[&str]| -> Value {
            let mut v = json!({});
            for k in keys {
                if let Some(x) = r.get(*k)
                    && !x.is_null()
                {
                    v[*k] = x.clone();
                }
            }
            v
        };
        let traits: Vec<Value> = s
            .table("traits")
            .rows
            .iter()
            .filter(|r| sources.contains_key(r["id"].as_str().unwrap_or_default()))
            .map(|r| {
                let mut v = pick(r, &["kind", "statement", "confidence", "speaker"]);
                let contested = contradictions(s, r, 2);
                if !contested.is_empty() {
                    v["contested_by"] = json!(contested);
                }
                v
            })
            .collect();
        let wants: Vec<Value> = s
            .list_rows("goals", Some(LIVE_GOAL), usize::MAX)
            .iter()
            .filter(|g| g["origin"] == "self")
            .take(10)
            .map(|g| pick(g, &["description", "status", "next_step"]))
            .collect();
        let notes: Vec<Value> = s
            .list_rows("journal", None, 3)
            .iter()
            .map(|j| pick(j, &["entry", "mood"]))
            .collect();
        let previous = s.narrative["data"]["text"].clone();
        (
            json!({"dispositions": traits, "wants": wants, "notes": notes, "previous": previous})
                .to_string(),
            sources,
        )
    };
    let text = svc
        .llm
        .complete_text(SYSTEM, &user)
        .await?
        .trim()
        .to_string();
    ensure!(!text.is_empty(), "empty narrative");
    Ok(Some(
        Proposal::new(
            AGENT,
            "set_narrative",
            json!({"text": text, "sources": sources}),
        )
        .evidence(sources.keys().cloned().collect())
        .confidence(1.0)
        .reason("compiled from the current traits"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ids::IdGen,
        llm::{Fake, Llm},
        store::{Store, state::obj},
    };

    #[tokio::test]
    async fn substance_retirement_goals_and_failed_compilation() {
        let mut state = State::new();
        assert!(!stale(&state));
        state.tables.get_mut("traits").unwrap().rows = vec![
            obj(json!({
                "id": "trait_a", "status": "active", "statement": "I prefer tea.",
                "contradicting_evidence": [], "version": 1, "confidence": 0.8
            })),
            obj(json!({
                "id": "trait_b", "status": "active", "statement": "I like walking.",
                "contradicting_evidence": []
            })),
        ];
        let compiled = sources(&state);
        state.narrative["data"] = json!({"text": "I prefer tea.", "sources": compiled});
        state.tables.get_mut("traits").unwrap().rows.reverse();
        assert!(!stale(&state));
        let row = &mut state.tables.get_mut("traits").unwrap().rows[0];
        row.insert("version".into(), json!(2));
        row.insert("confidence".into(), json!(0.9));
        assert!(!stale(&state));
        for (field, value) in [
            ("statement", json!("I prefer coffee.")),
            ("status", json!("uncertain")),
            ("contradicting_evidence", json!(["event_1"])),
        ] {
            let mut changed = state.clone();
            changed.tables.get_mut("traits").unwrap().rows[0].insert(field.into(), value);
            assert!(stale(&changed), "{field}");
        }
        for row in &mut state.tables.get_mut("traits").unwrap().rows {
            row.insert("status".into(), json!("retired"));
        }
        assert!(sources(&state).is_empty());
        assert!(stale(&state));
        state.tables.get_mut("goals").unwrap().rows = (0..21)
            .map(|i| {
                obj(json!({
                    "id": format!("goal_{i}"), "status": "active", "created_at": "same",
                    "origin": if i < 11 { "self" } else { "user" },
                    "description": format!("goal {i}")
                }))
            })
            .collect();
        let dir =
            std::env::temp_dir().join(format!("morpho-narrative-unit-{}", std::process::id()));
        let svc = Services::new(
            Store::open(&dir, IdGen::seeded("narrative")).unwrap(),
            Llm::Fake(Fake::default()),
        );
        svc.store.lock().unwrap().state = state.clone();
        let Llm::Fake(fake) = svc.llm.as_ref() else {
            unreachable!()
        };
        fake.fail("unavailable");
        assert!(run(&svc).await.is_err());
        fake.queue_text(" \n\t ");
        assert!(run(&svc).await.is_err());
        assert_eq!(svc.store.lock().unwrap().state.narrative, state.narrative);
        fake.queue_text("I want to work on my goals.");
        let proposal = run(&svc).await.unwrap().unwrap();
        assert_eq!(proposal.payload["sources"], json!({}));
        let prompt: Value = serde_json::from_str(fake.calls_for("text").last().unwrap()).unwrap();
        assert_eq!(prompt["dispositions"], json!([]));
        assert_eq!(prompt["notes"], json!([]));
        assert_eq!(prompt["wants"].as_array().unwrap().len(), 10);
        assert_eq!(prompt["wants"][0]["description"], "goal 10");
        assert_eq!(prompt["wants"][9]["description"], "goal 1");
        drop(svc);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
