mod common;
use common::services;
use morpho::{
    agents::reflection::{Batch, run},
    llm::{Fake, Llm},
    state::{engine::commit, models::Proposal},
};
use serde_json::json;

/// Reflection may return three changes and spends them on what it reads first. With
/// `REFLECT_SURPRISAL_TOP` set, the observation furthest from everything already stored
/// leads the batch instead of arriving in timestamp order.
#[tokio::test]
async fn the_novel_observation_leads_the_batch() {
    unsafe { std::env::set_var("REFLECT_SURPRISAL_TOP", "0.25") };
    let svc = services("surprisal");
    let source = {
        let mut st = svc.store.lock().unwrap();
        st.append_event(
            "user_message",
            "user",
            json!({"text": "alpha rollout status"}),
            None,
            None,
        )
        .unwrap()["event_id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // Enough stored rows to clear the sample guard, all about one subject.
    let stored: Vec<Proposal> = (0..12)
        .map(|i| {
            let mut p = Proposal::new(
                "test",
                "create_memory",
                json!({"summary": format!("alpha rollout deployment stage {i}"),
                       "kind": "semantic", "importance": 0.5, "confidence": 0.9}),
            );
            p.evidence = vec![source.clone()];
            p
        })
        .collect();
    commit(&svc.store, &svc.llm, &stored, None).await.unwrap();

    let start = svc.store.lock().unwrap().state.events.len();
    for text in [
        "alpha rollout deployment stage nineteen",
        "alpha rollout deployment stage twenty",
        "penguins nesting in the boiler room",
        "alpha rollout deployment stage twentyone",
    ] {
        let emb = svc.llm.embed(&[text.to_owned()]).await.unwrap().remove(0);
        let mut st = svc.store.lock().unwrap();
        let slot = st.vectors.append(&emb).unwrap();
        st.append_event(
            "user_message",
            "user",
            json!({ "text": text }),
            None,
            Some(slot),
        )
        .unwrap();
    }
    let end = svc.store.lock().unwrap().state.events.len();
    let transitions = svc.store.lock().unwrap().state.transitions.len();
    run(
        &svc,
        &Batch {
            event_start: start,
            event_end: end,
            transition_start: transitions,
            transition_end: transitions,
        },
    )
    .await
    .unwrap();

    let Llm::Fake(fake) = svc.llm.as_ref() else {
        unreachable!()
    };
    let prompt = last_reflection(fake);
    let odd = prompt
        .find("penguins")
        .expect("batch missing the odd event");
    let first = prompt
        .find("stage nineteen")
        .expect("batch missing an event");
    assert!(odd < first, "arrival order kept: {prompt}");
}

fn last_reflection(fake: &Fake) -> String {
    fake.calls
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(schema, _)| schema == "Reflection")
        .map(|(_, text)| text.clone())
        .expect("reflection was never called")
}
