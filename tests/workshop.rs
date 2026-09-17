mod common;
use common::{fake, services, temp_dir};
use morpho::Services;
use morpho::agents::act::{Arm, Assignment, Limits, agenda_open, work};
use morpho::agents::reflection;
use morpho::agents::seed::seed_traits;
use morpho::context::composer::identity;
use morpho::context::ranking::score;
use morpho::pyfmt::now;
use morpho::state::engine::commit;
use morpho::state::models::Proposal;
use morpho::worker::cycle;
use morpho::workshop;
use serde_json::{Value, json};

fn workspace(name: &str) -> std::path::PathBuf {
    let ws = temp_dir(name);
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    ws
}

fn act(
    tool: &str,
    path: &str,
    content: &str,
    command: &str,
    expect: bool,
    confidence: f64,
) -> Value {
    json!({"thought":"t","tool":tool,"path":path,"content":content,"command":command,
        "text": if tool == "done" { "report" } else { "" },"expect_success":expect,"confidence":confidence,
        "practice":""})
}

const LIMITS: Limits = Limits {
    steps: 10,
    thinks: 3,
};

fn kinds(log: &[Value]) -> Vec<&str> {
    log.iter().map(|r| r["kind"].as_str().unwrap()).collect()
}

fn goal(svc: &Services, kind: &str) -> Value {
    let st = svc.store.lock().unwrap();
    let g = st
        .state
        .table("goals")
        .rows
        .iter()
        .find(|g| g["kind"] == kind)
        .unwrap();
    json!(g)
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
    let long = "x = 1\n".repeat(1000);
    workshop::write(&ws, "long.py", &long).unwrap();
    assert_eq!(workshop::read(&ws, "long.py").unwrap().text, long);
    assert!(
        workshop::run(&ws, "cat long.py")
            .unwrap()
            .text
            .contains("chars cut")
    );
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

/// A confident miss, a fix and a passing check, then done.
async fn fixed_failure(arm: Arm) -> (Services, Vec<Value>) {
    let svc = services(&format!("ws-fix-{}", arm.name()));
    let ws = workspace(&format!("ws-fix-dir-{}", arm.name()));
    let task_event = common::event(&svc, "make x.py succeed");
    commit(
        &svc.store,
        &svc.llm,
        &[Proposal::new(
            "reflection",
            "create_memory",
            json!({"summary": "make x.py succeed. Ended done; verified by a passing check. Report: report", "kind": "semantic"}),
        )
        .evidence(vec![task_event.clone()])],
        None,
    )
    .await
    .unwrap();
    let f = fake(&svc);
    f.queue(
        "Act",
        act("write", "x.py", "import sys; sys.exit(1)\n", "", false, 0.0),
    );
    f.queue("Act", act("run", "", "", "python3 x.py", true, 0.9));
    f.queue(
        "Think",
        json!({"thought":"it exits 1","plan":"fix it","p_success":0.8}),
    );
    f.queue("Act", act("write", "x.py", "print(1)\n", "", false, 0.0));
    f.queue("Act", act("run", "", "", "python3 x.py", true, 0.9));
    f.queue(
        "Lesson",
        json!({"statement": "When python3 x.py exits 1, I read x.py before I run it again.", "general": true}),
    );
    f.queue("Act", act("done", "", "", "", false, 0.0));
    let task = Assignment {
        event_id: &task_event,
        text: "make x.py succeed",
    };
    let log = work(&svc, &ws, arm, Some(&task), &LIMITS).await.unwrap();
    (svc, log)
}

#[tokio::test]
async fn a_fixed_failure_closes_its_loop_and_becomes_a_practice() {
    let (svc, log) = fixed_failure(Arm::Morphling).await;
    assert_eq!(
        kinds(&log),
        [
            "act", "act", "loop", "think", "act", "act", "loop", "lesson", "act", "episode",
            "credit"
        ]
    );
    assert_eq!(log[3]["trigger"], "surprise");
    let recalls: Vec<bool> = log
        .iter()
        .filter(|r| r["kind"] == "act")
        .map(|r| r["recall"].is_array())
        .collect();
    assert_eq!(recalls, [true, false, false, false, false]);
    let surprise = goal(&svc, "surprise");
    assert_eq!(surprise["status"], "completed");
    assert_eq!(surprise["origin"], "self");
    assert!(
        surprise["evidence"]
            .as_array()
            .unwrap()
            .contains(&log[5]["observation_id"])
    );

    let practice = |svc: &Services| {
        let st = svc.store.lock().unwrap();
        json!(
            st.state
                .table("traits")
                .rows
                .iter()
                .find(|t| t["kind"] == "practice")
                .unwrap()
        )
    };
    let first = practice(&svc);
    let evidence = first["evidence"].as_array().unwrap();
    assert!(
        evidence.contains(&log[1]["observation_id"])
            && evidence.contains(&log[5]["observation_id"])
    );
    assert_eq!(
        first["confidence"], 0.4,
        "shown but never applied: no credit"
    );
    assert_eq!(log[10]["verified"], true);
    assert_eq!(log[10]["practices"], 0);

    let f = fake(&svc);
    let mut applied = act("run", "", "", "python3 x.py", true, 0.9);
    applied["practice"] = first["id"].clone();
    f.queue("Act", applied);
    f.queue("Act", act("done", "", "", "", false, 0.0));
    let task_event = common::event(&svc, "check x.py again");
    let task = Assignment {
        event_id: &task_event,
        text: "check x.py again",
    };
    let ws = workspace("ws-fix-dir-morphling");
    std::fs::write(ws.join("x.py"), "print(1)\n").unwrap();
    let again = work(&svc, &ws, Arm::Morphling, Some(&task), &LIMITS)
        .await
        .unwrap();
    assert_eq!(again[0]["practice"], first["id"]);
    let credited = practice(&svc);
    assert_eq!(credited["confidence"], 0.5);
    let episode = again.iter().find(|r| r["kind"] == "episode").unwrap();
    assert!(
        credited["supporting_evidence"]
            .as_array()
            .unwrap()
            .contains(&episode["id"])
    );

    let st = svc.store.lock().unwrap();
    let memory = st.state.table("memories").rows[0].clone();
    assert!(memory["successes"].as_f64().unwrap() > 0.0);
    assert!(identity(&st, None).block.contains("How I work:\n- trait_"));
    assert!(!agenda_open(&st.state));
    let row = |successes: f64| {
        let mut r = memory.clone();
        r.insert("successes".into(), json!(successes));
        r
    };
    let base = score(&row(0.0), 0.5, &now(), 30.0);
    assert!(score(&row(3.0), 0.5, &now(), 30.0) > base);
    let mut plain = memory.clone();
    plain.remove("successes");
    assert_eq!(score(&plain, 0.5, &now(), 30.0), base);
}

#[tokio::test]
async fn the_transcript_agent_keeps_no_state() {
    let (svc, log) = fixed_failure(Arm::Transcript).await;
    assert_eq!(kinds(&log), ["act", "act", "act", "act", "act"]);
    assert!(fake(&svc).calls_for("Think").is_empty());
    assert!(
        svc.store
            .lock()
            .unwrap()
            .state
            .table("goals")
            .rows
            .is_empty()
    );
}

#[tokio::test]
async fn a_stall_thinks_and_unverified_work_becomes_the_idle_agenda() {
    let svc = services("ws-stall");
    let ws = workspace("ws-stall-dir");
    let f = fake(&svc);
    f.queue(
        "Act",
        act(
            "write",
            "x.py",
            "raise SystemExit('same failure')\n",
            "",
            false,
            0.0,
        ),
    );
    f.queue("Act", act("run", "", "", "python3 x.py", false, 0.5));
    f.queue("Act", act("run", "", "", "python3 x.py", false, 0.5));
    f.queue(
        "Think",
        json!({"thought":"same failure","plan":"read it","p_success":0.5}),
    );
    f.queue("Act", act("done", "", "", "", false, 0.0));
    let text = "make x.py succeed";
    let event_id = common::event(&svc, text);
    let task = Assignment {
        event_id: &event_id,
        text,
    };
    let log = work(&svc, &ws, Arm::Morphling, Some(&task), &LIMITS)
        .await
        .unwrap();
    let think = log.iter().find(|r| r["kind"] == "think").unwrap();
    assert_eq!(think["trigger"], "stall");
    let input: Value =
        serde_json::from_str(f.calls_for("Think")[0].lines().last().unwrap()).unwrap();
    assert_eq!(input["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(input["attempts"][0]["output_tail"], "same failure");
    let unverified = goal(&svc, "unverified");
    assert_eq!(unverified["status"], "active");
    assert!(agenda_open(&svc.store.lock().unwrap().state));

    f.queue("Act", act("write", "x.py", "print(1)\n", "", false, 0.0));
    f.queue("Act", act("run", "", "", "python3 x.py", true, 0.9));
    let idle = work(&svc, &ws, Arm::Morphling, None, &LIMITS)
        .await
        .unwrap();
    assert_eq!(
        kinds(&idle),
        ["act", "act", "loop", "episode", "credit"],
        "the idle gate closes with the loop"
    );
    let act_input: Value =
        serde_json::from_str(f.calls_for("Act")[4].lines().last().unwrap()).unwrap();
    assert!(
        act_input["recalled_state"]
            .as_str()
            .unwrap()
            .contains("Unverified: my changes to x.py")
    );
    assert_eq!(goal(&svc, "unverified")["status"], "completed");
}

#[tokio::test]
async fn reflection_reads_episodes_not_raw_steps() {
    assert_eq!(reflection::window(&[1, 2, 2, 1, 2], 0, 2, |x| *x == 2), 4);
    let svc = services("ws-reflect");
    common::event(&svc, "task");
    for kind in ["action", "observation"].repeat(8) {
        svc.store
            .lock()
            .unwrap()
            .append_event(kind, "self", json!({"text": kind}), None, None)
            .unwrap();
    }
    cycle(&svc, true).await.unwrap();
    let calls = fake(&svc).calls_for("Reflection");
    assert_eq!(calls.len(), 1);
    let input: Value = serde_json::from_str(calls[0].lines().last().unwrap()).unwrap();
    assert_eq!(input["observations"].as_array().unwrap().len(), 1);
    assert_eq!(svc.store.lock().unwrap().state.cursor("reflection"), 17);
    svc.store
        .lock()
        .unwrap()
        .append_event("thought", "self", json!({"text": "hm"}), None, None)
        .unwrap();
    cycle(&svc, true).await.unwrap();
    assert_eq!(fake(&svc).calls_for("Reflection").len(), 1);
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
