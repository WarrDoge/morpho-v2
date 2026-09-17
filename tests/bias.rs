mod common;
use common::{event, services};
use morpho::agents::seed::seed_traits;
use morpho::context::composer::{compose_text_as, identity};
use morpho::state::engine::commit;
use morpho::state::models::Proposal;
use serde_json::json;

#[tokio::test]
async fn identity_biases_recall_and_ranks_traits_by_relevance() {
    unsafe { std::env::set_var("IDENTITY_BIAS", "8") };
    assert_eq!(morpho::config::settings().identity_bias, 8.0);
    let svc = services("bias");
    let mut seed: Vec<_> = (0..8)
        .map(|i| json!({"kind":"value","statement":format!("tea matters to me, value {i}"),"confidence":0.9}))
        .collect();
    seed.push(json!({"kind":"preference","statement":"tea tea tea is my thing","confidence":0.5}));
    seed.push(json!({"kind":"preference","statement":"quiet mornings","confidence":0.5}));
    seed_traits(&svc, &seed).await.unwrap();
    let e = event(&svc, "notes");
    let mems = [
        Proposal::new(
            "interaction",
            "create_memory",
            json!({"summary":"bridge in rotterdam"}),
        )
        .evidence(vec![e.clone()]),
        Proposal::new(
            "interaction",
            "create_memory",
            json!({"summary":"tea harvest in assam"}),
        )
        .evidence(vec![e.clone()]),
    ];
    commit(&svc.store, &svc.llm, &mems, None).await.unwrap();
    let (_, manifest) = compose_text_as(&svc, "rotterdam bridge", "alice", Some(4000))
        .await
        .unwrap();
    let first = manifest["memories"][0]["id"].as_str().unwrap();
    let row = json!(
        svc.store
            .lock()
            .unwrap()
            .state
            .get("memories", first)
            .unwrap()
    );
    assert_eq!(row["summary"], "tea harvest in assam");
    let q = morpho::llm::hash_embedding("tea", 1024);
    let meta = identity(&svc.store.lock().unwrap(), Some(&q)).meta;
    assert_eq!(meta.len(), 10);
    assert!(meta.iter().all(|m| m["selection_reason"] == "relevant"));
    let tea = svc.store.lock().unwrap().state.table("traits").rows[8]["id"].clone();
    assert_eq!(meta[0]["id"], tea);
}
