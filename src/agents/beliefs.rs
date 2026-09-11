//! Creates hypotheses and revises belief confidence from evidence (§9.4, §16).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::base::{
    BeliefStatus, Relation, clamp, enum_str, event_text, event_vector, fmt_rows, message_events,
};
use crate::Services;
use crate::llm::BELIEF_DECISION;
use crate::pyfmt::Row;
use crate::state::models::{LIVE_BELIEF, Proposal};

pub const SYSTEM: &str = "You are the belief agent of a persistent-state assistant.
Beliefs are propositions about the user or the world held with explicit uncertainty.
Given one event and similar existing beliefs: mark which beliefs this event supports or contradicts
(with a revised confidence and status), and propose new hypotheses only for genuinely new,
generalizable propositions. Contradictory beliefs may coexist; do not force resolution.
Statuses: hypothesis, active, uncertain, contradicted, deprecated. Return only the JSON object.";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewBelief {
    pub proposition: String,
    pub confidence: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeliefUpdate {
    pub belief_id: String,
    pub relation: Relation,
    pub new_confidence: f64,
    pub new_status: BeliefStatus,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeliefDecision {
    pub new_beliefs: Vec<NewBelief>,
    pub updates: Vec<BeliefUpdate>,
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
    let emb = event_vector(svc, e).await?;
    let similar = svc
        .store
        .lock()
        .unwrap()
        .similar("beliefs", &emb, 5, Some(LIVE_BELIEF));
    let user = format!(
        "EVENT:\n[{eid}] {}: {text}\n\nSIMILAR BELIEFS:\n{}",
        e["source"].as_str().unwrap_or_default(),
        fmt_rows(&similar, &["proposition", "confidence", "status"])
    );
    let out: BeliefDecision = svc
        .llm
        .complete_json(SYSTEM, &user, &BELIEF_DECISION)
        .await?;
    let known: Vec<&str> = similar.iter().filter_map(|b| b["id"].as_str()).collect();
    for u in &out.updates {
        if !known.contains(&u.belief_id.as_str()) {
            continue;
        }
        let key = if u.relation == Relation::Supports {
            "add_supporting"
        } else {
            "add_contradicting"
        };
        let payload = json!({
            "confidence": clamp(u.new_confidence),
            "status": enum_str(&u.new_status),
            key: [eid],
        });
        proposals.push(
            Proposal::new("beliefs", "update_belief", payload)
                .target(&u.belief_id)
                .evidence(vec![eid.clone()])
                .confidence(clamp(u.new_confidence)),
        );
    }
    for nb in &out.new_beliefs {
        let payload = json!({
            "proposition": nb.proposition,
            "confidence": clamp(nb.confidence),
            "status": "hypothesis",
        });
        proposals.push(
            Proposal::new("beliefs", "create_belief", payload)
                .evidence(vec![eid.clone()])
                .confidence(clamp(nb.confidence)),
        );
    }
    Ok(proposals)
}
