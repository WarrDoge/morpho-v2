mod common;
use common::{fake, services, temp_dir};
use morpho::agents::act::{Arm, Assignment, Limits, work};
use morpho::agents::seed::seed_traits;
use morpho::state::engine::commit;
use morpho::state::models::Proposal;
use morpho::workshop;
use serde_json::json;

fn workspace(name: &str) -> std::path::PathBuf {
    let ws = temp_dir(name);
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    ws
}

#[test]
fn the_sandbox_holds_only_the_workspace() {
    unsafe { std::env::set_var("DEEPINFRA_API_KEY", "must-not-leak") };
    let ws = workspace("ws-sandbox");
    let w = workshop::write(&ws, "pkg/a.py", "print(1)\n").unwrap();
    assert_eq!(w.exit, 0, "{}", w.text);
    assert_eq!(
        std::fs::read_to_string(ws.join("pkg/a.py")).unwrap(),
        "print(1)\n"
    );
    assert_eq!(workshop::run(&ws, "python3 pkg/a.py").unwrap().text, "1\n");
    assert_eq!(workshop::list(&ws).unwrap().text, "./pkg/a.py\n");
    let home = std::env::var("HOME").unwrap();
    assert_ne!(
        workshop::read(&ws, &format!("{home}/.bashrc"))
            .unwrap()
            .exit,
        0
    );
    let env = workshop::run(&ws, "env").unwrap().text;
    assert!(!env.contains("DEEPINFRA"), "{env}");
    let net = workshop::run(
        &ws,
        "python3 -c 'import socket; socket.create_connection((\"1.1.1.1\", 53), 1)'",
    )
    .unwrap();
    assert_ne!(net.exit, 0);
    let t = workshop::run(&ws, "python3 -c 'print(object())'").unwrap();
    assert!(t.text.contains("0x?"), "{}", t.text);
}

async fn failing_run(arm: Arm) -> (Vec<serde_json::Value>, usize) {
    let svc = services(&format!("ws-{}", arm.name()));
    let ws = workspace(&format!("ws-dir-{}", arm.name()));
    let f = fake(&svc);
    f.queue(
        "Act",
        json!({"thought":"t","tool":"write","path":"x.py","content":"import sys; sys.exit(1)\n",
            "command":"","text":"","expect_success":false,"confidence":0.0}),
    );
    f.queue(
        "Act",
        json!({"thought":"t","tool":"run","path":"","content":"","command":"python3 x.py",
            "text":"","expect_success":true,"confidence":0.9}),
    );
    f.queue(
        "Think",
        json!({"thought":"it exits 1","plan":"fix it","p_success":0.8}),
    );
    f.queue(
        "Act",
        json!({"thought":"t","tool":"done","path":"","content":"","command":"","text":"report",
            "expect_success":false,"confidence":0.0}),
    );
    let task = Assignment {
        event_id: &common::event(&svc, "make x.py succeed"),
        text: "make x.py succeed",
    };
    let limits = Limits {
        steps: 10,
        thinks: 3,
        cycle_every: 0,
    };
    let log = work(&svc, &ws, arm, Some(&task), &limits).await.unwrap();
    let st = svc.store.lock().unwrap();
    let observation = st
        .state
        .events
        .iter()
        .rfind(|e| e["type"] == "observation")
        .unwrap();
    let keys: Vec<_> = observation["payload"].as_object().unwrap().keys().collect();
    assert_eq!(keys[..2], ["exit", "ok"]);
    assert_eq!(observation["payload"]["exit"], 1);
    (log, f.calls_for("Think").len())
}

#[tokio::test]
async fn a_confident_miss_makes_only_the_surprise_arm_think() {
    let (log, thinks) = failing_run(Arm::Surprise).await;
    assert_eq!(thinks, 1);
    let kinds: Vec<_> = log.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["act", "act", "think", "act"]);
    assert_eq!(log[1]["surprise"], true);
    assert_eq!(log[2]["trigger"], "surprise");
    let (log, thinks) = failing_run(Arm::NoThink).await;
    assert_eq!(thinks, 0);
    assert_eq!(log.len(), 3);
}

#[tokio::test]
async fn an_observation_can_lower_a_trait() {
    let svc = services("ws-trait");
    seed_traits(
        &svc,
        &[json!({"kind":"style","statement":"My first draft usually passes.","confidence":0.9})],
    )
    .await
    .unwrap();
    let id = svc.store.lock().unwrap().state.table("traits").rows[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let obs = svc
        .store
        .lock()
        .unwrap()
        .append_event(
            "observation",
            "workspace",
            json!({"exit": 1, "ok": false, "text": "FAILED"}),
            None,
            None,
        )
        .unwrap()["event_id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = commit(
        &svc.store,
        &svc.llm,
        &[
            Proposal::new("reflection", "update_trait", json!({"confidence": 0.7}))
                .target(&id)
                .evidence(vec![obs]),
        ],
        None,
    )
    .await
    .unwrap()
    .remove(0);
    assert!(r.accepted, "{:?}", r.reason);
}
