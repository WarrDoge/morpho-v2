mod common;
use common::services;
use morpho::{
    context::composer::compose_text_as,
    state::{engine::commit, models::Proposal},
};
use serde_json::json;

/// The two borrowed mechanisms, both off by default: a stream of what is known about whoever
/// is speaking, and the text of the events a belief cites rather than only their ids.
#[tokio::test]
async fn speaker_stream_and_premises_reach_the_prompt_when_enabled() {
    unsafe {
        std::env::set_var("SPEAKER_SHARE", "0.15");
        std::env::set_var("PREMISE_CHARS", "200");
    }
    let svc = services("honcho");
    let say = |source: &str, text: &str| {
        let mut st = svc.store.lock().unwrap();
        st.append_event("user_message", source, json!({"text": text}), None, None)
            .unwrap()["event_id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let said = say("bob", "my brother bakes sourdough on saturdays");
    let work = say("dana", "the alpha rollout is on track");
    let mut proposals: Vec<Proposal> = (0..25)
        .map(|i| {
            let mut p = Proposal::new(
                "test",
                "create_memory",
                json!({"summary": format!("alpha deployment note number {i}"),
                       "kind": "semantic", "importance": 0.5, "confidence": 0.9}),
            );
            p.evidence = vec![work.clone()];
            p
        })
        .collect();
    for (text, kind) in [
        ("brother bakes sourdough", "create_memory"),
        ("mother repairs mantel clocks", "create_memory"),
    ] {
        let mut p = Proposal::new(
            "test",
            kind,
            json!({"summary": text, "kind": "semantic", "importance": 0.5, "confidence": 0.9}),
        );
        p.evidence = vec![said.clone()];
        proposals.push(p);
    }
    let mut belief = Proposal::new(
        "test",
        "create_belief",
        json!({"proposition": "alpha ships weekly", "confidence": 0.8, "status": "active"}),
    );
    belief.evidence = vec![said.clone()];
    proposals.push(belief);
    commit(&svc.store, &svc.llm, &proposals, None)
        .await
        .unwrap();

    let (text, manifest) = compose_text_as(&svc, "alpha", "bob", Some(4000))
        .await
        .unwrap();

    // Cosine puts twenty alpha memories first; the speaker stream carries what it skipped.
    assert!(
        manifest["sections"]["speaker"]["included"]
            .as_u64()
            .unwrap()
            > 0,
        "speaker stream empty: {manifest}"
    );
    assert!(text.contains("WHO I'M TALKING TO"));
    assert!(text.contains("sourdough") || text.contains("clocks"));
    // The belief arrives with the text of the event it cites, not just the id.
    assert!(
        text.contains("grounds"),
        "no premises on the belief: {text}"
    );
    assert!(text.contains("saturdays"));
}
