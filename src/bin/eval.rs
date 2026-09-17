//! Scenario runner: metrics for one scenario, optional strict replay and baseline comparison.
//!
//! Usage: eval evals/scenarios/dana.json [--label L] [--strict] [--baseline FILE] [--control] [--trial N] [--rejudge RESULT] [--audit-dir DIR]

#[path = "support/audit.rs"]
mod audit;

use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::Instrument;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use morpho::Services;
use morpho::agents::seed::seed_traits;
use morpho::config::settings;
use morpho::ids::IdGen;
use morpho::interact::interact_as;
use morpho::llm::{DeepInfra, Llm, REPLY, Recording, is_miss};
use morpho::pyfmt::{Row, py_str, round4, tokens};
use morpho::state::models::{LIVE_BELIEF, LIVE_GOAL, LIVE_MEMORY, LIVE_TRAIT};
use morpho::store::Store;
use morpho::worker::{compile_narrative, cycle};

const CONTROL_SYSTEM: &str =
    "You are a helpful assistant. Below is the transcript of your conversation so far
with the user (older turns may have been cut off). Answer the user's latest message concisely.";
const JUDGE_SYSTEM: &str = "Grade the assistant reply against the rubric. \
Return only {\"response\":\"PASS\"} or {\"response\":\"FAIL\"}.";
const AGREE_SYSTEM: &str = "Two replies from the same assistant to related questions. \
Do they express the same position, preference or claim? \
Return only {\"response\":\"YES\"} or {\"response\":\"NO\"}.";
const LOWER_IS_WORSE: [&str; 6] = [
    "probe_accuracy",
    "update_accuracy",
    "judge_accuracy",
    "consistency",
    "retrieval_hit",
    "revision_rate",
];
const HIGHER_IS_WORSE: [&str; 5] = [
    "noise",
    "dup_max_cos",
    "llm_calls",
    "embed_calls",
    "prompt_tokens",
];
const INFO: [&str; 7] = [
    "compactness",
    "ctx_tokens",
    "seconds",
    "cache_misses",
    "probes",
    "revisions",
    "turns",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("evals")
}

fn lower_list(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_lowercase)
        .collect()
}

/// Every expect group present; a rejected (stale) value may only appear after the answer.
pub fn judge(reply: &str, turn: &Value) -> bool {
    let r = reply.to_lowercase();
    let groups: Vec<Vec<String>> = turn["expect"]
        .as_array()
        .into_iter()
        .flatten()
        .map(lower_list)
        .collect();
    if !groups
        .iter()
        .all(|g| g.iter().any(|a| r.contains(a.as_str())))
    {
        return false;
    }
    let rejects: Vec<usize> = lower_list(&turn["reject"])
        .iter()
        .filter_map(|x| r.find(x.as_str()))
        .collect();
    let Some(first_reject) = rejects.into_iter().min() else {
        return true;
    };
    let first_answer = groups
        .iter()
        .flatten()
        .filter_map(|a| r.find(a.as_str()))
        .min()
        .unwrap();
    first_answer < first_reject
}

fn mean(xs: &[f64]) -> Value {
    if xs.is_empty() {
        Value::Null
    } else {
        json!(round4(xs.iter().sum::<f64>() / xs.len() as f64))
    }
}

fn text(t: &Value) -> &str {
    t["text"].as_str().unwrap_or_default()
}

fn speaker(t: &Value) -> &str {
    t["speaker"].as_str().unwrap_or("user")
}

/// PASS/FAIL or YES/NO by majority of `JUDGE_VOTES` calls; 1.0 when it equals `yes`. The
/// judge is not deterministic, so borderline replies are settled by vote. Vote 1 keeps the
/// unsalted prompt, so earlier single-vote caches still serve it.
async fn verdict(llm: &Llm, system: &str, user: &str, yes: &str) -> Result<f64> {
    let votes = settings().judge_votes.max(1);
    let mut agree = 0;
    for vote in 0..votes {
        let salted = if vote == 0 {
            user.to_string()
        } else {
            format!("{user}\n\n(vote {})", vote + 1)
        };
        let v: Value = llm.complete_json(system, &salted, &REPLY).await?;
        let word = v["response"].as_str().unwrap_or_default().trim();
        agree += usize::from(word.eq_ignore_ascii_case(yes));
    }
    Ok((agree * 2 > votes) as u8 as f64)
}

fn turn_span(i: usize, t: &Value) -> tracing::Span {
    tracing::info_span!(
        "eval.turn",
        turn = i + 1,
        tag = t["tag"].as_str().unwrap_or(""),
        family = t["family"].as_str().unwrap_or("")
    )
}

async fn run_harness(
    svc: &Services,
    turns: &[Value],
    cycle_every: usize,
    audit: Option<&audit::Audit>,
) -> Result<(Vec<String>, Vec<Value>, Vec<String>)> {
    let (mut replies, mut manifests, mut event_ids) = (Vec::new(), Vec::new(), Vec::new());
    for (i, t) in turns.iter().enumerate() {
        if let Some(a) = audit {
            a.record(svc, "turn_start", json!({"turn": i + 1, "input": t}))?;
        }
        let body = interact_as(
            svc,
            text(t),
            t["session"].as_str(),
            speaker(t),
            Some(&format!("eval-{i}")),
        )
        .instrument(turn_span(i, t))
        .await?;
        replies.push(body["response"].as_str().unwrap_or_default().to_string());
        manifests.push(body["context"].clone());
        event_ids.push(body["event_id"].as_str().unwrap_or_default().to_string());
        if let Some(a) = audit {
            a.record(svc, "turn", json!({"turn": i + 1, "reply": body}))?;
        }
        if (i + 1) % cycle_every == 0 {
            let stats = cycle(svc, true).await?;
            if let Some(a) = audit {
                a.record(svc, "cycle", json!({"turn": i + 1, "stats": stats}))?;
            }
        }
    }
    // Drain bounded pages, stopping at the durable spending/retry limits.
    for _ in 0..64 {
        let stats = cycle(svc, true).await?;
        if let Some(a) = audit {
            a.record(svc, "final_cycle", stats.clone())?;
        }
        let failed = svc
            .store
            .lock()
            .unwrap()
            .state
            .cursors
            .get("reflection")
            .and_then(|r| r["failures"].as_i64())
            .unwrap_or(0);
        if stats["pending"] != true
            || !stats["maintenance"].is_null()
            || failed >= settings().consumer_max_failures
        {
            break;
        }
    }
    Ok((replies, manifests, event_ids))
}

async fn run_control(
    svc: &Services,
    turns: &[Value],
    audit: Option<&audit::Audit>,
) -> Result<Vec<String>> {
    let budget = settings().context_token_budget;
    let mut replies = Vec::new();
    let mut transcript: Vec<String> = Vec::new();
    for (i, t) in turns.iter().enumerate() {
        if let Some(a) = audit {
            a.record(svc, "turn_start", json!({"turn":i+1,"input":t}))?;
        }
        let mut tail: Vec<&str> = Vec::new();
        let mut used = 0;
        for line in transcript.iter().rev() {
            if used + tokens(line) > budget {
                break;
            }
            tail.insert(0, line);
            used += tokens(line);
        }
        let prompt = format!("{CONTROL_SYSTEM}\n\n# TRANSCRIPT\n{}", tail.join("\n"));
        let reply = svc
            .llm
            .complete_text(&prompt, text(t))
            .instrument(turn_span(i, t))
            .await?;
        if let Some(a) = audit {
            a.record(svc, "turn", json!({"turn":i+1,"reply":{"response":reply}}))?;
        }
        transcript.push(format!("{}: {}", speaker(t), text(t)));
        transcript.push(format!("assistant: {reply}"));
        replies.push(reply);
    }
    Ok(replies)
}

async fn behaviour(llm: &Llm, turns: &[Value], replies: &[String]) -> Result<Map<String, Value>> {
    let probes: Vec<(&Value, &String)> = turns
        .iter()
        .zip(replies)
        .filter(|(t, _)| t["tag"] == json!("probe") && t["expect"].is_array())
        .collect();
    let verdicts: Vec<f64> = probes
        .iter()
        .map(|(t, r)| judge(r, t) as u8 as f64)
        .collect();
    let updates: Vec<f64> = probes
        .iter()
        .filter(|(t, _)| t["reject"].as_array().is_some_and(|a| !a.is_empty()))
        .map(|(t, r)| judge(r, t) as u8 as f64)
        .collect();
    let mut m = Map::new();
    m.insert("probe_accuracy".into(), mean(&verdicts));
    m.insert("update_accuracy".into(), mean(&updates));
    m.insert("probes".into(), json!(probes.len()));
    let mut judged = Vec::new();
    let mut judge_failed = Vec::new();
    let mut families: Map<String, Value> = Map::new();
    let mut by_family: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    let mut grouped: Vec<(&str, &Value, &String)> = Vec::new();
    for (i, (t, r)) in turns.iter().zip(replies).enumerate() {
        if let Some(rubric) = t["judge"].as_str() {
            let user = format!("RUBRIC: {rubric}\n\nQUESTION: {}\n\nREPLY: {r}", text(t));
            let v = verdict(llm, JUDGE_SYSTEM, &user, "PASS").await?;
            tracing::info!(target: "eval", turn = i + 1, family = t["family"].as_str().unwrap_or(""),
                ok = v == 1.0, "probe");
            if v == 0.0 {
                judge_failed.push(i + 1);
            }
            judged.push(v);
            if let Some(fam) = t["family"].as_str() {
                by_family.entry(fam.into()).or_default().push(v);
            }
        }
        if let Some(g) = t["group"].as_str() {
            grouped.push((g, t, r));
        }
    }
    let mut agree = Vec::new();
    for (i, (g, ta, a)) in grouped.iter().enumerate() {
        for (h, tb, b) in &grouped[i + 1..] {
            if g != h {
                continue;
            }
            let user = format!(
                "QUESTION A: {}\nREPLY A: {a}\n\nQUESTION B: {}\nREPLY B: {b}",
                text(ta),
                text(tb)
            );
            agree.push(verdict(llm, AGREE_SYSTEM, &user, "YES").await?);
        }
    }
    if !judged.is_empty() {
        m.insert("judge_accuracy".into(), mean(&judged));
        m.insert("judge_failed".into(), json!(judge_failed));
        for (fam, xs) in by_family {
            families.insert(fam, mean(&xs));
        }
        if !families.is_empty() {
            m.insert("judge_by_family".into(), Value::Object(families));
        }
    }
    if !agree.is_empty() {
        m.insert("consistency".into(), mean(&agree));
    }
    Ok(m)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

fn state_metrics(
    svc: &Services,
    turns: &[Value],
    replies: &[String],
    manifests: &[Value],
    ids: &[String],
) -> Result<Map<String, Value>> {
    let st = svc.store.lock().unwrap();
    let s = &st.state;
    let mut hits: Vec<f64> = Vec::new();
    for (t, m) in turns.iter().zip(manifests) {
        let refs: Vec<&str> = t["refs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|i| ids[i as usize].as_str())
            .collect();
        if t["tag"] != json!("probe") || refs.is_empty() {
            continue;
        }
        let hit = m["memories"].as_array().into_iter().flatten().any(|entry| {
            s.get("memories", entry["id"].as_str().unwrap_or_default())
                .is_some_and(|row| {
                    strings(&row["source_events"])
                        .iter()
                        .any(|e| refs.contains(&e.as_str()))
                })
        });
        hits.push(hit as u8 as f64);
    }

    let live_mem = s.list_rows("memories", Some(LIVE_MEMORY), 10_000);
    let distractors: Vec<&str> = turns
        .iter()
        .enumerate()
        .filter(|(_, t)| t["tag"] == json!("distractor"))
        .map(|(i, _)| ids[i].as_str())
        .collect();
    let noisy = live_mem
        .iter()
        .filter(|m| {
            let src = strings(&m["source_events"]);
            !src.is_empty() && src.iter().all(|e| distractors.contains(&e.as_str()))
        })
        .count();

    let slots: Vec<usize> = live_mem
        .iter()
        .filter_map(|m| s.slots.get(m["id"].as_str().unwrap_or_default()).copied())
        .collect();
    let mut dup: Option<f64> = None;
    for (i, a) in slots.iter().enumerate() {
        for b in &slots[i + 1..] {
            let cos = morpho::store::vectors::relevance(st.vectors.similarity_between(*a, *b)?);
            dup = Some(dup.map_or(cos, |d| d.max(cos)));
        }
    }

    let beliefs = s.list_rows("beliefs", None, 10_000);
    let mut revised: Vec<f64> = Vec::new();
    for t in turns.iter().filter(|t| t["tag"] == json!("update")) {
        let contradicts = lower_list(&t["contradicts"]);
        if contradicts.is_empty() {
            continue;
        }
        let asserts = lower_list(&t["asserts"]);
        for b in &beliefs {
            let prop = b["proposition"].as_str().unwrap_or_default().to_lowercase();
            if contradicts.iter().all(|k| prop.contains(k.as_str()))
                && !asserts.iter().any(|k| prop.contains(k.as_str()))
            {
                let ok = b["status"] != json!("active")
                    || b["contradicting_evidence"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty());
                revised.push(ok as u8 as f64);
            }
        }
    }

    let live_beliefs = s.list_rows("beliefs", Some(LIVE_BELIEF), 10_000);
    let goals = s.list_rows("goals", Some(LIVE_GOAL), 10_000);
    let field = |rows: &[Row], k: &str| -> Vec<String> {
        rows.iter()
            .map(|r| r[k].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let state_text = [
        field(&live_mem, "summary"),
        field(&live_beliefs, "proposition"),
        field(&goals, "description"),
    ]
    .concat()
    .join(" ");
    let transcript = turns.iter().map(text).collect::<Vec<_>>().join(" ") + &replies.join(" ");
    let ctx: Vec<f64> = manifests
        .iter()
        .filter_map(|m| m["tokens"].as_f64())
        .collect();

    let mut m = Map::new();
    m.insert("retrieval_hit".into(), mean(&hits));
    m.insert(
        "noise".into(),
        if live_mem.is_empty() {
            Value::Null
        } else {
            json!(round4(noisy as f64 / live_mem.len() as f64))
        },
    );
    m.insert(
        "dup_max_cos".into(),
        dup.map_or(Value::Null, |d| json!(round4(d))),
    );
    m.insert("revision_rate".into(), mean(&revised));
    m.insert("revisions".into(), json!(revised.len()));
    m.insert(
        "compactness".into(),
        json!(round4(
            tokens(&state_text) as f64 / tokens(&transcript) as f64
        )),
    );
    m.insert("ctx_tokens".into(), mean(&ctx));
    let mut by_origin: Map<String, Value> = Map::new();
    for g in s.list_rows("goals", None, 10_000) {
        let origin = g["origin"].as_str().unwrap_or("unknown").to_string();
        let n = by_origin.get(&origin).and_then(Value::as_u64).unwrap_or(0);
        by_origin.insert(origin, json!(n + 1));
    }
    m.insert("goals_by_origin".into(), Value::Object(by_origin));
    let mood_changes = s
        .transitions
        .iter()
        .filter(|t| {
            t["table_name"] == "working_state"
                && t["before"]["data"]["mood"] != t["after"]["data"]["mood"]
        })
        .count();
    m.insert("mood_changes".into(), json!(mood_changes));
    let traits = &s.table("traits").rows;
    if !traits.is_empty() {
        let count = |pred: &dyn Fn(&Row) -> bool| traits.iter().filter(|r| pred(r)).count();
        m.insert(
            "traits".into(),
            json!({
                "seed": count(&|r| r["origin"] == "seed" && LIVE_TRAIT.contains(&r["status"].as_str().unwrap_or_default())),
                "experienced": count(&|r| r["origin"] == "experienced" && LIVE_TRAIT.contains(&r["status"].as_str().unwrap_or_default())),
                "retired": count(&|r| r["status"] == "retired"),
                "revised": count(&|r| r["version"].as_i64().unwrap_or(1) > 1),
                "contested": count(&|r| LIVE_TRAIT.contains(&r["status"].as_str().unwrap_or_default()) && r["contradicting_evidence"].as_array().is_some_and(|a| !a.is_empty())),
            }),
        );
        let live: Vec<usize> = s
            .list_rows("traits", Some(LIVE_TRAIT), 10_000)
            .iter()
            .filter_map(|t| s.slots.get(t["id"].as_str().unwrap_or_default()).copied())
            .collect();
        let mut dup: Option<f64> = None;
        for (i, a) in live.iter().enumerate() {
            for b in &live[i + 1..] {
                let cos = morpho::store::vectors::relevance(st.vectors.similarity_between(*a, *b)?);
                dup = Some(dup.map_or(cos, |d| d.max(cos)));
            }
        }
        m.insert(
            "trait_dup_max_cos".into(),
            dup.map_or(Value::Null, |d| json!(round4(d))),
        );
    }
    m.insert(
        "journal_entries".into(),
        json!(s.table("journal").rows.len()),
    );
    let self_goals: Vec<&Row> = s
        .table("goals")
        .rows
        .iter()
        .filter(|g| g["origin"] == "self")
        .collect();
    m.insert(
        "wants".into(),
        json!({
            "self": self_goals.len(),
            "with_next_step": self_goals.iter().filter(|g| g.get("next_step").and_then(Value::as_str).is_some_and(|x| !x.is_empty())).count(),
            "blocked": self_goals.iter().filter(|g| g["status"] == "blocked").count(),
        }),
    );
    let rejected = s
        .events
        .iter()
        .filter(|e| e["type"] == "state_change_result")
        .flat_map(|e| e["payload"]["changes"].as_array().into_iter().flatten())
        .filter(|c| c["accepted"] == false)
        .count();
    m.insert("changes_rejected".into(), json!(rejected));
    m.insert(
        "narrative_versions".into(),
        json!(s.narrative["version"].as_i64().unwrap_or(1) - 1),
    );
    Ok(m)
}

/// How far the compiled self moved: cosine distance between its first and last version.
async fn narrative_metrics(svc: &Services) -> Result<Map<String, Value>> {
    let (first, last) = {
        let st = svc.store.lock().unwrap();
        let first = st
            .state
            .transitions
            .iter()
            .find(|t| t["table_name"] == "narrative")
            .and_then(|t| t["after"]["data"]["text"].as_str())
            .unwrap_or_default()
            .to_string();
        let last = st.state.narrative["data"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (first, last)
    };
    let mut m = Map::new();
    if first.is_empty() || last.is_empty() {
        return Ok(m);
    }
    let drift = if first == last {
        0.0
    } else {
        let v = svc.llm.embed(&[first, last]).await?;
        let dot: f32 = v[0].iter().zip(&v[1]).map(|(a, b)| a * b).sum();
        let norm = |x: &[f32]| x.iter().map(|a| a * a).sum::<f32>().sqrt();
        1.0 - (dot / (norm(&v[0]) * norm(&v[1])).max(1e-9)) as f64
    };
    m.insert("narrative_drift".into(), json!(round4(drift)));
    Ok(m)
}

fn compare(metrics: &Map<String, Value>, baseline: Option<&Map<String, Value>>) -> Vec<String> {
    let Some(baseline) = baseline else {
        return Vec::new();
    };
    let mut bad = Vec::new();
    for k in LOWER_IS_WORSE.iter().chain(&HIGHER_IS_WORSE) {
        let (Some(a), Some(b)) = (
            metrics.get(*k).and_then(Value::as_f64),
            baseline.get(*k).and_then(Value::as_f64),
        ) else {
            continue;
        };
        let tolerance = if *k == "dup_max_cos" { 1e-3 } else { 0.0 };
        let worse = if LOWER_IS_WORSE.contains(k) {
            a < b
        } else {
            a > b + tolerance
        };
        if worse {
            bad.push(format!(
                "{k}: {} vs baseline {}",
                py_str(&metrics[*k]),
                py_str(&baseline[*k])
            ));
        }
    }
    bad
}

fn print_table(metrics: &Map<String, Value>, baseline: Option<&Map<String, Value>>) {
    println!("{:16} {:>10} {:>10}", "metric", "value", "baseline");
    for k in LOWER_IS_WORSE.iter().chain(&HIGHER_IS_WORSE).chain(&INFO) {
        let Some(v) = metrics.get(*k) else { continue };
        let b = baseline
            .and_then(|b| b.get(*k))
            .filter(|b| !b.is_null())
            .map(py_str);
        println!("{k:16} {:>10} {:>10}", py_str(v), b.unwrap_or_default());
    }
}

/// A separate, stronger grader with its own cache; verdicts never count as harness calls.
fn judge_llm(stem: &str, trial: Option<u64>, strict: bool) -> Result<Option<Llm>> {
    let model = &settings().judge_model;
    if model.is_empty() {
        return Ok(None);
    }
    let inner =
        (!settings().deepinfra_api_key.is_empty() && !strict).then(|| DeepInfra::with_model(model));
    let cache = root().join("cache").join(format!(
        "{stem}{}.judge.json",
        trial.map(|n| format!(".t{n}")).unwrap_or_default()
    ));
    Ok(Some(Llm::Recording(
        Recording::new(inner, cache, strict, false)?
            .model(model)
            .temperature(settings().judge_temperature),
    )))
}

async fn rejudge(scenario: &Path, result: &Path, strict: bool) -> Result<i32> {
    let data: Value = serde_json::from_str(&std::fs::read_to_string(scenario)?)?;
    let turns = data["turns"].as_array().context("scenario has no turns")?;
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(result)?)?;
    let replies: Vec<String> = doc["replies"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| r["reply"].as_str().unwrap_or_default().to_string())
        .collect();
    let stem = result.file_stem().unwrap().to_string_lossy().to_string();
    let judge = judge_llm(&stem, None, strict)?.context("--rejudge needs JUDGE_MODEL")?;
    let metrics = behaviour(&judge, turns, &replies).await?;
    judge.save()?;
    print_table(&metrics, None);
    let out = result.with_extension("rejudged.json");
    std::fs::write(
        &out,
        serde_json::to_vec_pretty(
            &json!({"source": result, "judge_model": settings().judge_model, "metrics": metrics}),
        )?,
    )?;
    println!("wrote {}", out.display());
    Ok(0)
}

/// Scenario, label (or the baseline's) and trial: the `run` every span and metric carries.
fn run_name(
    scenario: &Path,
    label: Option<&str>,
    baseline: Option<&Path>,
    control: bool,
    trial: Option<u64>,
) -> String {
    let stem = scenario.file_stem().unwrap().to_string_lossy();
    let label = label.map(String::from).or_else(|| {
        let stem = baseline?.file_stem()?.to_str()?;
        stem.split_once('.').map(|(_, l)| l.to_string())
    });
    format!(
        "{stem}{}.{}{}",
        if control { ".control" } else { "" },
        label.unwrap_or_else(|| "replay".into()),
        trial.map(|n| format!(".t{n}")).unwrap_or_default()
    )
}

#[tracing::instrument(name = "eval", skip_all, fields(scenario = %scenario.display(),
    label = label.unwrap_or(""), trial = trial.unwrap_or(0), harness_version = 8,
    control, drop_streams = %settings().drop_streams))]
async fn run_scenario(
    scenario: &Path,
    label: Option<&str>,
    strict: bool,
    baseline: Option<&Path>,
    control: bool,
    trial: Option<u64>,
    audit_dir: Option<PathBuf>,
) -> Result<i32> {
    let data: Value = serde_json::from_str(&std::fs::read_to_string(scenario)?)?;
    if !control {
        let time = data["start_time"]
            .as_str()
            .unwrap_or("2026-09-11T12:00:00Z");
        morpho::pyfmt::set_eval_clock(
            morpho::pyfmt::parse_dt(time).context("invalid scenario start_time")?,
        )?;
    }
    let turns = data["turns"].as_array().context("scenario has no turns")?;
    let stem = scenario.file_stem().unwrap().to_string_lossy().to_string();
    let name = format!("{stem}{}", if control { ".control" } else { "" });
    let cache = root().join("cache").join(format!(
        "{name}{}{}.json",
        trial.map(|n| format!(".t{n}")).unwrap_or_default(),
        if control { "" } else { ".v8" }
    ));
    let inner = if !settings().deepinfra_api_key.is_empty() && !strict {
        Some(DeepInfra::new())
    } else {
        None
    };
    let llm = Llm::Recording(Recording::new(inner, cache, strict, control)?);
    let grader = judge_llm(&stem, trial, strict)?;
    let base: Option<Map<String, Value>> = match baseline {
        Some(p) => serde_json::from_str::<Value>(&std::fs::read_to_string(p)?)?["metrics"]
            .as_object()
            .cloned(),
        None => None,
    };
    let audit = audit_dir.map(audit::Audit::new).transpose()?;
    let dir = audit
        .as_ref()
        .map(|a| a.dir.join("database"))
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("morpho-eval-{name}-{}", std::process::id()))
        });
    if audit.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let svc = Services::new(Store::open(&dir, IdGen::seeded(&name))?, llm);
    if let Some(a) = &audit {
        a.record(
            &svc,
            "start",
            json!({"scenario": scenario, "strict": strict, "control": control}),
        )?;
    }
    let t0 = Instant::now();
    let mut metrics = Map::new();
    let outcome: Result<Vec<String>> = async {
        if control {
            let replies = run_control(&svc, turns, audit.as_ref()).await?;
            metrics.extend(behaviour(grader.as_ref().unwrap_or(&svc.llm), turns, &replies).await?);
            Ok(replies)
        } else {
            let cycle_every = data["cycle_every"].as_u64().unwrap_or(3) as usize;
            let seed = data["seed"].as_array().cloned().unwrap_or_default();
            seed_traits(&svc, &seed).await?;
            compile_narrative(&svc).await?;
            let (replies, manifests, ids) =
                run_harness(&svc, turns, cycle_every, audit.as_ref()).await?;
            metrics.extend(behaviour(grader.as_ref().unwrap_or(&svc.llm), turns, &replies).await?);
            metrics.extend(state_metrics(&svc, turns, &replies, &manifests, &ids)?);
            metrics.extend(narrative_metrics(&svc).await?);
            Ok(replies)
        }
    }
    .await;
    svc.llm.save()?;
    if let Some(j) = &grader {
        j.save()?;
    }
    if let Some(a) = &audit {
        a.record(
            &svc,
            "end",
            json!({"error": outcome.as_ref().err().map(|e| format!("{e:#}"))}),
        )?;
    }
    if audit.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let replies = match outcome {
        Ok(r) => r,
        Err(e) if is_miss(&e) => {
            let head = e.to_string().lines().next().unwrap_or_default().to_string();
            println!("cache miss (prompts changed; re-record without --strict): {head}");
            eprintln!("{e}");
            return Ok(1);
        }
        Err(e) => return Err(e),
    };
    let c = svc.llm.counts();
    metrics.insert("llm_calls".into(), json!(c.llm_calls));
    metrics.insert("prompt_tokens".into(), json!(c.prompt_tokens));
    if c.embed_calls > 0 {
        metrics.insert("embed_calls".into(), json!(c.embed_calls));
    }
    metrics.insert("completion_tokens".into(), json!(c.completion_tokens));
    metrics.insert(
        "provider_prompt_tokens".into(),
        json!(c.provider_prompt_tokens),
    );
    metrics.insert(
        "provider_completion_tokens".into(),
        json!(c.provider_completion_tokens),
    );
    metrics.insert("usage_reported_calls".into(), json!(c.usage_reported_calls));
    metrics.insert("cache_misses".into(), json!(c.cache_misses));
    metrics.insert("calls_by_kind".into(), json!(c.calls_by_kind));
    metrics.insert(
        "prompt_tokens_by_kind".into(),
        json!(c.prompt_tokens_by_kind),
    );
    metrics.insert(
        "seconds".into(),
        json!((t0.elapsed().as_secs_f64() * 10.0).round() / 10.0),
    );
    metrics.insert("turns".into(), json!(turns.len()));

    if let Some(a) = &audit {
        std::fs::write(
            a.dir.join("result.json"),
            serde_json::to_vec_pretty(&json!({"metrics": metrics, "replies": replies}))?,
        )?;
    }
    let summary = Value::Object(metrics.clone());
    tracing::info!(target: "eval", metrics = %summary, "result");
    print_table(&metrics, base.as_ref());
    let bad = compare(&metrics, base.as_ref());
    for line in &bad {
        println!("REGRESSION {line}");
    }
    if let Some(label) = label {
        let out = root().join("results").join(format!("{name}.{label}.json"));
        let replies: Vec<Value> = turns
            .iter()
            .zip(&replies)
            .map(|(t, r)| {
                let ok = if t["tag"] == json!("probe") && t["expect"].is_array() {
                    json!(judge(r, t))
                } else {
                    Value::Null
                };
                json!({"text": text(t), "reply": r, "ok": ok})
            })
            .collect();
        let doc = json!({"scenario": stem, "harness_version": 8, "control": control, "trial": trial, "drop_streams": settings().drop_streams, "judge_model": settings().judge_model, "metrics": metrics, "replies": replies});
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
        serde::Serialize::serialize(
            &doc,
            &mut serde_json::Serializer::with_formatter(&mut buf, fmt),
        )?;
        std::fs::write(&out, buf)?;
        println!("wrote {}", out.display());
    }
    Ok(if !bad.is_empty() || (strict && c.cache_misses > 0) {
        1
    } else {
        0
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    settings();
    let mut args = std::env::args().skip(1);
    let (mut scenario, mut label, mut strict, mut baseline, mut control, mut trial, mut audit_dir) =
        (None, None, false, None, false, None, None);
    let mut rejudge_path = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--audit-dir" => {
                audit_dir = Some(PathBuf::from(
                    args.next().context("--audit-dir requires a directory")?,
                ))
            }
            "--label" => label = args.next(),
            "--strict" => strict = true,
            "--baseline" => baseline = args.next().map(PathBuf::from),
            "--control" => control = true,
            "--trial" => trial = args.next().and_then(|n| n.parse().ok()),
            "--rejudge" => rejudge_path = args.next().map(PathBuf::from),
            _ => scenario = Some(PathBuf::from(a)),
        }
    }
    let scenario = scenario
        .context("usage: eval SCENARIO [--label L] [--strict] [--baseline FILE] [--control] [--trial N] [--rejudge RESULT] [--audit-dir DIR]")?;
    let run = run_name(
        &scenario,
        label.as_deref(),
        baseline.as_deref(),
        control,
        trial,
    );
    morpho::telemetry::init(
        "morpho-eval",
        tracing::level_filters::LevelFilter::WARN,
        rejudge_path.is_none().then_some(run.as_str()),
    )?;
    if let Some(result) = rejudge_path {
        let code = rejudge(&scenario, &result, strict).await?;
        morpho::telemetry::shutdown();
        std::process::exit(code)
    }
    let code = run_scenario(
        &scenario,
        label.as_deref(),
        strict,
        baseline.as_deref(),
        control,
        trial,
        audit_dir,
    )
    .await?;
    morpho::telemetry::shutdown();
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use morpho::llm::Fake;

    #[tokio::test]
    async fn judge_and_consistency_verdicts_are_averaged() {
        unsafe { std::env::set_var("JUDGE_VOTES", "1") };
        let fake = Fake::default();
        for word in ["PASS", "FAIL", "YES", "NO", "YES"] {
            fake.queue("Reply", json!({"response": word}));
        }
        let llm = Llm::Fake(fake);
        let turns = [
            json!({"text":"a","judge":"r","family":"x"}),
            json!({"text":"b","judge":"r","family":"y"}),
            json!({"text":"c","group":"g"}),
            json!({"text":"d","group":"g"}),
            json!({"text":"e","group":"g"}),
        ];
        let replies: Vec<String> = ["1", "2", "3", "4", "5"].map(String::from).to_vec();
        let m = behaviour(&llm, &turns, &replies).await.unwrap();
        assert_eq!(m["judge_accuracy"], json!(0.5));
        assert_eq!(m["judge_by_family"], json!({"x": 1.0, "y": 0.0}));
        assert_eq!(m["consistency"], json!(round4(2.0 / 3.0)));
        assert_eq!(m["probes"], json!(0));
    }
}
