//! Shared prompt formatting, byte-identical to the Python helpers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Services;
use crate::pyfmt::{Row, py_dumps, py_str};
use crate::state::models::{MESSAGE_TYPES, strings, union};

pub fn event_text(e: &Row) -> String {
    match e.get("payload") {
        Some(Value::Object(p)) if p.contains_key("text") => py_str(&p["text"]),
        Some(p) => py_dumps(p),
        None => "None".into(),
    }
}

/// The event's stored embedding, or a fresh one for events appended without a vector.
pub async fn event_vector(svc: &Services, e: &Row) -> anyhow::Result<Vec<f32>> {
    let eid = e["event_id"].as_str().unwrap_or_default();
    let stored = {
        let st = svc.store.lock().unwrap();
        st.state.slots.get(eid).map(|&slot| st.vectors.get(slot))
    };
    match stored {
        Some(v) => Ok(v),
        None => Ok(svc.llm.embed(&[event_text(e)]).await?.remove(0)),
    }
}

pub fn fmt_events(events: &[Row]) -> String {
    events
        .iter()
        .map(|e| {
            format!(
                "[{}] {}/{}: {}",
                py_str(&e["event_id"]),
                py_str(&e["source"]),
                py_str(&e["type"]),
                event_text(e)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn fmt_rows(rows: &[Row], fields: &[&str]) -> String {
    let text = rows
        .iter()
        .map(|r| {
            let cols: Vec<String> = fields
                .iter()
                .map(|f| format!("{f}={}", py_str(r.get(*f).unwrap_or(&Value::Null))))
                .collect();
            format!("[{}] {}", py_str(&r["id"]), cols.join(" | "))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        "(none)".into()
    } else {
        text
    }
}

pub fn clamp(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

pub fn merge_questions(working: &Row, new: &[String]) -> Vec<String> {
    let current = working
        .get("open_questions")
        .map(strings)
        .unwrap_or_default();
    union(&current, new).into_iter().take(10).collect()
}

pub fn message_events(events: &[Row]) -> Vec<Row> {
    events
        .iter()
        .filter(|e| MESSAGE_TYPES.contains(&e["type"].as_str().unwrap_or_default()))
        .cloned()
        .collect()
}

pub fn event_ids(events: &[Row]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["event_id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BeliefStatus {
    #[default]
    Hypothesis,
    Active,
    Uncertain,
    Contradicted,
    Deprecated,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    #[default]
    Proposed,
    Active,
    Blocked,
    Completed,
    Abandoned,
    Superseded,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Relation {
    #[default]
    Supports,
    Contradicts,
}

pub fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}
