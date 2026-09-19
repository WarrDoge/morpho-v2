mod common;
use common::{event, fake, services};
use morpho::agents::seed::seed_traits;
use morpho::context::composer::compose_text_as;
use morpho::interact::{interact_as, looks_truncated, unusable};
use morpho::state::engine::commit;
use morpho::state::models::Proposal;
use serde_json::json;

#[tokio::test]
async fn own_replies_return_by_similarity_across_sessions() {
    let svc = services("said");
    fake(&svc).queue_text("Tea is my favourite drink.");
    fake(&svc).queue_text("Bridges are long.");
    interact_as(&svc, "what do you drink?", Some("s1"), "alice", None)
        .await
        .unwrap();
    interact_as(&svc, "tell me about bridges", Some("s1"), "alice", None)
        .await
        .unwrap();
    let (text, manifest) = compose_text_as(&svc, "favourite drink tea", "bob", Some(4000))
        .await
        .unwrap();
    assert!(text.contains("## WHAT I SAID"));
    let said = manifest["said"].as_array().unwrap();
    assert_eq!(said.len(), 2);
    assert_eq!(manifest["sections"]["recent"]["included"], 0);
    let body = interact_as(&svc, "drink?", Some("s2"), "bob", None)
        .await
        .unwrap();
    let ids: Vec<_> = body["context"]["said"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].clone())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(fake(&svc).calls_for("text")[2].contains("Tea is my favourite drink."));
}

#[tokio::test]
async fn pooled_recall_drops_a_restated_fact_and_boosts_named_entities() {
    let svc = services("pool");
    let e = event(&svc, "notes");
    let rows = [
        Proposal::new(
            "interaction",
            "create_memory",
            json!({"summary":"alice moved to berlin last year"}),
        )
        .evidence(vec![e.clone()]),
        Proposal::new(
            "interaction",
            "create_belief",
            json!({"proposition":"alice moved to berlin last year indeed"}),
        )
        .evidence(vec![e.clone()]),
        Proposal::new(
            "interaction",
            "create_memory",
            json!({"summary":"tell me a story"}),
        )
        .evidence(vec![e.clone()]),
        Proposal::new(
            "interaction",
            "create_memory",
            json!({"summary":"about bob and stamps"}),
        )
        .evidence(vec![e.clone()]),
        Proposal::new(
            "interaction",
            "upsert_entity",
            json!({"name":"Bob","kind":"person"}),
        )
        .evidence(vec![e.clone()]),
    ];
    let ids = commit(&svc.store, &svc.llm, &rows, None).await.unwrap();
    let bob = ids[4].object_ids[0].clone();
    commit(
        &svc.store,
        &svc.llm,
        &[Proposal::new(
            "interaction",
            "update_memory",
            json!({"add_entity_ids":[bob]}),
        )
        .target(&ids[3].object_ids[0])
        .evidence(vec![e])],
        None,
    )
    .await
    .unwrap();
    let (_, manifest) = compose_text_as(&svc, "alice moved to berlin", "alice", Some(4000))
        .await
        .unwrap();
    let omitted: Vec<_> = ["memories", "beliefs"]
        .iter()
        .flat_map(|sec| {
            manifest["sections"][*sec]["items"]
                .as_array()
                .unwrap()
                .clone()
        })
        .filter(|i| i["included"] == false)
        .collect();
    assert_eq!(omitted.len(), 1);
    assert!(
        omitted[0]["omission_reason"]
            .as_str()
            .unwrap()
            .starts_with("duplicate of ")
    );
    // Equal word overlap with the input; the memory linked to the named entity wins.
    let (_, manifest) = compose_text_as(&svc, "tell me about bob then", "cara", Some(4000))
        .await
        .unwrap();
    assert_eq!(manifest["memories"][0]["id"], ids[3].object_ids[0]);
}

#[tokio::test]
async fn self_goals_cite_a_trait_and_mood_is_working_state() {
    let svc = services("self-goal");
    seed_traits(
        &svc,
        &[json!({"kind":"preference","statement":"I enjoy Rust."})],
    )
    .await
    .unwrap();
    let trait_id = svc.store.lock().unwrap().state.table("traits").rows[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let e = event(&svc, "hi");
    let r = commit(
        &svc.store,
        &svc.llm,
        &[Proposal::new(
            "reflection",
            "create_goal",
            json!({"description":"Read the Rust book","priority":0.4}),
        )
        .evidence(vec![e.clone()])],
        None,
    )
    .await
    .unwrap()
    .remove(0);
    assert_eq!(
        r.reason.as_deref(),
        Some("a goal of your own must cite the trait it follows from")
    );
    let r = commit(
        &svc.store,
        &svc.llm,
        &[Proposal::new(
            "reflection",
            "create_goal",
            json!({"description":"Read the Rust book","priority":0.4}),
        )
        .evidence(vec![trait_id])],
        None,
    )
    .await
    .unwrap()
    .remove(0);
    assert!(r.accepted);
    let goal = json!(svc.store.lock().unwrap().state.table("goals").rows[0]);
    assert_eq!(goal["origin"], "self");
    let r = commit(
        &svc.store,
        &svc.llm,
        &[Proposal::new(
            "interaction",
            "set_working_state",
            json!({"mood":"curious"}),
        )
        .evidence(vec![e])],
        None,
    )
    .await
    .unwrap()
    .remove(0);
    assert!(r.accepted);
    assert_eq!(
        svc.store.lock().unwrap().state.working["data"]["mood"],
        "curious"
    );
    assert!(looks_truncated("Here is the list:"));
    assert!(looks_truncated("a practice called \n"));
    assert!(!looks_truncated("Done.\n"));
    assert!(looks_truncated("text"));
    assert!(looks_truncated("{"));
    assert!(!looks_truncated("Tea."));
    assert!(looks_truncated(" adrenaline_alert_0f0a8c92d75b"));
    assert!(!looks_truncated("see https://example.org/a b."));
    assert!(looks_truncated("I didn't say"));
    assert!(looks_truncated("I'm not retracting it \u{2014} but"));
    assert!(!unusable("I'm not retracting it \u{2014} but"));
    assert!(unusable(" adrenaline_alert_0f0a8c92d75b"));
    assert!(unusable("text"));
    assert!(looks_truncated("text}{"));
}
