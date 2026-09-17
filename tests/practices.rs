mod common;
use common::{event, services};
use morpho::agents::narrative;
use morpho::agents::seed::seed_traits;
use morpho::context::composer::identity;
use morpho::state::engine::commit;
use morpho::state::models::Proposal;
use serde_json::json;

#[tokio::test]
async fn dropped_practices_leave_identity_and_the_narrative() {
    unsafe { std::env::set_var("MORPHO_DROP_STREAMS", "practices") };
    let svc = services("practices-dropped");
    seed_traits(
        &svc,
        &[json!({"kind":"style","statement":"I test before I report."})],
    )
    .await
    .unwrap();
    let obs = event(&svc, "exit 0");
    let r = commit(
        &svc.store,
        &svc.llm,
        &[Proposal::new(
            "practice",
            "create_trait",
            json!({"kind": "practice", "statement": "When a module is missing, I set PYTHONPATH.", "confidence": 0.8}),
        )
        .evidence(vec![obs])],
        None,
    )
    .await
    .unwrap()
    .remove(0);
    assert!(r.accepted, "{:?}", r.reason);
    let st = svc.store.lock().unwrap();
    let block = identity(&st, None).block;
    assert!(block.contains("I test before I report."));
    assert!(!block.contains("PYTHONPATH") && !block.contains("How I work"));
    assert!(!narrative::sources(&st.state).contains_key(&r.object_ids[0]));
}
