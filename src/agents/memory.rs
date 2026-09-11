//! Forms episodic memories and reinforces existing ones (§9.2, §14).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::base::{clamp, event_text, fmt_rows, message_events};
use crate::Services;
use crate::llm::MEMORY_DECISION;
use crate::pyfmt::Row;
use crate::state::models::{LIVE_MEMORY, Proposal};

pub const SYSTEM: &str = "You are the memory agent of a persistent-state assistant.
Given one event and the most similar existing memories, decide whether anything is worth
remembering.
Most small talk is not. Prefer reinforcing an existing memory over creating a near-duplicate.
Each new memory is one self-contained sentence stating what happened or what was learned.
Return only the JSON object.";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewMemory {
    pub summary: String,
    pub importance: f64,
    pub confidence: f64,
    pub entities: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reinforce {
    pub memory_id: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryDecision {
    pub new_memories: Vec<NewMemory>,
    pub reinforce: Vec<Reinforce>,
}

pub async fn run(svc: &Services, events: &[Row]) -> Result<Vec<Proposal>> {
    let mut out = Vec::new();
    for e in message_events(events) {
        out.extend(one(svc, &e).await?);
    }
    Ok(out)
}

async fn one(svc: &Services, e: &Row) -> Result<Vec<Proposal>> {
    let mut proposals = Vec::new();
    let text = event_text(e);
    let eid = e["event_id"].as_str().unwrap_or_default().to_string();
    let emb = svc.llm.embed(std::slice::from_ref(&text)).await?.remove(0);
    let similar = svc
        .store
        .lock()
        .unwrap()
        .similar("memories", &emb, 5, Some(LIVE_MEMORY));
    let user = format!(
        "EVENT:\n[{eid}] {}: {text}\n\nSIMILAR MEMORIES:\n{}",
        e["source"].as_str().unwrap_or_default(),
        fmt_rows(&similar, &["summary", "importance"])
    );
    let out: MemoryDecision = svc
        .llm
        .complete_json(SYSTEM, &user, &MEMORY_DECISION)
        .await?;
    let known: Vec<&str> = similar.iter().filter_map(|m| m["id"].as_str()).collect();
    for r in &out.reinforce {
        if known.contains(&r.memory_id.as_str()) {
            proposals.push(
                Proposal::new(
                    "memory",
                    "update_memory",
                    json!({"reinforce": true, "add_source_events": [eid]}),
                )
                .target(&r.memory_id)
                .evidence(vec![eid.clone()])
                .confidence(0.8),
            );
        }
    }
    for nm in &out.new_memories {
        let ents: Vec<String> = {
            let st = svc.store.lock().unwrap();
            nm.entities
                .iter()
                .filter_map(|n| st.state.find_entity(n, None))
                .map(|x| x["id"].as_str().unwrap_or_default().to_string())
                .collect()
        };
        let imp = clamp(nm.importance);
        let payload = json!({
            "kind": "episodic",
            "summary": nm.summary,
            "importance": imp,
            "confidence": clamp(nm.confidence),
            "status": if imp >= 0.3 { "active" } else { "candidate" },
            "source_events": [eid],
            "entity_ids": ents,
        });
        proposals.push(
            Proposal::new("memory", "create_memory", payload)
                .evidence(vec![eid.clone()])
                .confidence(clamp(nm.confidence)),
        );
    }
    Ok(proposals)
}
