mod common;
use common::{fake, services, temp_dir};
use morpho::{
    Services,
    ids::IdGen,
    interact::{enqueue, interact_as, process_next, request},
    llm::{Fake, Llm},
    store::Store,
};
use serde_json::json;

#[tokio::test]
async fn fifo_is_private_and_duplicate_reply_survives_restart() {
    let svc = services("inbox-fifo");
    let lock = svc.cycle_lock.lock().await;
    enqueue(&svc, "Alice's input", Some("a"), "alice", Some("a1")).unwrap();
    enqueue(&svc, "Bob's input", Some("b"), "bob", Some("b1")).unwrap();
    assert!(svc.store.lock().unwrap().state.events.is_empty());
    assert!(enqueue(&svc, "different", Some("a"), "alice", Some("a1")).is_err());
    fake(&svc)
        .delay_ms
        .store(20, std::sync::atomic::Ordering::Relaxed);
    let work = process_next(&svc);
    let inspection = async {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        assert!(svc.store.lock().unwrap().state.events.is_empty());
        assert_eq!(request(&svc, "a1").unwrap()["status"], "pending");
    };
    let (done, _) = tokio::join!(work, inspection);
    assert!(done.unwrap());
    assert!(!fake(&svc).calls_for("Turn")[0].contains("Bob's input"));
    assert!(process_next(&svc).await.unwrap());
    let calls = fake(&svc).calls_for("Turn");
    assert!(calls[1].contains("alice/user_message"));
    assert!(calls[1].contains("Speaker: bob"));
    assert_eq!(calls.len(), 2); // one model call per turn
    let reply = request(&svc, "a1").unwrap()["reply"].clone();
    drop(lock);
    assert_eq!(
        interact_as(&svc, "Alice's input", Some("a"), "alice", Some("a1"))
            .await
            .unwrap(),
        reply
    );
    assert_eq!(fake(&svc).calls_for("Turn").len(), 2);
    drop(svc);
    let svc = Services::new(
        Store::open(&temp_dir("inbox-fifo"), IdGen::random()).unwrap(),
        Llm::Fake(Fake::default()),
    );
    assert_eq!(
        interact_as(&svc, "Alice's input", Some("a"), "alice", Some("a1"))
            .await
            .unwrap(),
        reply
    );
    assert!(fake(&svc).calls_for("Turn").is_empty());
    assert_eq!(svc.store.lock().unwrap().state.events.len(), 4);
}

#[tokio::test]
async fn failed_or_cancelled_turn_keeps_input_and_retries_once() {
    let svc = services("inbox-failure");
    enqueue(&svc, "remember this", None, "alice", Some("retry")).unwrap();
    fake(&svc).queue("Turn",json!({"response":"draft", "changes":[{"operation":"create_memory","target":null,"payload":"not json","evidence_ids":[],"confidence":0.8,"reason":"test"}]}));
    assert!(process_next(&svc).await.is_err());
    assert_eq!(request(&svc, "retry").unwrap()["attempts"], 1);
    assert_eq!(svc.store.lock().unwrap().state.events.len(), 1); // observed failure only
    assert!(svc.store.lock().unwrap().state.transitions.is_empty());
    enqueue(&svc, "remember this", None, "alice", Some("retry")).unwrap();
    fake(&svc)
        .delay_ms
        .store(100, std::sync::atomic::Ordering::Relaxed);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), process_next(&svc))
            .await
            .is_err()
    );
    assert_eq!(request(&svc, "retry").unwrap()["status"], "pending");
    drop(svc);
    let svc = Services::new(
        Store::open(&temp_dir("inbox-failure"), IdGen::random()).unwrap(),
        Llm::Fake(Fake::default()),
    );
    assert!(process_next(&svc).await.unwrap());
    assert_eq!(request(&svc, "retry").unwrap()["reply"]["response"], "ok");
    assert!(!process_next(&svc).await.unwrap());
    assert_eq!(
        svc.store
            .lock()
            .unwrap()
            .state
            .events
            .iter()
            .filter(|e| e["type"] == "user_message")
            .count(),
        1
    );
}

#[tokio::test]
async fn concurrent_callers_share_one_fifo_and_do_not_skip_paused_input() {
    let svc = services("inbox-concurrent");
    fake(&svc)
        .delay_ms
        .store(10, std::sync::atomic::Ordering::Relaxed);
    let (a, b) = tokio::join!(
        interact_as(&svc, "first", None, "a", Some("one")),
        interact_as(&svc, "second", None, "b", Some("two"))
    );
    assert_eq!(a.unwrap()["sequence"], 1);
    assert_eq!(b.unwrap()["sequence"], 2);
    enqueue(&svc, "broken", None, "a", Some("three")).unwrap();
    enqueue(&svc, "later", None, "b", Some("four")).unwrap();
    {
        let st = svc.store.lock().unwrap();
        futures_executor::block_on(
            st.db
                .execute("UPDATE inbox SET attempts=3 WHERE request_id='three'", ()),
        )
        .unwrap();
    }
    assert!(!process_next(&svc).await.unwrap());
    assert_eq!(request(&svc, "three").unwrap()["status"], "paused");
    assert_eq!(request(&svc, "four").unwrap()["status"], "pending");
}

fn native_change(_system: &str, user: &str, _schema: &str) -> serde_json::Value {
    let eid = user
        .lines()
        .find_map(|line| line.strip_prefix("Input evidence id: "))
        .unwrap();
    json!({"response":"saved","changes":[
        {"operation":"create_memory","target":null,"payload":{"summary":"Alice moved to Berlin"},"evidence_ids":[eid],"confidence":0.9,"reason":"Alice explicitly said so"},
        {"operation":"set_working_state","target":null,"payload":{"patch":{"current_topic":"Alice's move"}},"evidence_ids":[eid],"confidence":0.9,"reason":"current subject"}
    ]})
}

#[tokio::test]
async fn native_json_changes_and_reply_publish_together() {
    let svc = services("native-turn");
    *fake(&svc).respond.lock().unwrap() = Some(native_change);
    let reply = interact_as(&svc, "I moved to Berlin", None, "alice", Some("native"))
        .await
        .unwrap();
    assert_eq!(reply["response"], "saved");
    let live = svc.store.lock().unwrap().state.clone();
    assert_eq!(live.table("memories").rows.len(), 1);
    assert_eq!(live.working["data"]["current_topic"], "Alice's move");
    assert_eq!(live.proposals[0]["rationale"], "Alice explicitly said so");
    drop(svc);
    let svc = Services::new(
        Store::open(&temp_dir("native-turn"), IdGen::random()).unwrap(),
        Llm::Fake(Fake::default()),
    );
    assert_eq!(svc.store.lock().unwrap().state, live);
    assert_eq!(request(&svc, "native").unwrap()["reply"], reply);
}
