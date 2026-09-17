//! The compiled self: one first-person description written from the trait table.
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use anyhow::{Result, ensure};
use serde_json::{Value, json};

use crate::{
    Services,
    agents::episode::is_practice,
    context::composer::shown_trait,
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
        .filter(|r| !is_practice(r) || (promoted(state, r) && shown_trait(r)))
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

/// A practice becomes part of who the agent is once it held with confidence across episodes.
pub fn promoted(state: &State, row: &crate::pyfmt::Row) -> bool {
    let episodes = row["supporting_evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| state.event(id).is_some_and(|e| e["type"] == "episode"))
        .count();
    is_practice(row) && row["confidence"].as_f64().unwrap_or(0.0) >= 0.6 && episodes >= 2
}

/// A narrative is stale once any live trait was formed, reworded, contested or retired since it was compiled.
pub fn stale(state: &State) -> bool {
    let current = sources(state);
    if current.is_empty() {
        return false;
    }
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
        if sources.is_empty() {
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
            .list_rows("goals", Some(LIVE_GOAL), 10)
            .iter()
            .filter(|g| g["origin"] == "self")
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
