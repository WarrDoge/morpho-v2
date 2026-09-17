//! Seed dispositions: the temperament an agent starts with before experience revises it.
use anyhow::{Result, ensure};
use serde_json::{Value, json};

use crate::{
    Services,
    state::{engine::commit, models::Proposal},
};

/// Idempotent: a store that already holds traits keeps them.
pub async fn seed_traits(svc: &Services, traits: &[Value]) -> Result<usize> {
    if traits.is_empty()
        || !svc
            .store
            .lock()
            .unwrap()
            .state
            .table("traits")
            .rows
            .is_empty()
    {
        return Ok(0);
    }
    let event = svc.store.lock().unwrap().append_event(
        "seed",
        "harness",
        json!({"traits": traits.len()}),
        None,
        None,
    )?;
    let eid = event["event_id"].as_str().unwrap_or_default().to_string();
    let proposals: Vec<Proposal> = traits
        .iter()
        .map(|t| {
            Proposal::new("harness", "create_trait", t.clone())
                .evidence(vec![eid.clone()])
                .confidence(1.0)
                .reason("seed")
        })
        .collect();
    let results = commit(&svc.store, &svc.llm, &proposals, Some(&eid)).await?;
    for r in &results {
        ensure!(
            r.accepted,
            "seed trait rejected: {}",
            r.reason.clone().unwrap_or_default()
        );
    }
    Ok(results.len())
}
