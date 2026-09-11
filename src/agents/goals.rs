//! Tracks goal lifecycle and conflicts (§8, §9.6). Inferred goals are marked inferred.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::base::{
    GoalStatus, clamp, enum_str, event_ids, fmt_events, fmt_rows, merge_questions, message_events,
};
use crate::Services;
use crate::llm::GOAL_REVIEW;
use crate::pyfmt::Row;
use crate::state::models::{LIVE_GOAL, Proposal};

pub const SYSTEM: &str = "You are the goal agent of a persistent-state assistant.
Given the current goals and recent events: mark goals that are now completed, blocked, abandoned or
superseded; list new goals that are strongly implied but were never explicitly requested (these are
inferred, be conservative); and describe conflicts between goals. Return only the JSON object.";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalChange {
    pub goal_id: String,
    pub status: GoalStatus,
    pub reason: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewGoal {
    pub description: String,
    pub priority: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalReview {
    pub changes: Vec<GoalChange>,
    pub inferred_goals: Vec<NewGoal>,
    pub conflicts: Vec<String>,
}

pub async fn run(svc: &Services, events: &[Row]) -> Result<Vec<Proposal>> {
    let events = message_events(events);
    if events.is_empty() {
        return Ok(Vec::new());
    }
    let goals = svc
        .store
        .lock()
        .unwrap()
        .state
        .list_rows("goals", Some(LIVE_GOAL), 100);
    if goals.is_empty() && events.len() < 2 {
        return Ok(Vec::new());
    }
    let user = format!(
        "GOALS:\n{}\n\nEVENTS:\n{}",
        fmt_rows(&goals, &["status", "origin", "priority", "description"]),
        fmt_events(&events)
    );
    let out: GoalReview = svc.llm.complete_json(SYSTEM, &user, &GOAL_REVIEW).await?;
    let evidence = event_ids(&events);
    let known: HashMap<&str, &Row> = goals
        .iter()
        .map(|g| (g["id"].as_str().unwrap_or_default(), g))
        .collect();
    let mut proposals = Vec::new();
    for c in &out.changes {
        let status = enum_str(&c.status);
        if known
            .get(c.goal_id.as_str())
            .is_some_and(|g| g["status"] != json!(status))
        {
            proposals.push(
                Proposal::new("goals", "update_goal", json!({"status": status}))
                    .target(&c.goal_id)
                    .evidence(evidence.clone())
                    .confidence(0.75),
            );
        }
    }
    for g in &out.inferred_goals {
        let payload = json!({
            "description": g.description,
            "priority": clamp(g.priority),
            "origin": "inferred",
            "status": "proposed",
        });
        proposals.push(
            Proposal::new("goals", "create_goal", payload)
                .evidence(evidence.clone())
                .confidence(0.5),
        );
    }
    if !out.conflicts.is_empty() {
        let working = {
            let st = svc.store.lock().unwrap();
            st.state
                .working
                .get("data")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        };
        let patch = json!({"patch": {"open_questions": merge_questions(&working, &out.conflicts)}});
        proposals.push(
            Proposal::new("goals", "set_working_state", patch)
                .evidence(evidence)
                .confidence(0.6),
        );
    }
    Ok(proposals)
}
