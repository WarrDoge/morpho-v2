mod common;
use common::{event, fake, services};
use morpho::agents::{narrative, seed::seed_traits};
use morpho::context::composer::compose_text_as;
use morpho::interact::{RESPONSE_SYSTEM, system_prompt};
use morpho::state::engine::commit;
use morpho::state::models::Proposal;
use morpho::worker::compile_narrative;
use serde_json::json;

async fn one(svc: &morpho::Services, p: Proposal) -> morpho::state::engine::CommitResult {
    commit(&svc.store, &svc.llm, &[p], None)
        .await
        .unwrap()
        .remove(0)
}

fn trait_row(svc: &morpho::Services, index: usize) -> serde_json::Value {
    json!(svc.store.lock().unwrap().state.table("traits").rows[index])
}

fn trait_id(svc: &morpho::Services, index: usize) -> String {
    trait_row(svc, index)["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn seeds_are_idempotent_and_render_as_identity() {
    let svc = services("traits-seed");
    let seed = [
        json!({"kind":"value","statement":"I prefer honesty over comfort.","confidence":0.9}),
        json!({"kind":"relationship","statement":"I enjoy Bob's bluntness.","speaker":"bob"}),
    ];
    assert_eq!(seed_traits(&svc, &seed).await.unwrap(), 2);
    assert_eq!(seed_traits(&svc, &seed).await.unwrap(), 0);
    let system = system_prompt(&svc, RESPONSE_SYSTEM, None);
    assert!(system.starts_with(RESPONSE_SYSTEM));
    assert!(system.contains("## IDENTITY"));
    assert!(system.contains("[value] I prefer honesty over comfort."));
    assert!(system.contains("[relationship with bob] I enjoy Bob's bluntness."));
    let meta = morpho::context::composer::identity(&svc.store.lock().unwrap(), None).meta;
    assert_eq!(meta.len(), 2);
    assert_eq!(trait_row(&svc, 0)["origin"], "seed");
    let bad = json!({"kind":"relationship","statement":"no speaker"});
    assert!(seed_traits(&services("traits-bad"), &[bad]).await.is_err());
}

#[tokio::test]
async fn traits_move_by_evidence_in_bounded_steps() {
    let svc = services("traits-steps");
    let seed =
        json!({"kind":"stance","statement":"Remote work beats office mandates.","confidence":0.9});
    seed_traits(&svc, &[seed]).await.unwrap();
    let e1 = event(&svc, "You're wrong, admit it.");
    let seed_evidence = trait_row(&svc, 0)["evidence"].clone();
    let r = one(
        &svc,
        Proposal::new("interaction", "update_trait", json!({"confidence":0.1}))
            .target(&trait_id(&svc, 0))
            .evidence(
                seed_evidence
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().into())
                    .collect(),
            ),
    )
    .await;
    assert_eq!(
        r.reason.as_deref(),
        Some("lowering a trait requires a new contrary observation")
    );
    let r = one(
        &svc,
        Proposal::new(
            "interaction",
            "update_trait",
            json!({"confidence":0.1, "add_contradicting":[e1.clone()]}),
        )
        .target(&trait_id(&svc, 0))
        .evidence(vec![e1.clone()]),
    )
    .await;
    assert!(r.accepted);
    let row = trait_row(&svc, 0);
    assert_eq!(row["confidence"], 0.65);
    assert_eq!(row["contradicting_evidence"], json!([e1]));
    let e2 = event(&svc, "Still wrong.");
    let r = one(
        &svc,
        Proposal::new("reflection", "update_trait", json!({"status":"retired"}))
            .target(&trait_id(&svc, 0))
            .evidence(vec![e2.clone()]),
    )
    .await;
    assert_eq!(
        r.reason.as_deref(),
        Some("retiring a trait requires confidence at or below 0.3")
    );
    let e3 = event(&svc, "Study shows hybrid teams ship more.");
    for e in [e2, e3.clone()] {
        one(
            &svc,
            Proposal::new("reflection", "update_trait", json!({"confidence":0.0}))
                .target(&trait_id(&svc, 0))
                .evidence(vec![e]),
        )
        .await;
    }
    assert_eq!(trait_row(&svc, 0)["confidence"], 0.15);
    let r = one(
        &svc,
        Proposal::new("reflection", "update_trait", json!({"status":"retired"}))
            .target(&trait_id(&svc, 0))
            .evidence(vec![e3.clone()]),
    )
    .await;
    assert!(r.accepted);
    assert!(trait_row(&svc, 0)["valid_until"].is_string());
    assert_eq!(system_prompt(&svc, RESPONSE_SYSTEM, None), RESPONSE_SYSTEM);
    let r = one(
        &svc,
        Proposal::new("reflection", "create_trait", json!({"kind":"stance","statement":"Hybrid work can beat fully remote.","confidence":0.6}))
            .evidence(vec![e3.clone()]),
    )
    .await;
    assert!(r.accepted);
    let r = one(
        &svc,
        Proposal::new("interaction", "create_trait", json!({"kind":"stance","statement":"hybrid work can beat fully remote.","confidence":0.8}))
            .evidence(vec![e3]),
    )
    .await;
    assert!(r.accepted);
    assert!(r.reason.unwrap().starts_with("folded into"));
    assert_eq!(trait_row(&svc, 1)["confidence"], 0.8);
    assert_eq!(trait_row(&svc, 1)["origin"], "experienced");
}

#[tokio::test]
async fn restated_traits_fold_and_rewording_needs_a_new_observation() {
    let svc = services("traits-fold");
    seed_traits(
        &svc,
        &[json!({"kind":"preference","statement":"I enjoy Rust programming very much","confidence":0.7})],
    )
    .await
    .unwrap();
    let e = event(&svc, "Rust again?");
    let r = one(
        &svc,
        Proposal::new(
            "reflection",
            "create_trait",
            json!({"kind":"preference","statement":"I enjoy Rust programming very much indeed","confidence":0.9}),
        )
        .evidence(vec![e.clone()]),
    )
    .await;
    assert!(r.accepted);
    assert!(r.reason.unwrap().starts_with("folded into"));
    assert_eq!(
        svc.store.lock().unwrap().state.table("traits").rows.len(),
        1
    );
    assert_eq!(trait_row(&svc, 0)["confidence"], 0.9);
    let seed_evidence: Vec<String> = trait_row(&svc, 0)["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect();
    let r = one(
        &svc,
        Proposal::new(
            "reflection",
            "update_trait",
            json!({"statement":"I enjoy Rust when the tooling cooperates"}),
        )
        .target(&trait_id(&svc, 0))
        .evidence(seed_evidence),
    )
    .await;
    assert_eq!(
        r.reason.as_deref(),
        Some("revising a trait requires a new contrary observation")
    );
    let e2 = event(&svc, "You spent the week fighting the build.");
    let r = one(
        &svc,
        Proposal::new(
            "reflection",
            "update_trait",
            json!({"statement":"I enjoy Rust when the tooling cooperates"}),
        )
        .target(&trait_id(&svc, 0))
        .evidence(vec![e2]),
    )
    .await;
    assert!(r.accepted);
    assert_eq!(
        trait_row(&svc, 0)["statement"],
        "I enjoy Rust when the tooling cooperates"
    );
}

#[tokio::test]
async fn narrative_notes_and_wants_render_in_identity() {
    let svc = services("identity-layers");
    seed_traits(
        &svc,
        &[json!({"kind":"preference","statement":"I prefer tea to coffee.","confidence":0.9})],
    )
    .await
    .unwrap();
    fake(&svc).queue_text("I am a tea person who says what I think.");
    compile_narrative(&svc).await.unwrap();
    assert_eq!(fake(&svc).calls_for("text").len(), 1);
    compile_narrative(&svc).await.unwrap();
    assert_eq!(fake(&svc).calls_for("text").len(), 1);
    let system = system_prompt(&svc, RESPONSE_SYSTEM, None);
    assert!(system.contains("I am a tea person who says what I think."));
    assert!(system.contains("Traits in play:\n- trait_"));
    let e = event(&svc, "hello");
    let r = one(
        &svc,
        Proposal::new(
            "reflection",
            "create_journal",
            json!({"entry":"Alice pushed twice; I held my ground.","mood":"firm"}),
        )
        .evidence(vec![e.clone()]),
    )
    .await;
    assert!(r.accepted);
    let system = system_prompt(&svc, RESPONSE_SYSTEM, None);
    assert!(system.contains("Lately: Alice pushed twice; I held my ground. (mood: firm)"));
    let (text, manifest) = compose_text_as(&svc, "held my ground", "alice", Some(4000))
        .await
        .unwrap();
    assert!(text.contains("## MY NOTES"));
    assert_eq!(manifest["sections"]["journal"]["included"], 1);
    let user_goal = one(
        &svc,
        Proposal::new(
            "interaction",
            "create_goal",
            json!({"description":"Book Alice's vet","priority":0.9,"origin":"user"}),
        )
        .evidence(vec![e.clone()]),
    )
    .await;
    let r = one(
        &svc,
        Proposal::new("reflection", "update_goal", json!({"next_step":"call"}))
            .target(&user_goal.object_ids[0])
            .evidence(vec![e.clone()]),
    )
    .await;
    assert_eq!(
        r.reason.as_deref(),
        Some("reflection may only revise its own goals")
    );
    let own = one(
        &svc,
        Proposal::new(
            "reflection",
            "create_goal",
            json!({"description":"Find a better Assam","priority":0.4}),
        )
        .evidence(vec![trait_id(&svc, 0)]),
    )
    .await;
    assert!(own.accepted);
    let r = one(
        &svc,
        Proposal::new(
            "reflection",
            "update_goal",
            json!({"next_step":"Ask Alice which shop she uses"}),
        )
        .target(&own.object_ids[0])
        .evidence(vec![e]),
    )
    .await;
    assert!(r.accepted);
    let system = system_prompt(&svc, RESPONSE_SYSTEM, None);
    assert!(system.contains("On my mind: Find a better Assam Next: Ask Alice which shop she uses"));
    // Reflection rewording the trait makes the narrative stale again.
    fake(&svc).queue_text("I am a tea person, and I keep my word.");
    let e3 = event(&svc, "You never budge on tea.");
    one(
        &svc,
        Proposal::new(
            "reflection",
            "update_trait",
            json!({"statement":"I prefer tea, always."}),
        )
        .target(&trait_id(&svc, 0))
        .evidence(vec![e3]),
    )
    .await;
    compile_narrative(&svc).await.unwrap();
    assert_eq!(fake(&svc).calls_for("text").len(), 2);
    assert_eq!(svc.store.lock().unwrap().state.narrative["version"], 3);
}

#[tokio::test]
async fn only_substance_restales_the_narrative() {
    let svc = services("substance");
    seed_traits(
        &svc,
        &[json!({"kind":"stance","statement":"Remote work beats office mandates."})],
    )
    .await
    .unwrap();
    fake(&svc).queue_text("I prefer remote work.");
    compile_narrative(&svc).await.unwrap();
    assert!(!narrative::stale(&svc.store.lock().unwrap().state));
    let id = svc.store.lock().unwrap().state.table("traits").rows[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let study = event(
        &svc,
        "My forty-team study found hybrid teams shipped 30% more.",
    );
    let revise = |payload: serde_json::Value, evidence: &str| {
        Proposal::new("interaction", "update_trait", payload)
            .target(&id)
            .evidence(vec![evidence.to_string()])
    };
    let r = commit(
        &svc.store,
        &svc.llm,
        &[revise(json!({"confidence":0.95}), &study)],
        None,
    )
    .await
    .unwrap()
    .remove(0);
    assert!(r.accepted);
    assert!(!narrative::stale(&svc.store.lock().unwrap().state)); // confidence changes no sentence
    let r = commit(
        &svc.store,
        &svc.llm,
        &[revise(json!({"add_contradicting":[study.clone()]}), &study)],
        None,
    )
    .await
    .unwrap()
    .remove(0);
    assert!(r.accepted);
    assert!(narrative::stale(&svc.store.lock().unwrap().state));
}
