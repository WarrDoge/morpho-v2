mod common;
use chrono::Duration;
use common::{event, fake, services};
use morpho::{
    agents::{
        reflection::{Reflection, Verification},
        self_model::SelfPatch,
    },
    context::composer::compose_text,
    interact::interact,
    pyfmt::{iso, now},
    state::{engine::commit, models::Proposal},
    worker::{commit_all, cycle},
};
use serde_json::{Value, json};

#[tokio::test]
async fn failed_maintenance_keeps_progress_and_retries_atomically() {
    let svc = services("maintenance-retry");
    event(&svc, "evidence");
    fake(&svc).fail("temporary failure");
    assert!(cycle(&svc, true).await.is_err());
    assert_eq!(svc.store.lock().unwrap().state.cursor("reflection"), 0);
    assert!(svc.store.lock().unwrap().state.snapshots.is_empty());
    assert!(
        svc.store
            .lock()
            .unwrap()
            .state
            .events
            .iter()
            .any(|e| e["type"] == "runtime_failure")
    );
    cycle(&svc, true).await.unwrap();
    assert_eq!(svc.store.lock().unwrap().state.cursor("reflection"), 2);
    assert_eq!(svc.store.lock().unwrap().state.snapshots.len(), 1);
    let calls = fake(&svc).calls_for("Reflection");
    assert!(calls.last().unwrap().contains("temporary failure"));
    cycle(&svc, false).await.unwrap();
    assert_eq!(fake(&svc).calls_for("Reflection").len(), calls.len());
}

#[tokio::test]
async fn due_predictions_trigger_after_event_cursor_is_caught_up() {
    let svc = services("idle-prediction");
    let eid = event(&svc, "rain observation");
    cycle(&svc, true).await.unwrap();
    let p = Proposal::new(
        "reflection",
        "create_prediction",
        json!({"prediction":"rain","probability":0.7,"deadline":iso(&(now()-Duration::days(1)))}),
    )
    .evidence(vec![eid.clone()]);
    let id = commit(&svc.store, &svc.llm, &[p], None).await.unwrap()[0].object_ids[0].clone();
    fake(&svc).queue(
        "Reflection",
        Reflection {
            verifications: vec![Verification {
                prediction_id: id.clone(),
                verified: true,
                evidence_ids: vec![eid],
            }],
            ..Default::default()
        },
    );
    let stats = cycle(&svc, false).await.unwrap();
    assert_eq!(stats["snapshot"], 2);
    assert_eq!(
        svc.store
            .lock()
            .unwrap()
            .state
            .get("predictions", &id)
            .unwrap()["verified"],
        true
    );
    let count = fake(&svc).calls_for("Reflection").len();
    cycle(&svc, false).await.unwrap();
    assert_eq!(fake(&svc).calls_for("Reflection").len(), count);
}

fn respond(system: &str, _user: &str, _schema: &str) -> Value {
    json!({"response":if system.contains("weather_uncertainty_observed") {"I need a fresh observation before answering."} else {"It will rain."},"changes":[]})
}

#[tokio::test]
async fn idle_self_revision_causally_changes_next_response_and_is_single_flight() {
    let svc = services("causal");
    *fake(&svc).respond.lock().unwrap() = Some(respond);
    let before = interact(&svc, "Will it rain?", None).await.unwrap();
    fake(&svc).queue(
        "Reflection",
        Reflection {
            self_model: Some(SelfPatch {
                uncertainties: vec!["weather_uncertainty_observed".into()],
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    fake(&svc)
        .delay_ms
        .store(20, std::sync::atomic::Ordering::Relaxed);
    let (a, b) = tokio::join!(cycle(&svc, true), cycle(&svc, true));
    a.unwrap();
    b.unwrap();
    assert_eq!(fake(&svc).calls_for("Reflection").len(), 1);
    let after = interact(&svc, "Will it rain?", None).await.unwrap();
    assert_eq!(before["response"], "It will rain.");
    assert_eq!(
        after["response"],
        "I need a fresh observation before answering."
    );
    assert_eq!(svc.store.lock().unwrap().state.self_state["version"], 2);
}

#[tokio::test]
async fn excess_proposals_are_retained_and_background_budget_is_durable() {
    let svc = services("retention");
    let eid = event(&svc, "explicit requests");
    let proposals = (0..51)
        .map(|i| {
            Proposal::new(
                "interaction",
                "create_goal",
                json!({"description":format!("request {i}")}),
            )
            .evidence(vec![eid.clone()])
        })
        .collect();
    let stats = commit_all(&svc, proposals, None).await.unwrap();
    assert_eq!(stats["accepted"], 51);
    assert_eq!(svc.store.lock().unwrap().state.proposals.len(), 51);
    let key = format!("maintenance_tokens:{}", now().format("%Y-%m-%d"));
    svc.store
        .lock()
        .unwrap()
        .set_metadata(&key, "100000")
        .unwrap();
    assert_eq!(
        cycle(&svc, true).await.unwrap()["maintenance"],
        "daily token budget exhausted"
    );
    assert!(fake(&svc).calls_for("Reflection").is_empty());
}

#[tokio::test]
async fn context_manifest_tracks_skipped_long_rows() {
    let svc = services("context-manifest");
    let eid = event(&svc, "reference");
    let long = Proposal::new(
        "interaction",
        "create_memory",
        json!({"summary":"reference ".repeat(600),"importance":1.0}),
    )
    .evidence(vec![eid.clone()]);
    let short = Proposal::new(
        "interaction",
        "create_memory",
        json!({"summary":"reference short","importance":0.1}),
    )
    .evidence(vec![eid]);
    let ids = commit(&svc.store, &svc.llm, &[long, short], None)
        .await
        .unwrap();
    let (text, manifest) = compose_text(&svc, "reference", Some(400)).await.unwrap();
    assert!(!text.contains(&ids[0].object_ids[0]));
    assert!(text.contains(&ids[1].object_ids[0]));
    assert_eq!(manifest["memories"][0]["id"], ids[1].object_ids[0]);
    assert!(manifest["tokens"].as_u64().unwrap() <= 400);
}

#[tokio::test]
async fn paused_input_does_not_block_idle_learning_or_leak_queued_text() {
    let svc = services("paused-maintenance");
    morpho::interact::enqueue(&svc, "unconsumed secret", None, "alice", Some("paused")).unwrap();
    fake(&svc).fail("observed provider failure");
    assert!(morpho::interact::process_next(&svc).await.is_err());
    {
        let st = svc.store.lock().unwrap();
        futures_executor::block_on(st.db.execute("UPDATE inbox SET attempts=3", ())).unwrap();
    }
    cycle(&svc, true).await.unwrap();
    let prompt = &fake(&svc).calls_for("Reflection")[0];
    assert!(prompt.contains("observed provider failure"));
    assert!(!prompt.contains("unconsumed secret"));
    assert_eq!(
        morpho::interact::request(&svc, "paused").unwrap()["status"],
        "paused"
    );
}
