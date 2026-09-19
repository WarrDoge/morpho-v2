//! Explicitly live probes; never run by cargo test.
//! audit continuity OUTPUT | audit causal OUTPUT DANA_AUDIT
#[path = "../src/bin/support/audit.rs"]
mod audit;

use anyhow::{Context, Result, ensure};
use morpho::{
    Services,
    config::settings,
    context::composer::compose_for,
    ids::IdGen,
    interact::{RESPONSE_SYSTEM, interact_as, request_context},
    llm::{DeepInfra, Llm},
    store::Store,
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};

async fn continuity(a: &audit::Audit, svc: &mut Services) -> Result<()> {
    let turns = [
        ("alice", "a1", "I live in Berlin. My dog Rex is a beagle."),
        ("bob", "b1", "I live in Madrid. My dog Rex is a poodle."),
        (
            "alice",
            "a1",
            "Correction: my Rex is a basset hound, not a beagle.",
        ),
        (
            "alice",
            "a1",
            "Help me track booking a vet appointment for my Rex.",
        ),
        ("bob", "b1", "What breed is my dog, and where do I live?"),
        ("alice", "a2", "What breed is my dog, and where do I live?"),
        (
            "alice",
            "a2",
            "I booked Rex's vet appointment. That task is done.",
        ),
        (
            "alice",
            "a2",
            "What do I still need to do from my to-do list?",
        ),
    ];
    for (i, (speaker, session, text)) in turns.iter().enumerate() {
        if i == 5 {
            let before = svc.store.lock().unwrap().state.clone();
            // Drop the database owner before reopening. Keep process-wide provider counters.
            let spare = Services::new(
                Store::open(&a.dir.join("spare"), IdGen::random())?,
                Llm::Fake(Default::default()),
            );
            let old = std::mem::replace(svc, spare);
            let llm = old.llm.clone();
            drop(old);
            let store = Store::open(&a.dir.join("database"), IdGen::random())?;
            ensure!(store.state == before, "state changed on reopen");
            *svc = Services {
                store: store.shared(),
                llm,
                cycle_lock: Default::default(),
            };
            a.record(svc, "reopen", json!({"identical_state": true}))?;
        }
        ensure!(svc.llm.counts().llm_calls <= 14, "foreground call cap");
        a.record(
            svc,
            "turn_start",
            json!({"turn":i+1,"speaker":speaker,"text":text}),
        )?;
        let reply = interact_as(
            svc,
            text,
            Some(session),
            speaker,
            Some(&format!("audit-{i}")),
        )
        .await?;
        a.record(svc, "turn", json!({"turn":i+1,"reply":reply}))?;
        if i == 3 {
            ensure!(
                svc.store
                    .lock()
                    .unwrap()
                    .state
                    .table("goals")
                    .rows
                    .iter()
                    .any(|g| g["origin"] == "user" && g["status"] == "active"),
                "explicit goal was not saved"
            );
        }
        if i == 6 {
            ensure!(
                svc.store
                    .lock()
                    .unwrap()
                    .state
                    .table("goals")
                    .rows
                    .iter()
                    .any(|g| g["origin"] == "user" && g["status"] == "completed"),
                "explicit goal was not completed"
            );
        }
        let calls = svc.llm.counts().llm_calls;
        let repeated = interact_as(
            svc,
            text,
            Some(session),
            speaker,
            Some(&format!("audit-{i}")),
        )
        .await?;
        ensure!(
            repeated == reply && svc.llm.counts().llm_calls == calls,
            "duplicate was not idempotent"
        );
    }
    let worker_svc = Arc::new(Services {
        store: svc.store.clone(),
        llm: svc.llm.clone(),
        cycle_lock: Default::default(),
    });
    let initial = svc.store.lock().unwrap().state.snapshots.len();
    let worker = tokio::spawn(async move { morpho::worker::run_forever(&worker_svc).await });
    let result = tokio::time::timeout(Duration::from_secs(600), async {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            ensure!(
                svc.llm.counts().llm_calls < 32,
                "continuity call cap reached"
            );
            let st = svc.store.lock().unwrap();
            ensure!(
                !st.state
                    .events
                    .iter()
                    .any(|e| e["type"] == "runtime_failure"
                        && e["payload"]["operation"] == "maintenance"),
                "idle maintenance failed; no retry"
            );
            if st.state.snapshots.len() > initial
                && st.state.cursors["reflection"]["pending"] != true
            {
                break;
            }
        }
        a.record(svc, "idle_completed", json!({"without_new_input":true}))?;
        let calls = svc.llm.counts().llm_calls;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            ensure!(
                svc.llm.counts().llm_calls == calls,
                "unchanged idle state repeated inference"
            );
        }
        a.record(
            svc,
            "idle_quiet",
            json!({"observation_seconds":4,"unchanged_calls":true}),
        )?;
        Ok::<_, anyhow::Error>(())
    })
    .await;
    worker.abort();
    let _ = worker.await;
    result.context("idle worker timed out")?
}

fn self_section(context: &str) -> &str {
    context
        .split("\n\n")
        .find(|part| part.starts_with("## SELF MODEL\n"))
        .unwrap_or("")
}

async fn causal(a: &audit::Audit, svc: &Services, source: &Path) -> Result<()> {
    morpho::pyfmt::set_eval_clock(morpho::pyfmt::parse_dt("2026-09-11T12:00:00Z").unwrap())?;
    let rows: Vec<Value> = std::fs::read_to_string(source.join("steps.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    let (index, after) = rows
        .iter()
        .enumerate()
        .find(|(_, row)| {
            (row["phase"] == "cycle" || row["phase"] == "turn")
                && row["state"]["self_state"]["version"].as_u64().unwrap_or(1) > 1
        })
        .context("no observed operational self revision")?;
    let before = rows[..index]
        .iter()
        .rev()
        .find(|r| r["phase"] == "turn")
        .context("no pre-cycle state")?;
    let transition = after["transitions"]
        .as_array()
        .context("transitions")?
        .last()
        .context("no transition")?["id"]
        .as_i64();
    let (state, _) = morpho::snapshots::replay(&source.join("database"), transition)?;
    ensure!(
        serde_json::to_value(&state.self_state)? == after["state"]["self_state"],
        "checkpoint self-state mismatch"
    );
    // Read historical vectors from the retained source; all interventions remain private.
    let source_store = Store::open(&source.join("database"), IdGen::random())?;
    let mut fork = source_store.fork();
    fork.state = state;
    let checkpoint = Services {
        store: fork.shared(),
        llm: svc.llm.clone(),
        cycle_lock: Default::default(),
    };
    a.record(&checkpoint, "checkpoint", json!({"source":source,"turn":after["details"]["turn"],"previous_self":before["state"]["self_state"]}))?;
    for probe in [
        "What should I focus on next?",
        "Have I finished the Anmeldung?",
    ] {
        let staged = checkpoint.staged();
        let event = staged.store.lock().unwrap().append_event(
            "user_message",
            "user",
            json!({"text":probe}),
            None,
            None,
        )?;
        let embedding = svc.llm.embed(&[probe.to_owned()]).await?.remove(0);
        let (context, manifest) = compose_for(
            &staged,
            &embedding,
            probe,
            "user",
            event["event_id"].as_str(),
            None,
        )?;
        let revised = self_section(&context);
        ensure!(
            !revised.is_empty(),
            "relevant revision was excluded from actual context"
        );
        staged.store.lock().unwrap().state.self_state =
            serde_json::from_value(before["state"]["self_state"].clone())?;
        let (previous_context, _) = compose_for(
            &staged,
            &embedding,
            probe,
            "user",
            event["event_id"].as_str(),
            None,
        )?;
        let previous = self_section(&previous_context);
        ensure!(previous != revised, "no self-model intervention");
        let ablated = context.replacen(revised, previous, 1);
        let user = request_context("user", probe, event["event_id"].as_str().unwrap(), "");
        a.record(
            &checkpoint,
            "paired_prompts",
            json!({"probe":probe,"before":ablated,"after":context,"user":user,"manifest":manifest}),
        )?;
        for trial in 0..3 {
            let conditions = if trial % 2 == 0 {
                ["before", "after"]
            } else {
                ["after", "before"]
            };
            for condition in conditions {
                ensure!(svc.llm.counts().llm_calls < 12, "causal call cap");
                let prompt = if condition == "before" {
                    &ablated
                } else {
                    &context
                };
                let response = svc
                    .llm
                    .complete_text(
                        RESPONSE_SYSTEM,
                        &request_context(
                            "user",
                            probe,
                            event["event_id"].as_str().unwrap(),
                            prompt,
                        ),
                    )
                    .await?;
                a.record(&checkpoint, "causal_reply", json!({"probe":probe,"trial":trial+1,"condition":condition,"response":response}))?;
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args
        .next()
        .context("audit continuity OUTPUT | audit causal OUTPUT DANA_AUDIT")?;
    ensure!(
        !settings().deepinfra_api_key.is_empty(),
        "live provider key required"
    );
    if mode == "continuity" {
        ensure!(
            settings().idle_reflect_seconds <= 1.0,
            "set IDLE_REFLECT_SECONDS=1 for the idle probe"
        );
    }
    let a = audit::Audit::new(args.next().context("output directory required")?.into())?;
    let mut svc = Services::new(
        Store::open(&a.dir.join("database"), IdGen::random())?,
        Llm::Live(DeepInfra::new()),
    );
    a.record(
        &svc,
        "start",
        json!({"mode":mode,"completion_call_cap":if mode=="continuity" {32}else{12}}),
    )?;
    let result = match mode.as_str() {
        "continuity" => continuity(&a, &mut svc).await,
        "causal" => {
            causal(
                &a,
                &svc,
                Path::new(&args.next().context("Dana audit directory required")?),
            )
            .await
        }
        _ => anyhow::bail!("unknown mode"),
    };
    a.record(
        &svc,
        "end",
        json!({"error":result.as_ref().err().map(|e| format!("{e:#}"))}),
    )?;
    result
}
