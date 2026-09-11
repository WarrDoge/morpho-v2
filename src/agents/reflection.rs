//! Periodic inspection: inconsistencies, patterns, open questions, predictions (§9.7, §19, §21).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::base::{
    BeliefStatus, clamp, enum_str, event_ids, fmt_events, fmt_rows, merge_questions,
};
use crate::Services;
use crate::llm::REFLECTION;
use crate::pyfmt::{Row, dt_of, iso, now};
use crate::state::models::{LIVE_BELIEF, Proposal};

pub const SYSTEM: &str = "You are the reflection agent of a persistent-state assistant.
Inspect the accumulated state. Report: beliefs that are inconsistent with each other or with
memories (with a revised confidence/status); recurring patterns worth storing as semantic memories
(cite memory or event ids as evidence); unresolved questions; a few concrete falsifiable
predictions with a horizon in days; and verdicts for predictions whose deadline has passed
(verified = true if the prediction came true according to the evidence). Cite the specific
event or memory ids each item rests on. Every statement must be about the user or the world in
plain language; never write statements about memories, beliefs, ids, or the state itself.
Be conservative. Return only the JSON object.";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeliefRevision {
    pub belief_id: String,
    pub confidence: f64,
    pub status: BeliefStatus,
    pub reason: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub statement: String,
    pub evidence_ids: Vec<String>,
    pub confidence: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewPrediction {
    pub prediction: String,
    pub probability: f64,
    pub days_until: i64,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub prediction_id: String,
    pub verified: bool,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reflection {
    pub inconsistencies: Vec<BeliefRevision>,
    pub patterns: Vec<Pattern>,
    pub open_questions: Vec<String>,
    pub predictions: Vec<NewPrediction>,
    pub verifications: Vec<Verification>,
}

fn or_default(ids: &[String], fallback: &[String]) -> Vec<String> {
    if ids.is_empty() {
        fallback.to_vec()
    } else {
        ids.to_vec()
    }
}

fn data(row: &Row) -> Row {
    row.get("data")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub async fn run(svc: &Services) -> Result<Vec<Proposal>> {
    let now = now();
    let (beliefs, memories, goals, recent, self_data, working, due) = {
        let st = svc.store.lock().unwrap();
        let s = &st.state;
        let due: Vec<Row> = s
            .list_rows("predictions", None, 50)
            .into_iter()
            .filter(|p| p["verified"].is_null())
            .filter(|p| dt_of(p.get("deadline")).is_some_and(|d| d <= now))
            .collect();
        (
            s.list_rows("beliefs", Some(LIVE_BELIEF), 20),
            s.list_rows(
                "memories",
                Some(&["active", "reinforced", "consolidated"]),
                30,
            ),
            s.list_rows("goals", Some(&["active", "blocked"]), 20),
            s.recent_events(10),
            data(&s.self_state),
            data(&s.working),
            due,
        )
    };
    if beliefs.is_empty() && memories.is_empty() && recent.is_empty() {
        return Ok(Vec::new());
    }
    let user = format!(
        "BELIEFS:\n{}\n\nMEMORIES:\n{}\n\nGOALS:\n{}\n\nPREDICTIONS PAST DEADLINE:\n{}\n\n\
         SELF MODEL:\n{}\n\nWORKING STATE:\n{}\n\nRECENT EVENTS:\n{}",
        fmt_rows(&beliefs, &["proposition", "confidence", "status"]),
        fmt_rows(&memories, &["kind", "summary"]),
        fmt_rows(&goals, &["status", "description"]),
        fmt_rows(&due, &["prediction", "probability"]),
        crate::pyfmt::py_dumps(&Value::Object(self_data)),
        crate::pyfmt::py_dumps(&Value::Object(working.clone())),
        fmt_events(&recent)
    );
    let out: Reflection = svc.llm.complete_json(SYSTEM, &user, &REFLECTION).await?;
    let mut evidence = event_ids(&recent);
    if evidence.is_empty() {
        evidence = vec!["reflection".into()];
    }
    let known_b: Vec<&str> = beliefs.iter().filter_map(|b| b["id"].as_str()).collect();
    let mut proposals = Vec::new();
    for r in out
        .inconsistencies
        .iter()
        .filter(|r| known_b.contains(&r.belief_id.as_str()))
    {
        let payload = json!({"confidence": clamp(r.confidence), "status": enum_str(&r.status)});
        proposals.push(
            Proposal::new("reflection", "update_belief", payload)
                .target(&r.belief_id)
                .evidence(or_default(&r.evidence_ids, &evidence))
                .confidence(clamp(r.confidence)),
        );
    }
    for p in &out.patterns {
        let payload = json!({
            "kind": "semantic", "summary": p.statement, "importance": 0.6,
            "confidence": clamp(p.confidence), "status": "active",
        });
        proposals.push(
            Proposal::new("reflection", "create_memory", payload)
                .evidence(or_default(&p.evidence_ids, &evidence))
                .confidence(clamp(p.confidence)),
        );
    }
    for p in &out.predictions {
        let deadline = now + chrono::Duration::days(p.days_until.max(1));
        let payload = json!({
            "prediction": p.prediction, "probability": clamp(p.probability), "deadline": iso(&deadline),
        });
        proposals.push(
            Proposal::new("reflection", "create_prediction", payload)
                .evidence(or_default(&p.evidence_ids, &evidence))
                .confidence(clamp(p.probability)),
        );
    }
    let known_p: Vec<&str> = due.iter().filter_map(|p| p["id"].as_str()).collect();
    for v in out
        .verifications
        .iter()
        .filter(|v| known_p.contains(&v.prediction_id.as_str()))
    {
        proposals.push(
            Proposal::new(
                "reflection",
                "verify_prediction",
                json!({"verified": v.verified}),
            )
            .target(&v.prediction_id)
            .evidence(or_default(&v.evidence_ids, &evidence))
            .confidence(0.7),
        );
    }
    if !out.open_questions.is_empty() {
        let patch =
            json!({"patch": {"open_questions": merge_questions(&working, &out.open_questions)}});
        proposals.push(
            Proposal::new("reflection", "set_working_state", patch)
                .evidence(evidence)
                .confidence(0.6),
        );
    }
    Ok(proposals)
}
