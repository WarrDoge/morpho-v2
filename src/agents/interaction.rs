//! Interprets one incoming interaction; proposes working-state, entities, explicit goals (§9.1).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::base::{clamp, event_text, fmt_rows, merge_questions};
use crate::Services;
use crate::llm::INTERPRETATION;
use crate::pyfmt::{Row, py_dumps};
use crate::state::models::{LIVE_GOAL, Proposal};

pub const SYSTEM: &str = "You are the interaction agent of a persistent-state assistant.
Interpret the latest user input against the current working state and goals.
Identify entities (people, places, projects, things), explicit requests the user made that are not
already covered by an existing goal, open questions, and constraints. Rate how important this input
is to remember long-term (0 = trivia, 1 = critical). Return only the JSON object.";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interpretation {
    pub current_topic: String,
    pub current_task: Option<String>,
    pub entities: Vec<Entity>,
    pub new_explicit_requests: Vec<String>,
    pub open_questions: Vec<String>,
    pub constraints: Vec<String>,
    pub importance: f64,
}

pub async fn run(svc: &Services, event: &Row) -> Result<Vec<Proposal>> {
    let (working, goals) = {
        let st = svc.store.lock().unwrap();
        let w = st
            .state
            .working
            .get("data")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        (w, st.state.list_rows("goals", Some(LIVE_GOAL), 100))
    };
    let user = format!(
        "WORKING STATE:\n{}\n\nEXISTING GOALS:\n{}\n\nINPUT:\n{}",
        py_dumps(&Value::Object(working.clone())),
        fmt_rows(&goals, &["status", "description"]),
        event_text(event)
    );
    let out: Interpretation = svc
        .llm
        .complete_json(SYSTEM, &user, &INTERPRETATION)
        .await?;
    let eid = event["event_id"].as_str().unwrap_or_default().to_string();
    let mut patch = json!({
        "current_topic": out.current_topic,
        "current_task": out.current_task,
        "active_entities": out.entities.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        "current_constraints": out.constraints,
        "last_input_importance": clamp(out.importance),
    });
    if !out.open_questions.is_empty() {
        patch["open_questions"] = json!(merge_questions(&working, &out.open_questions));
    }
    let mut proposals = vec![
        Proposal::new("interaction", "set_working_state", json!({"patch": patch}))
            .evidence(vec![eid.clone()])
            .confidence(0.9),
    ];
    for e in &out.entities {
        let kind = if e.kind.is_empty() { "thing" } else { &e.kind };
        proposals.push(
            Proposal::new(
                "interaction",
                "upsert_entity",
                json!({"name": e.name, "kind": kind}),
            )
            .evidence(vec![eid.clone()])
            .confidence(0.8),
        );
    }
    for r in &out.new_explicit_requests {
        let payload =
            json!({"description": r, "priority": 0.7, "origin": "user", "status": "active"});
        proposals.push(
            Proposal::new("interaction", "create_goal", payload)
                .evidence(vec![eid.clone()])
                .confidence(0.85),
        );
    }
    Ok(proposals)
}
