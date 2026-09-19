mod common;
use common::{event, services, temp_dir};
use morpho::{
    ids::IdGen,
    snapshots::replay,
    state::{engine::commit, models::Proposal},
    store::{Commit, Record, Store, journal::Entry, obj, vectors::bytes},
};
use serde_json::json;

#[test]
fn imports_legacy_vectors_and_torn_utf8_without_modifying_sources() {
    let dir = temp_dir("import");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dim = morpho::config::settings().embed_dim;
    let vector = vec![0.5; dim];
    let row = obj(
        json!({"id":1,"event_id":"evt_old","type":"user_message","source":"user","payload":{"text":"original"},"vector":0}),
    );
    let e = Entry {
        seq: 1,
        record: Record::Event(row),
    };
    let mut journal = serde_json::to_vec(&e).unwrap();
    journal.extend_from_slice(b"\n{\"torn\":\"\xf0\x9f");
    let mut vectors = bytes(&vector);
    vectors.push(1);
    std::fs::write(dir.join("journal.jsonl"), &journal).unwrap();
    std::fs::write(dir.join("vectors.f32"), &vectors).unwrap();
    let st = Store::open(&dir, IdGen::random()).unwrap();
    assert_eq!(st.state.last_event_id(), 1);
    assert_eq!(st.vectors.get(0).unwrap(), vector);
    assert!((st.vectors.similarity_between(0, 0).unwrap() - 1.0).abs() < 1e-6);
    assert!(Store::open(&dir, IdGen::random()).is_err()); // exclusive coordinator ownership
    drop(st);
    assert_eq!(std::fs::read(dir.join("journal.jsonl")).unwrap(), journal);
    assert_eq!(std::fs::read(dir.join("vectors.f32")).unwrap(), vectors);
    let st = Store::open(&dir, IdGen::random()).unwrap();
    assert_eq!(st.state.last_event_id(), 1); // migration is once-only
    st.set_metadata("embedding", "wrong-model:7").unwrap();
    drop(st);
    assert!(Store::open(&dir, IdGen::random()).is_err());
}

#[tokio::test]
async fn failed_atomic_publish_rolls_back_records_vectors_and_reply() {
    let svc = services("atomic");
    morpho::interact::enqueue(&svc, "input", None, "a", Some("r")).unwrap();
    let staged = svc.staged();
    let eid = event(&staged, "private");
    commit(
        &staged.store,
        &staged.llm,
        &[Proposal::new(
            "interaction",
            "create_memory",
            json!({"summary":"private memory"}),
        )
        .evidence(vec![eid])],
        None,
    )
    .await
    .unwrap();
    {
        let st = svc.store.lock().unwrap();
        futures_executor::block_on(st.db.execute_batch("CREATE TRIGGER fail_reply BEFORE UPDATE OF reply ON inbox BEGIN SELECT RAISE(ABORT,'simulated commit failure'); END;")).unwrap();
    }
    assert!(
        svc.publish(staged, Some(("r", &json!({"response":"draft"}))))
            .is_err()
    );
    assert!(svc.store.lock().unwrap().state.events.is_empty());
    assert!(morpho::interact::request(&svc, "r").unwrap()["reply"].is_null());
    drop(svc);
    let st = Store::open(&temp_dir("atomic"), IdGen::random()).unwrap();
    assert_eq!(st.journal.seq, 0);
    assert_eq!(st.vectors.slots, 0);
    assert!(st.state.events.is_empty());
}

#[tokio::test]
async fn historical_replay_stops_events_and_vector_versions_at_boundary() {
    let svc = services("history");
    let eid = event(&svc, "old evidence");
    let p = Proposal::new(
        "interaction",
        "create_memory",
        json!({"summary":"old text"}),
    )
    .evidence(vec![eid.clone()]);
    let id = commit(&svc.store, &svc.llm, &[p], None).await.unwrap()[0].object_ids[0].clone();
    let slot = svc.store.lock().unwrap().state.slots[&id];
    event(&svc, "future evidence");
    let p = Proposal::new(
        "interaction",
        "update_memory",
        json!({"summary":"future text"}),
    )
    .target(&id)
    .evidence(vec![eid]);
    commit(&svc.store, &svc.llm, &[p], None).await.unwrap();
    let live = svc.store.lock().unwrap().state.clone();
    let (old, n) = replay(&temp_dir("history"), Some(1)).unwrap();
    assert_eq!(n, 1);
    assert_eq!(old.events.len(), 1);
    assert_eq!(old.get("memories", &id).unwrap()["summary"], "old text");
    assert_eq!(old.slots[&id], slot);
    let (full, _) = replay(&temp_dir("history"), None).unwrap();
    assert_eq!(full, live);
}

#[test]
fn corrupt_complete_legacy_record_fails_without_partial_import() {
    let dir = temp_dir("bad-import");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("journal.jsonl"), b"not-json\n").unwrap();
    assert!(Store::open(&dir, IdGen::random()).is_err());
    assert_eq!(
        std::fs::read(dir.join("journal.jsonl")).unwrap(),
        b"not-json\n"
    );
    std::fs::write(
        dir.join("journal.jsonl"),
        format!(
            "{}\n",
            serde_json::to_string(&Entry {
                seq: 1,
                record: Record::Commit(Commit::default())
            })
            .unwrap()
        ),
    )
    .unwrap();
    assert!(Store::open(&dir, IdGen::random()).is_ok());
}
