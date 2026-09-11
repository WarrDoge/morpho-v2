//! Worker semantics: retry, poison batches, single-flight cycles, reflection verifying predictions.

mod common;

use chrono::Duration;
use common::{event, fake, services};
use morpho::agents::memory::{MemoryDecision, NewMemory};
use morpho::agents::reflection::{Reflection, Verification};
use morpho::context::composer::compose_text;
use morpho::pyfmt::{iso, now};
use morpho::state::engine::commit;
use morpho::state::models::Proposal;
use morpho::worker::cycle;
use serde_json::json;

fn decision(summary: &str) -> MemoryDecision {
    MemoryDecision {
        new_memories: vec![NewMemory {
            summary: summary.into(),
            importance: 0.8,
            confidence: 0.9,
            entities: vec![],
        }],
        reinforce: vec![],
    }
}

fn memories(svc: &morpho::Services) -> usize {
    svc.store.lock().unwrap().state.table("memories").rows.len()
}

#[tokio::test]
async fn failed_batch_is_retried() {
    let svc = services("retry");
    event(&svc, "I moved to Berlin");
    fake(&svc).fail("boom");
    cycle(&svc, false).await.unwrap();
    assert_eq!(svc.store.lock().unwrap().state.cursor("memory"), 0);
    assert_eq!(memories(&svc), 0);
    fake(&svc).queue("MemoryDecision", decision("User moved to Berlin"));
    cycle(&svc, false).await.unwrap();
    assert_eq!(svc.store.lock().unwrap().state.cursor("memory"), 1);
    assert_eq!(memories(&svc), 1);
}

#[tokio::test]
async fn poison_batch_skipped_after_max_failures() {
    let svc = services("poison");
    event(&svc, "poison");
    for _ in 0..2 {
        fake(&svc).fail("boom");
        cycle(&svc, false).await.unwrap();
        assert_eq!(svc.store.lock().unwrap().state.cursor("memory"), 0);
    }
    fake(&svc).fail("boom");
    cycle(&svc, false).await.unwrap();
    assert_eq!(svc.store.lock().unwrap().state.cursor("memory"), 1);
    assert_eq!(memories(&svc), 0);
    event(&svc, "fine now");
    fake(&svc).queue("MemoryDecision", decision("All fine"));
    cycle(&svc, false).await.unwrap();
    assert_eq!(memories(&svc), 1);
}

#[tokio::test]
async fn concurrent_cycles_process_each_event_once() {
    let svc = services("concurrent");
    event(&svc, "once");
    fake(&svc).queue("MemoryDecision", decision("Said once"));
    let (a, b) = tokio::join!(cycle(&svc, false), cycle(&svc, false));
    a.unwrap();
    b.unwrap();
    assert_eq!(memories(&svc), 1);
    let st = svc.store.lock().unwrap();
    assert_eq!(
        st.state
            .proposals
            .iter()
            .filter(|p| p["agent"] == "memory")
            .count(),
        1
    );
}

#[tokio::test]
async fn reflection_verifies_due_predictions_and_snapshots() {
    let svc = services("predict");
    let eid = event(&svc, "It will rain tomorrow");
    let deadline = iso(&(now() - Duration::days(1)));
    let p = Proposal::new(
        "reflection",
        "create_prediction",
        json!({"prediction": "rain", "probability": 0.7, "deadline": deadline}),
    )
    .evidence(vec![eid.clone()]);
    let pid = commit(&svc.store, &svc.llm, &[p], None).await.unwrap()[0].object_ids[0].clone();
    fake(&svc).queue(
        "Reflection",
        Reflection {
            verifications: vec![Verification {
                prediction_id: pid.clone(),
                verified: true,
                evidence_ids: vec![eid],
            }],
            ..Default::default()
        },
    );
    let stats = cycle(&svc, true).await.unwrap();
    assert_eq!(stats["snapshot"], json!(1));
    let st = svc.store.lock().unwrap();
    let row = st.state.get("predictions", &pid).unwrap();
    assert_eq!(row["verified"], json!(true));
    assert!(row["verified_at"].is_string());
    assert_eq!(row["version"], json!(2));
    assert_eq!(st.snapshots(5).unwrap().len(), 1);
}

#[tokio::test]
async fn context_stays_within_budget() {
    let svc = services("budget");
    let eid = event(&svc, "start");
    let proposals: Vec<Proposal> = (0..40)
        .map(|i| {
            Proposal::new("memory", "create_memory", json!({"summary": format!("Fact number {i} about the user's long and detailed life story")}))
                .evidence(vec![eid.clone()])
        })
        .collect();
    commit(&svc.store, &svc.llm, &proposals, None)
        .await
        .unwrap();
    for i in 0..15 {
        event(&svc, &format!("{i} {}", "long message ".repeat(40)));
    }
    let (_, manifest) = compose_text(&svc, "life story", Some(600)).await.unwrap();
    assert!(manifest["tokens"].as_u64().unwrap() <= 600);
    assert!(
        manifest["sections"]["memories"]["dropped"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(manifest["sections"]["recent"]["included"].as_u64().unwrap() > 0);
}
