//! Scenario runner: metrics for one scenario, optional strict replay and baseline comparison.
//!
//! Usage: eval evals/scenarios/dana.json [--label L] [--strict] [--baseline FILE] [--control]

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use morpho::Services;
use morpho::config::settings;
use morpho::ids::IdGen;
use morpho::interact::interact;
use morpho::llm::{DeepInfra, Llm, Recording, is_miss};
use morpho::pyfmt::{Row, py_str, round4, tokens};
use morpho::state::models::{LIVE_BELIEF, LIVE_GOAL, LIVE_MEMORY};
use morpho::store::Store;
use morpho::worker::cycle;

const CONTROL_SYSTEM: &str =
    "You are a helpful assistant. Below is the transcript of your conversation so far
with the user (older turns may have been cut off). Answer the user's latest message concisely.";
const LOWER_IS_WORSE: [&str; 4] = [
    "probe_accuracy",
    "update_accuracy",
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

async fn run_harness(
    svc: &Services,
    turns: &[Value],
    cycle_every: usize,
) -> Result<(Vec<String>, Vec<Value>, Vec<String>)> {
    let (mut replies, mut manifests, mut event_ids) = (Vec::new(), Vec::new(), Vec::new());
    for (i, t) in turns.iter().enumerate() {
        let body = interact(svc, text(t), None).await?;
        replies.push(body["response"].as_str().unwrap_or_default().to_string());
        manifests.push(body["context"].clone());
        event_ids.push(body["event_id"].as_str().unwrap_or_default().to_string());
        if (i + 1) % cycle_every == 0 {
            cycle(svc, true).await?;
        }
    }
    cycle(svc, true).await?;
    Ok((replies, manifests, event_ids))
}

async fn run_control(llm: &Llm, turns: &[Value]) -> Result<Vec<String>> {
    let budget = settings().context_token_budget;
    let mut replies = Vec::new();
    let mut transcript: Vec<String> = Vec::new();
    for t in turns {
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
        let reply = llm.complete_text(&prompt, text(t)).await?;
        transcript.push(format!("user: {}", text(t)));
        transcript.push(format!("assistant: {reply}"));
        replies.push(reply);
    }
    Ok(replies)
}

fn behaviour(turns: &[Value], replies: &[String]) -> Map<String, Value> {
    let probes: Vec<(&Value, &String)> = turns
        .iter()
        .zip(replies)
        .filter(|(t, _)| t["tag"] == json!("probe"))
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
    m
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
) -> Map<String, Value> {
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
            let cos = morpho::store::vectors::relevance(st.vectors.similarity_between(*a, *b));
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
    m
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

async fn run_scenario(
    scenario: &Path,
    label: Option<&str>,
    strict: bool,
    baseline: Option<&Path>,
    control: bool,
) -> Result<i32> {
    let data: Value = serde_json::from_str(&std::fs::read_to_string(scenario)?)?;
    let turns = data["turns"].as_array().context("scenario has no turns")?;
    let stem = scenario.file_stem().unwrap().to_string_lossy().to_string();
    let name = format!("{stem}{}", if control { ".control" } else { "" });
    let cache = root().join("cache").join(format!("{name}.json"));
    let inner = if !settings().deepinfra_api_key.is_empty() && !strict {
        Some(DeepInfra::new())
    } else {
        None
    };
    let llm = Llm::Recording(Recording::new(inner, cache, strict)?);
    let base: Option<Map<String, Value>> = match baseline {
        Some(p) => serde_json::from_str::<Value>(&std::fs::read_to_string(p)?)?["metrics"]
            .as_object()
            .cloned(),
        None => None,
    };
    let dir = std::env::temp_dir().join(format!("morpho-eval-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let svc = Services::new(Store::open(&dir, IdGen::seeded(&name))?, llm);
    let t0 = Instant::now();
    let mut metrics = Map::new();
    let outcome: Result<Vec<String>> = async {
        if control {
            let replies = run_control(&svc.llm, turns).await?;
            metrics.extend(behaviour(turns, &replies));
            Ok(replies)
        } else {
            let cycle_every = data["cycle_every"].as_u64().unwrap_or(3) as usize;
            let (replies, manifests, ids) = run_harness(&svc, turns, cycle_every).await?;
            metrics.extend(behaviour(turns, &replies));
            metrics.extend(state_metrics(&svc, turns, &replies, &manifests, &ids));
            Ok(replies)
        }
    }
    .await;
    svc.llm.save()?;
    let _ = std::fs::remove_dir_all(&dir);
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
    metrics.insert("cache_misses".into(), json!(c.cache_misses));
    metrics.insert(
        "seconds".into(),
        json!((t0.elapsed().as_secs_f64() * 10.0).round() / 10.0),
    );
    metrics.insert("turns".into(), json!(turns.len()));

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
                let ok = if t["tag"] == json!("probe") {
                    json!(judge(r, t))
                } else {
                    Value::Null
                };
                json!({"text": text(t), "reply": r, "ok": ok})
            })
            .collect();
        let doc =
            json!({"scenario": stem, "control": control, "metrics": metrics, "replies": replies});
        std::fs::write(&out, serde_json::to_string_pretty(&doc)?)?;
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::level_filters::LevelFilter::WARN.into())
                .from_env_lossy(),
        )
        .with_writer(std::io::stderr)
        .init();
    let mut args = std::env::args().skip(1);
    let (mut scenario, mut label, mut strict, mut baseline, mut control) =
        (None, None, false, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--label" => label = args.next(),
            "--strict" => strict = true,
            "--baseline" => baseline = args.next().map(PathBuf::from),
            "--control" => control = true,
            _ => scenario = Some(PathBuf::from(a)),
        }
    }
    let scenario = scenario
        .context("usage: eval SCENARIO [--label L] [--strict] [--baseline FILE] [--control]")?;
    let code = run_scenario(
        &scenario,
        label.as_deref(),
        strict,
        baseline.as_deref(),
        control,
    )
    .await?;
    std::process::exit(code)
}
