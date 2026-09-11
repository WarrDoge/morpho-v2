//! Maintains the operational self model (§7, §9.5). Cannot grant capabilities (§23).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::base::{event_ids, fmt_events, message_events};
use crate::Services;
use crate::llm::SELF_PATCH;
use crate::pyfmt::{Row, py_dumps};
use crate::state::models::Proposal;

pub const SYSTEM: &str = "You are the self-model agent of a persistent-state assistant.
Given the current self model and recent events, return the updated self model fields.
Keep each list short (at most 8 items), most recent or most relevant first. Record commitments the
assistant made, actions it took, failures, limitations it hit, and things it is uncertain about.
You cannot add capabilities or permissions. Return only the JSON object.";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfPatch {
    pub limitations: Vec<String>,
    pub commitments: Vec<String>,
    pub recent_actions: Vec<String>,
    pub known_failures: Vec<String>,
    pub uncertainties: Vec<String>,
    pub current_objectives: Vec<String>,
}

pub async fn run(svc: &Services, events: &[Row]) -> Result<Vec<Proposal>> {
    let events = message_events(events);
    if events.is_empty() {
        return Ok(Vec::new());
    }
    let self_data = {
        let st = svc.store.lock().unwrap();
        st.state
            .self_state
            .get("data")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    let user = format!(
        "SELF MODEL:\n{}\n\nEVENTS:\n{}",
        py_dumps(&Value::Object(self_data.clone())),
        fmt_events(&events)
    );
    let out: SelfPatch = svc.llm.complete_json(SYSTEM, &user, &SELF_PATCH).await?;
    let lists = [
        ("limitations", &out.limitations),
        ("commitments", &out.commitments),
        ("recent_actions", &out.recent_actions),
        ("known_failures", &out.known_failures),
        ("uncertainties", &out.uncertainties),
        ("current_objectives", &out.current_objectives),
    ];
    let patch: Row = lists
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v.iter().take(8).collect::<Vec<_>>())))
        .collect();
    if patch.iter().all(|(k, v)| self_data.get(k) == Some(v)) {
        return Ok(Vec::new());
    }
    Ok(vec![
        Proposal::new("self_model", "update_self_state", json!({"patch": patch}))
            .evidence(event_ids(&events))
            .confidence(0.7),
    ])
}
