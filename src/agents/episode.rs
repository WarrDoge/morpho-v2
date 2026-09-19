//! What an episode of work leaves behind, written by code: a digest reflection reads instead of
//! every step, open loops that give the agent an agenda, and credit for what was in mind when
//! the work was verified or not.
use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    Services,
    agents::act::{Assignment, SESSION, clip, is_check},
    pyfmt::{Row, round4},
    state::{
        engine::{self, CommitResult},
        models::{LIVE_GOAL, LIVE_MEMORY, Proposal},
    },
    store::state::State,
};

/// An open loop: a goal row, and the log index of the step that opened it.
#[derive(Clone, Debug)]
pub struct Loop {
    pub id: String,
    pub at: usize,
}

fn s<'a>(r: &'a Row, k: &str) -> &'a str {
    r.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn f(r: &Row, k: &str) -> f64 {
    r.get(k).and_then(Value::as_f64).unwrap_or_default()
}

pub fn live_loops(state: &State, kind: &str) -> Vec<Loop> {
    state
        .table("goals")
        .rows
        .iter()
        .filter(|g| s(g, "kind") == kind && LIVE_GOAL.contains(&s(g, "status")))
        .map(|g| Loop {
            id: s(g, "id").to_string(),
            at: 0,
        })
        .collect()
}

/// The most pressing goal of its own, as the query for idle work.
pub fn top_goal(state: &State) -> Option<String> {
    state
        .list_rows("goals", Some(LIVE_GOAL), usize::MAX)
        .into_iter()
        .filter(|g| s(g, "origin") == "self")
        .max_by(|a, b| {
            f(a, "priority")
                .total_cmp(&f(b, "priority"))
                .then_with(|| s(b, "id").cmp(s(a, "id")))
        })
        .map(|g| match s(&g, "next_step") {
            "" => s(&g, "description").to_string(),
            step => format!("{} Next: {step}", s(&g, "description")),
        })
}

async fn commit(svc: &Services, proposals: &[Proposal]) -> Result<Vec<CommitResult>> {
    if proposals.is_empty() {
        return Ok(Vec::new());
    }
    engine::commit(&svc.store, &svc.llm, proposals, None).await
}

fn is_check_row(r: &Value) -> bool {
    r["kind"] == "act"
        && r["tool"] == "run"
        && r["exit"] == 0
        && r["expect_success"] == true
        && is_check(r["target"].as_str().unwrap_or(""))
}

fn tail(r: &Value, lines: usize, chars: usize) -> String {
    let out: Vec<&str> = r["output"].as_str().unwrap_or("").lines().collect();
    clip(
        &json!(out[out.len().saturating_sub(lines)..].join("\n")),
        chars,
    )
}

pub async fn open_surprise(svc: &Services, act: &Value, observation: &str) -> Result<Option<Loop>> {
    let expected = if act["expect_success"] == true {
        "it to pass"
    } else {
        "it to fail"
    };
    let p = Proposal::new(
        "loops",
        "create_goal",
        json!({"kind": "surprise", "priority": 0.9, "description": format!(
            "Unexpected: `{}` exited {} though I expected {expected}.",
            clip(&act["target"], 80), act["exit"])}),
    )
    .evidence(vec![observation.to_string()])
    .confidence(1.0)
    .reason("a confident expectation missed");
    let r = commit(svc, &[p]).await?.remove(0);
    Ok(r.accepted.then(|| Loop {
        id: r.object_ids[0].clone(),
        at: 0,
    }))
}

fn update(id: &str, fields: Value, evidence: &str, reason: &str) -> Proposal {
    Proposal::new("loops", "update_goal", fields)
        .target(id)
        .evidence(vec![evidence.to_string()])
        .confidence(1.0)
        .reason(reason)
}

/// Completes the live loops among `ids` on a passing check.
pub async fn complete(svc: &Services, ids: &[String], observation: &str) -> Result<Vec<Value>> {
    let live: Vec<(String, String)> = {
        let st = svc.store.lock().unwrap();
        ids.iter()
            .filter_map(|id| st.state.get("goals", id))
            .filter(|g| LIVE_GOAL.contains(&s(g, "status")))
            .map(|g| (s(g, "id").to_string(), s(g, "kind").to_string()))
            .collect()
    };
    let proposals: Vec<Proposal> = live
        .iter()
        .map(|(id, _)| {
            update(
                id,
                json!({"status": "completed"}),
                observation,
                "a passing check",
            )
        })
        .collect();
    let results = commit(svc, &proposals).await?;
    Ok(live
        .iter()
        .zip(results)
        .filter(|(_, r)| r.accepted)
        .map(|((id, kind), _)| json!({"kind": "loop", "op": "close", "loop": kind, "id": id}))
        .collect())
}

/// Ends an episode: the digest event, loop upkeep and credit. Returns the log rows.
pub async fn close(
    svc: &Services,
    task: Option<&Assignment<'_>>,
    log: &[Value],
    recalled: &BTreeSet<String>,
    surprises: &[Loop],
    before: &[Loop],
) -> Result<Vec<Value>> {
    let acts: Vec<&Value> = log.iter().filter(|r| r["kind"] == "act").collect();
    let last_write = acts.iter().rposition(|r| r["tool"] == "write");
    let verified = last_write.map(|w| acts[w + 1..].iter().any(|r| is_check_row(r)));
    let ended = acts
        .last()
        .and_then(|r| r["tool"].as_str())
        .filter(|t| matches!(*t, "done" | "rest"))
        .unwrap_or("limit");
    let runs: Vec<&&Value> = acts.iter().filter(|r| r["tool"] == "run").collect();
    let failed: Vec<&&&Value> = runs
        .iter()
        .filter(|r| {
            r["exit"] != 0
                && (r["surprise"] == true || is_check(r["target"].as_str().unwrap_or("")))
        })
        .collect();
    let failures: Vec<Value> = failed[failed.len().saturating_sub(4)..]
        .iter()
        .map(|r| {
            json!({"command": clip(&r["target"], 160), "exit": r["exit"],
            "tail": tail(r, 3, 300), "observation": r["observation_id"]})
        })
        .collect();
    let files: BTreeSet<&str> = acts
        .iter()
        .filter(|r| r["tool"] == "write")
        .filter_map(|r| r["target"].as_str())
        .collect();
    let files = files.into_iter().collect::<Vec<_>>().join(", ");
    let last_run = runs.last();
    let verdict = match verified {
        Some(true) => "verified by a passing check".to_string(),
        Some(false) => format!(
            "my changes to {files} have no passing check since{}",
            last_run.map_or(String::new(), |r| format!(
                " (last run: `{}` exit {})",
                clip(&r["target"], 80),
                r["exit"]
            ))
        ),
        None => "no files changed".to_string(),
    };
    let report = acts
        .last()
        .filter(|r| r["tool"] == "done")
        .map(|r| clip(&r["target"], 300));
    let text = format!(
        "{}. Ended {ended}; {verdict}.{}",
        task.map_or("Nothing assigned".to_string(), |t| clip(
            &json!(t.text),
            600
        )),
        report
            .as_ref()
            .map_or(String::new(), |r| format!(" Report: {r}"))
    );
    let payload = json!({
        "text": text, "assignment": task.map(|t| t.event_id), "ended": ended, "verified": verified,
        "report": report, "files": files,
        "steps": acts.iter().map(|r| format!("{} {}{}", r["tool"].as_str().unwrap_or(""),
            clip(&r["target"], 80), r["exit"].as_i64().map_or(String::new(), |e| format!(" -> exit {e}")))).collect::<Vec<_>>(),
        "failures": failures,
        "plans": log.iter().filter(|r| r["kind"] == "think").map(|r| clip(&r["plan"], 200)).collect::<Vec<_>>(),
    });
    let emb = svc.llm.embed(std::slice::from_ref(&text)).await?.remove(0);
    let episode = {
        let mut st = svc.store.lock().unwrap();
        let slot = st.vectors.append(&emb)?;
        st.append_event("episode", "self", payload, Some(SESSION), Some(slot))?["event_id"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let mut rows =
        vec![json!({"kind": "episode", "id": episode, "ended": ended, "verified": verified})];

    let mut loops: Vec<(Proposal, Value)> = Vec::new();
    let (live_surprises, live_unverified) = {
        let st = svc.store.lock().unwrap();
        let live = |id: &str| {
            st.state
                .get("goals", id)
                .is_some_and(|g| LIVE_GOAL.contains(&s(g, "status")))
        };
        let sup: Vec<String> = surprises
            .iter()
            .filter(|l| live(&l.id))
            .map(|l| l.id.clone())
            .collect();
        let unv: Vec<(String, f64)> = live_loops(&st.state, "unverified")
            .into_iter()
            .map(|l| {
                let p = st
                    .state
                    .get("goals", &l.id)
                    .map_or(0.0, |g| f(g, "priority"));
                (l.id, p)
            })
            .collect();
        (sup, unv)
    };
    for id in &live_surprises {
        loops.push((
            update(
                id,
                json!({"status": "superseded"}),
                &episode,
                "the episode ended",
            ),
            json!({"kind": "loop", "op": "supersede", "loop": "surprise", "id": id}),
        ));
    }
    if verified == Some(false) {
        if live_unverified.is_empty() {
            let p = Proposal::new(
                "loops",
                "create_goal",
                json!({"kind": "unverified", "priority": 0.8,
                    "description": format!("Unverified: {verdict}.")}),
            )
            .evidence(vec![episode.clone()])
            .confidence(1.0)
            .reason("work ended without a passing check");
            loops.push((
                p,
                json!({"kind": "loop", "op": "open", "loop": "unverified"}),
            ));
        }
        for (id, _) in &live_unverified {
            loops.push((
                update(id, json!({"priority": 0.8}), &episode, "still unverified"),
                json!({"kind": "loop", "op": "refresh", "loop": "unverified", "id": id}),
            ));
        }
    } else {
        for (id, priority) in live_unverified
            .iter()
            .filter(|(id, _)| before.iter().any(|b| &b.id == id))
        {
            let next = round4(priority - 0.2);
            let (fields, op) = if next < 0.3 {
                (json!({"status": "abandoned"}), "abandon")
            } else {
                (json!({"priority": next}), "decay")
            };
            loops.push((
                update(id, fields, &episode, "left open another episode"),
                json!({"kind": "loop", "op": op, "loop": "unverified", "id": id}),
            ));
        }
    }
    let (proposals, logs): (Vec<Proposal>, Vec<Value>) = loops.into_iter().unzip();
    for (mut row, r) in logs.into_iter().zip(commit(svc, &proposals).await?) {
        if r.accepted {
            if row["id"].is_null() {
                row["id"] = json!(r.object_ids.first());
            }
            rows.push(row);
        }
    }

    // Without a write, the last run of the code says how the episode went.
    let outcome = verified.or_else(|| {
        runs.iter()
            .rfind(|r| is_check(r["target"].as_str().unwrap_or("")))
            .map(|r| is_check_row(r))
    });
    if let Some(ok) = outcome {
        let credit: Vec<Proposal> = {
            let st = svc.store.lock().unwrap();
            let distances = st.vectors.distances(&emb)?;
            // Everything recalled for a coding task sits near 0.5 to the digest; only a closer
            // match says the memory was about this work.
            let weight = |id: &str| {
                st.state
                    .slots
                    .get(id)
                    .and_then(|slot| distances.get(slot))
                    .map_or(0.0, |d| round4(((1.0 - d - 0.6) / 0.4).clamp(0.0, 1.0)))
            };
            let memories = recalled
                .iter()
                .filter(|id| {
                    st.state
                        .get("memories", id)
                        .is_some_and(|m| LIVE_MEMORY.contains(&s(m, "status")))
                })
                .filter(|id| weight(id) > 0.0)
                .map(|id| {
                    let key = if ok { "add_success" } else { "add_failure" };
                    Proposal::new("credit", "update_memory", json!({key: weight(id)})).target(id)
                });
            memories
                .map(|p| {
                    p.evidence(vec![episode.clone()])
                        .confidence(1.0)
                        .reason(if ok {
                            "verified work"
                        } else {
                            "unverified work"
                        })
                })
                .collect()
        };
        let results = commit(svc, &credit).await?;
        let count = |op: &str| {
            credit
                .iter()
                .zip(&results)
                .filter(|(p, r)| p.operation == op && r.accepted)
                .count()
        };
        rows.push(json!({"kind": "credit", "verified": ok,
            "memories": count("update_memory")}));
    }
    Ok(rows)
}
