//! Engine guards the strict replay never exercises, plus fold-equals-live through the engine path.

mod common;

use chrono::Duration;
use common::{event, services};
use morpho::agents::consolidation::decay_proposals;
use morpho::ids::IdGen;
use morpho::pyfmt::{iso, now};
use morpho::state::engine::commit;
use morpho::state::models::Proposal;
use morpho::store::{Store, obj};
use serde_json::json;

async fn one(svc: &morpho::Services, p: Proposal) -> morpho::state::engine::CommitResult {
    commit(&svc.store, &svc.llm, &[p], None)
        .await
        .unwrap()
        .remove(0)
}

#[tokio::test]
async fn rejections_are_recorded_without_transitions() {
    let svc = services("rejections");
    let p = Proposal::new("memory", "create_memory", json!({"summary": "x"}));
    let r = one(&svc, p).await;
    assert!(!r.accepted);
    assert_eq!(r.reason.as_deref(), Some("evidence required"));
    let eid = event(&svc, "hi");
    let p = Proposal::new(
        "memory",
        "create_memory",
        json!({"summary": "x", "importance": 2}),
    )
    .evidence(vec![eid.clone()]);
    let r = one(&svc, p).await;
    assert!(!r.accepted);
    assert!(r.reason.unwrap().contains("importance"));
    let p =
        Proposal::new("memory", "update_memory", json!({"reinforce": true})).evidence(vec![eid]);
    assert_eq!(
        one(&svc, p).await.reason.as_deref(),
        Some("target required")
    );
    let st = svc.store.lock().unwrap();
    assert_eq!(st.state.proposals.len(), 3);
    assert!(
        st.state
            .proposals
            .iter()
            .all(|p| p["decision"] == "rejected")
    );
    assert!(st.state.transitions.is_empty());
}

#[tokio::test]
async fn optimistic_concurrency() {
    let svc = services("version");
    let eid = event(&svc, "hi");
    let p = Proposal::new(
        "memory",
        "create_memory",
        json!({"summary": "Rex is a dog"}),
    )
    .evidence(vec![eid.clone()]);
    let mid = one(&svc, p).await.object_ids.remove(0);
    let upd = || {
        Proposal::new(
            "memory",
            "update_memory",
            json!({"importance": 0.9, "expected_version": 1}),
        )
        .target(&mid)
        .evidence(vec![eid.clone()])
    };
    assert!(one(&svc, upd()).await.accepted);
    let second = one(&svc, upd()).await;
    assert!(!second.accepted);
    assert!(second.reason.unwrap().starts_with("version conflict"));
    let st = svc.store.lock().unwrap();
    let row = st.state.get("memories", &mid).unwrap();
    assert_eq!(row["importance"], json!(0.9));
    assert_eq!(row["version"], json!(2));
}

#[tokio::test]
async fn self_model_cannot_grant_capabilities_and_origin_is_coerced() {
    let svc = services("guards");
    let eid = event(&svc, "hi");
    let patch = json!({"patch": {"capabilities": ["root"]}});
    let r = one(
        &svc,
        Proposal::new("self_model", "update_self_state", patch.clone()),
    )
    .await;
    assert!(!r.accepted);
    let r = one(&svc, Proposal::new("harness", "update_self_state", patch)).await;
    assert!(r.accepted);
    let goal = |agent: &str| {
        Proposal::new(
            agent,
            "create_goal",
            json!({"description": format!("g {agent}"), "origin": "user"}),
        )
        .evidence(vec![eid.clone()])
    };
    let inferred = one(&svc, goal("goals")).await.object_ids.remove(0);
    let user = one(&svc, goal("interaction")).await.object_ids.remove(0);
    let st = svc.store.lock().unwrap();
    assert_eq!(st.state.self_state["data"]["capabilities"], json!(["root"]));
    assert_eq!(
        st.state.get("goals", &inferred).unwrap()["origin"],
        "inferred"
    );
    assert_eq!(st.state.get("goals", &user).unwrap()["origin"], "user");
}

#[tokio::test]
async fn duplicates_fold_or_reject_and_state_survives_reopen() {
    let svc = services("dupes");
    let e1 = event(&svc, "hi");
    let e2 = event(&svc, "again");
    let mem = |e: &str| {
        Proposal::new(
            "memory",
            "create_memory",
            json!({"summary": "The user has a dog named Rex"}),
        )
        .evidence(vec![e.to_string()])
    };
    let first = one(&svc, mem(&e1)).await;
    let second = one(&svc, mem(&e2)).await;
    assert!(second.accepted);
    assert_eq!(second.object_ids, first.object_ids);
    assert_eq!(
        second.reason.unwrap(),
        format!("folded into {}", first.object_ids[0])
    );
    let goal = || {
        Proposal::new(
            "interaction",
            "create_goal",
            json!({"description": "Find a vet"}),
        )
        .evidence(vec![e1.clone()])
    };
    assert!(one(&svc, goal()).await.accepted);
    let dup = one(&svc, goal()).await;
    assert!(!dup.accepted);
    assert!(dup.reason.unwrap().starts_with("duplicate of"));

    let dir = {
        let st = svc.store.lock().unwrap();
        let row = st.state.get("memories", &first.object_ids[0]).unwrap();
        assert_eq!(row["access_count"], json!(1));
        assert_eq!(row["source_events"], json!([e1, e2]));
        assert_eq!(
            st.state
                .proposals
                .iter()
                .filter(|p| p["reason"].as_str().is_some_and(|r| r.contains("folded")))
                .count(),
            1
        );
        common::temp_dir("dupes").to_path_buf()
    };
    let live = svc.store.lock().unwrap().state.clone();
    drop(svc);
    let reopened = Store::open(&dir, IdGen::seeded("dupes")).unwrap();
    assert_eq!(reopened.state, live);
}

#[test]
fn decay_is_episodic_only_and_age_based() {
    let now = now();
    let old = iso(&(now - Duration::days(400)));
    let row = |kind: &str, created: &str| {
        obj(
            json!({"id": format!("mem_{kind}"), "kind": kind, "importance": 0.2, "access_count": 0,
            "version": 1, "created_at": created}),
        )
    };
    let rows = [
        row("episodic", &old),
        row("semantic", &old),
        row("fresh", &iso(&now)),
    ];
    let mut rows = rows.to_vec();
    rows[2].insert("kind".into(), json!("episodic"));
    let out = decay_proposals(&rows, &now);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].target.as_deref(), Some("mem_episodic"));
    assert_eq!(out[0].payload["status"], "archived");
}

#[tokio::test]
async fn similar_vectors_do_not_merge_distinct_claims_and_unknown_evidence_is_rejected() {
    let svc = services("distinct-claims");
    let eid = event(&svc, "Alice and Bob");
    let p = |text: &str, evidence: &str| {
        Proposal::new("interaction", "create_belief", json!({"proposition":text}))
            .evidence(vec![evidence.into()])
    };
    // Same bag of words / identical fake embeddings, but different subject and object.
    assert!(one(&svc, p("Alice trusts Bob", &eid)).await.accepted);
    assert!(one(&svc, p("Bob trusts Alice", &eid)).await.accepted);
    assert_eq!(
        svc.store.lock().unwrap().state.table("beliefs").rows.len(),
        2
    );
    let result = one(&svc, p("unfounded claim", "evt_missing")).await;
    assert!(!result.accepted);
    assert_eq!(
        result.reason.as_deref(),
        Some("unknown evidence: evt_missing")
    );
}

#[tokio::test]
async fn flat_state_patches_preserve_version_and_permission_guards() {
    let svc = services("flat-patch");
    let eid = event(&svc, "flat input");
    let p = Proposal::new(
        "interaction",
        "set_working_state",
        json!({"current_topic":"flat input","expected_version":1}),
    )
    .evidence(vec![eid.clone()]);
    assert!(one(&svc, p.clone()).await.accepted);
    assert!(!one(&svc, p).await.accepted);
    assert_eq!(
        svc.store.lock().unwrap().state.working["data"]["current_topic"],
        "flat input"
    );
    assert!(
        !svc.store.lock().unwrap().state.working["data"]
            .as_object()
            .unwrap()
            .contains_key("expected_version")
    );
    let p = Proposal::new(
        "interaction",
        "update_self_state",
        json!({"capabilities":["root"]}),
    )
    .evidence(vec![eid]);
    assert!(!one(&svc, p).await.accepted);
}

#[tokio::test]
async fn entity_attribute_updates_resolve_existing_targets() {
    let svc = services("entity-update");
    let eid = event(&svc, "Nimbus has an office in Berlin");
    let proposal = |payload| {
        Proposal::new("interaction", "upsert_entity", payload).evidence(vec![eid.clone()])
    };
    let id = one(
        &svc,
        proposal(json!({"name":"Nimbus", "kind":"organization", "attributes":{"sector":"cloud"}})),
    )
    .await
    .object_ids
    .remove(0);
    for target in ["Nimbus", id.as_str()] {
        assert!(
            one(
                &svc,
                proposal(json!({"attributes":{"office_location":"Berlin"}})).target(target)
            )
            .await
            .accepted
        );
    }
    assert!(!one(&svc, proposal(json!({"attributes":{}}))).await.accepted);
    assert!(
        !one(&svc, proposal(json!({"attributes":{}})).target("unknown"))
            .await
            .accepted
    );
    assert!(
        !one(&svc, proposal(json!({"name":"Other"})).target(&id))
            .await
            .accepted
    );
    let st = svc.store.lock().unwrap();
    let entity = st.state.get("entities", &id).unwrap();
    assert_eq!(entity["kind"], "organization");
    assert_eq!(
        entity["attributes"],
        json!({"sector":"cloud","office_location":"Berlin"})
    );
    assert_eq!(st.state.list_rows("entities", None, 100).len(), 1);
}
