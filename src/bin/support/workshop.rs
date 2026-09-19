//! The workshop: a morphling (or a conventional agent) works through coding tasks in a sandbox,
//! graded by hidden tests it never sees and by features of what it did.
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};

use morpho::Services;
use morpho::agents::act::{self, Arm, Assignment, Limits, SESSION};
use morpho::ids::IdGen;
use morpho::llm::{DeepInfra, Llm, Recording, is_miss};
use morpho::pyfmt::round4;
use morpho::store::Store;
use morpho::workshop;

use super::{drain, pin_clock, root};

const VOLATILE: [&str; 5] = [
    "seconds",
    "cache_misses",
    "provider_prompt_tokens",
    "provider_completion_tokens",
    "usage_reported_calls",
];

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(to);
    let status = Command::new("cp").arg("-a").arg(from).arg(to).status()?;
    ensure!(status.success(), "copy {} failed", from.display());
    Ok(())
}

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("morpho-{name}-{}", std::process::id()))
}

/// Hidden test files run one by one on a copy of the workspace: (passed, total) each.
fn grade(ws: &Path, hidden: &[PathBuf]) -> Result<Vec<(usize, usize)>> {
    let copy = temp("grade");
    copy_dir(ws, &copy)?;
    let mut out = Vec::new();
    for (i, file) in hidden.iter().enumerate() {
        let source = std::fs::read_to_string(file)?;
        let total = source.matches("def test_").count();
        let module = format!("_hidden_t{}", i + 1);
        std::fs::write(copy.join(format!("{module}.py")), &source)?;
        let o = workshop::run(&copy, &format!("python3 -m unittest {module}"))?;
        let count = |key: &str| {
            regex::Regex::new(&format!(r"{key}=(\d+)"))
                .unwrap()
                .captures(&o.text)
                .map_or(0, |c| c[1].parse::<usize>().unwrap_or(0))
        };
        let passed = if !o.text.contains("\nRan ") && !o.text.starts_with("Ran ") {
            0
        } else if o.exit == 0 {
            total
        } else {
            total.saturating_sub(count("failures") + count("errors"))
        };
        out.push((passed, total));
    }
    std::fs::remove_dir_all(&copy)?;
    Ok(out)
}

fn files(ws: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for path in workshop::list(ws)?.text.lines() {
        let path = path.trim_start_matches("./");
        if path.ends_with(".py") || path.ends_with(".md") {
            out.insert(path.to_string(), workshop::read(ws, path)?.text);
        }
    }
    Ok(out)
}

fn norm(path: &str) -> String {
    path.trim_start_matches("/work/")
        .trim_start_matches("./")
        .to_string()
}

/// The runs of the house-rule audit the assignment asks for, in order.
fn audit_runs(task: &Value) -> Vec<&Value> {
    task["log"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            r["tool"] == "run" && r["target"].as_str().is_some_and(|c| c.contains("check.py"))
        })
        .collect()
}

fn is_test_run(row: &Value) -> bool {
    row["tool"] == "run" && row["target"].as_str().is_some_and(|c| c.contains("test"))
}

fn line_distance(a: &str, b: &str) -> f64 {
    let a: HashSet<&str> = a.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let b: HashSet<&str> = b.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let union = a.union(&b).count();
    if union == 0 {
        0.0
    } else {
        1.0 - a.intersection(&b).count() as f64 / union as f64
    }
}

/// Habit flags for one task; None where the task gave no occasion.
fn flags(log: &[Value], existing: &HashSet<String>) -> Value {
    let acts: Vec<&Value> = log.iter().filter(|r| r["kind"] == "act").collect();
    let first_write = acts.iter().position(|r| r["tool"] == "write");
    let test_first = first_write.map(|w| acts[..w].iter().any(|r| is_test_run(r)));
    let mut seen = HashSet::new();
    let mut read_before_write = None;
    for r in &acts {
        let path = norm(r["target"].as_str().unwrap_or(""));
        match r["tool"].as_str() {
            Some("read") => {
                seen.insert(path);
            }
            Some("write") => {
                if existing.contains(&path) {
                    read_before_write =
                        Some(read_before_write.unwrap_or(true) && seen.contains(&path));
                }
                seen.insert(path);
            }
            _ => {}
        }
    }
    let done = acts.last().filter(|r| r["tool"] == "done");
    let green_before_done = done.map(|_| {
        acts.iter()
            .rev()
            .find(|r| r["tool"] == "run")
            .is_some_and(|r| r["exit"] == 0)
    });
    let wrote_tests = acts.iter().any(|r| {
        r["tool"] == "write" && norm(r["target"].as_str().unwrap_or("")).starts_with("tests/")
    });
    let mut verify: BTreeMap<&str, usize> = BTreeMap::new();
    for r in acts.iter().filter(|r| r["tool"] == "run") {
        let c = r["target"].as_str().unwrap_or("");
        let kind = if c.contains("pytest") {
            "pytest"
        } else if c.contains("unittest") {
            "unittest"
        } else if c.contains(" -c ") {
            "inline"
        } else if c.contains(".py") {
            "script"
        } else {
            "shell"
        };
        *verify.entry(kind).or_default() += 1;
    }
    json!({"test_first": test_first, "read_before_write": read_before_write,
        "green_before_done": green_before_done, "wrote_tests": wrote_tests, "verify": verify})
}

/// What followed each confident miss: a blind retry, recovery, and how much the code changed.
fn surprises(log: &[Value]) -> Value {
    let acts: Vec<&Value> = log.iter().filter(|r| r["kind"] == "act").collect();
    let (mut n, mut blind, mut recovered, mut steps, mut change) =
        (0, 0, 0, Vec::new(), Vec::new());
    for (i, r) in acts.iter().enumerate() {
        if r["surprise"] != true {
            continue;
        }
        n += 1;
        let written_before: Option<&Value> = acts[..i]
            .iter()
            .rev()
            .find(|w| w["tool"] == "write")
            .copied();
        if let Some(next) = acts.get(i + 1) {
            let rerun = next["tool"] == "run" && next["target"] == r["target"];
            let rewrite = next["tool"] == "write"
                && written_before.is_some_and(|w| {
                    w["target"] == next["target"] && w["content"] == next["content"]
                });
            if rerun || rewrite {
                blind += 1;
            }
        }
        if let Some(k) = acts[i + 1..]
            .iter()
            .position(|x| x["tool"] == "run" && x["exit"] == 0)
        {
            recovered += 1;
            steps.push(k + 1);
        }
        if let Some(w) = written_before
            && let Some(next) = acts[i + 1..]
                .iter()
                .find(|x| x["tool"] == "write" && x["target"] == w["target"])
        {
            change.push(line_distance(
                w["content"].as_str().unwrap_or(""),
                next["content"].as_str().unwrap_or(""),
            ));
        }
    }
    json!({"surprises": n, "blind_retry": blind, "recovered": recovered,
        "steps_to_green": mean_usize(&steps), "change_after_surprise": mean(&change)})
}

/// What followed each stall think: a repeat of the step before it, and a passing check later.
fn stalls(log: &[Value]) -> Value {
    let (mut n, mut blind, mut checked) = (0, 0, 0);
    for (i, r) in log.iter().enumerate() {
        if r["kind"] != "think" || r["trigger"] != "stall" {
            continue;
        }
        n += 1;
        let before = log[..i].iter().rfind(|x| x["kind"] == "act");
        let after = log[i + 1..].iter().find(|x| x["kind"] == "act");
        if let (Some(b), Some(a)) = (before, after)
            && a["tool"] == b["tool"]
            && a["target"] == b["target"]
        {
            blind += 1;
        }
        if log[i + 1..].iter().any(|x| {
            x["kind"] == "act"
                && x["tool"] == "run"
                && x["exit"] == 0
                && x["expect_success"] == true
                && act::is_check(x["target"].as_str().unwrap_or(""))
        }) {
            checked += 1;
        }
    }
    json!({"thinks": n, "blind_retry": blind, "check_after": checked})
}

/// `<scenario>.<arm>[-drop-<streams>].t<n>`: the cache, result and telemetry name of a run.
pub fn run_name(scenario: &Path, arm: Arm, trial: Option<u64>) -> String {
    let drop = &morpho::config::settings().drop_streams;
    format!(
        "{}.{}{}{}",
        scenario.file_stem().unwrap().to_string_lossy(),
        arm.name(),
        if drop.is_empty() {
            String::new()
        } else {
            format!("-drop-{}", drop.replace(',', "-"))
        },
        trial.map(|n| format!(".t{n}")).unwrap_or_default()
    )
}

fn mean(xs: &[f64]) -> Value {
    if xs.is_empty() {
        Value::Null
    } else {
        json!(round4(xs.iter().sum::<f64>() / xs.len() as f64))
    }
}

fn mean_usize(xs: &[usize]) -> Value {
    mean(&xs.iter().map(|&x| x as f64).collect::<Vec<_>>())
}

fn brier(runs: &[&Value]) -> Value {
    let scores: Vec<f64> = runs
        .iter()
        .filter_map(|r| {
            let c = r["confidence"].as_f64()?;
            let p = if r["expect_success"] == true {
                c
            } else {
                1.0 - c
            };
            let ok = if r["exit"] == 0 { 1.0 } else { 0.0 };
            Some((p - ok) * (p - ok))
        })
        .collect();
    mean(&scores)
}

/// Share of tasks 2..: agreeing with the run's majority value, per flag.
fn stability(tasks: &[Value]) -> Value {
    let mut out = Map::new();
    for flag in [
        "test_first",
        "read_before_write",
        "green_before_done",
        "wrote_tests",
    ] {
        let values: Vec<bool> = tasks
            .iter()
            .skip(1)
            .filter_map(|t| t["flags"][flag].as_bool())
            .collect();
        if values.is_empty() {
            out.insert(flag.into(), Value::Null);
            continue;
        }
        let yes = values.iter().filter(|v| **v).count();
        let majority = yes.max(values.len() - yes);
        out.insert(
            flag.into(),
            json!({"majority": yes * 2 >= values.len(), "share": round4(majority as f64 / values.len() as f64), "n": values.len()}),
        );
    }
    Value::Object(out)
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(name = "workshop", skip_all, fields(arm = arm.name(), trial = trial.unwrap_or(0)))]
pub async fn run(
    scenario: &Path,
    arm: Arm,
    trial: Option<u64>,
    strict: bool,
    baseline: Option<&Path>,
) -> Result<i32> {
    let data: Value = serde_json::from_str(&std::fs::read_to_string(scenario)?)?;
    pin_clock(&data)?;
    let name = run_name(scenario, arm, trial);
    let inner =
        (!morpho::config::settings().deepinfra_api_key.is_empty() && !strict).then(DeepInfra::new);
    let cache = root().join("cache").join(format!("{name}.json"));
    let llm = Llm::Recording(Recording::new(inner, cache, strict, false)?);
    let db = temp(&format!("{name}-db"));
    let ws = temp(&format!("{name}-ws"));
    let _ = std::fs::remove_dir_all(&db);
    copy_dir(
        &root().join(data["workspace"].as_str().context("workspace")?),
        &ws,
    )?;
    let svc = Services::new(Store::open(&db, IdGen::seeded(&name))?, llm);
    let lim = &data["limits"];
    let limit = |k: &str, d: u64| lim[k].as_u64().unwrap_or(d) as usize;
    let limits = Limits {
        steps: limit("steps", 20),
        thinks: limit("thinks", 3),
    };
    let t0 = Instant::now();
    let outcome = work(&svc, &ws, arm, &data, &limits, limit("idle_steps", 6)).await;
    svc.llm.save()?;
    let (tasks, idle, maintenance) = match outcome {
        Ok(v) => v,
        Err(e) if is_miss(&e) => {
            println!("cache miss (prompts changed; re-record without --strict)");
            eprintln!("{e}");
            return Ok(1);
        }
        Err(e) => return Err(e),
    };
    let style: Value = {
        let copy = temp("style");
        copy_dir(&ws, &copy)?;
        std::fs::copy(root().join("workshop/style.py"), copy.join("_style.py"))?;
        let o = workshop::run(&copy, "python3 _style.py")?;
        std::fs::remove_dir_all(&copy)?;
        serde_json::from_str(o.text.trim()).unwrap_or(json!({"error": o.text}))
    };
    let final_files = files(&ws)?;
    let st = svc.store.lock().unwrap();
    let s = &st.state;
    let observation_ids: HashSet<&str> = s
        .events
        .iter()
        .filter(|e| e["type"] == "observation")
        .filter_map(|e| e["event_id"].as_str())
        .collect();
    let traits: Vec<Value> = s
        .table("traits")
        .rows
        .iter()
        .map(|t| {
            let cites = |k: &str| {
                t[k].as_array()
                    .into_iter()
                    .flatten()
                    .filter(|id| id.as_str().is_some_and(|id| observation_ids.contains(id)))
                    .count()
            };
            json!({"kind": t["kind"], "statement": t["statement"], "confidence": t["confidence"],
                "status": t["status"], "observations_supporting": cites("supporting_evidence"),
                "observations_contradicting": cites("contradicting_evidence")})
        })
        .collect();
    let loops: Vec<Value> = s
        .table("goals")
        .rows
        .iter()
        .filter(|g| g.contains_key("kind"))
        .map(|g| json!({"kind": g["kind"], "description": g["description"], "status": g["status"]}))
        .collect();
    let mut goals: BTreeMap<String, usize> = BTreeMap::new();
    for g in &s.table("goals").rows {
        *goals
            .entry(g["origin"].as_str().unwrap_or("").to_string())
            .or_default() += 1;
    }
    let journal: Vec<Value> = s
        .table("journal")
        .rows
        .iter()
        .map(|j| j["entry"].clone())
        .collect();
    let narrative = s.narrative.get("data").map(|d| d["text"].clone());
    let self_state = s.self_state.get("data").cloned();
    let memories: Vec<Value> = s
        .list_rows("memories", Some(morpho::state::models::LIVE_MEMORY), 100)
        .iter()
        .map(|m| {
            let f = |k: &str| m.get(k).cloned().unwrap_or(json!(0));
            json!({"id": m["id"], "summary": m["summary"], "successes": f("successes"),
                   "failures": f("failures"), "access_count": f("access_count"),
                   "created_at": f("created_at"), "salience": f("salience")})
        })
        .collect();
    drop(st);

    let all_logs: Vec<&Value> = tasks
        .iter()
        .flat_map(|t| t["log"].as_array().into_iter().flatten())
        .chain(idle["log"].as_array().into_iter().flatten())
        .collect();
    let runs: Vec<&Value> = all_logs
        .iter()
        .copied()
        .filter(|r| r["kind"] == "act" && r["tool"] == "run")
        .collect();
    let half = runs.len() / 2;
    let thinks = |trigger: &str| {
        all_logs
            .iter()
            .filter(|r| r["kind"] == "think" && r["trigger"] == trigger)
            .count()
    };
    let task_logs: Vec<Value> = tasks
        .iter()
        .flat_map(|t| t["log"].as_array().cloned().unwrap_or_default())
        .collect();
    let (passed, total) =
        idle["hidden_after"]
            .as_array()
            .into_iter()
            .flatten()
            .fold((0, 0), |(p, t), h| {
                (
                    p + h[0].as_u64().unwrap_or(0),
                    t + h[1].as_u64().unwrap_or(0),
                )
            });
    let c = svc.llm.counts();
    let mut metrics = Map::new();
    metrics.insert(
        "hidden_final".into(),
        json!(round4(passed as f64 / total.max(1) as f64)),
    );
    metrics.insert(
        "hidden_by_task".into(),
        json!(
            tasks
                .iter()
                .map(|t| t["hidden_task"].clone())
                .collect::<Vec<_>>()
        ),
    );
    metrics.insert(
        "hidden_all_by_task".into(),
        json!(
            tasks
                .iter()
                .map(|t| t["hidden_all"].clone())
                .collect::<Vec<_>>()
        ),
    );
    metrics.insert(
        "steps_by_task".into(),
        json!(tasks.iter().map(|t| t["acts"].clone()).collect::<Vec<_>>()),
    );
    metrics.insert(
        "env_mistakes_by_task".into(),
        json!(
            tasks
                .iter()
                .map(|t| t["env_mistakes"].clone())
                .collect::<Vec<_>>()
        ),
    );
    metrics.insert(
        "ended_by_task".into(),
        json!(tasks.iter().map(|t| t["ended"].clone()).collect::<Vec<_>>()),
    );
    metrics.insert(
        "prompt_tokens_by_task".into(),
        json!(
            tasks
                .iter()
                .map(|t| t["prompt_tokens"].clone())
                .collect::<Vec<_>>()
        ),
    );
    metrics.insert(
        "tokens_per_hidden_pass".into(),
        json!(c.prompt_tokens.checked_div(passed)),
    );
    // The house-rule audit the assignment asks for: how often it ran, whether it followed the
    // last write, and whether it passed first time, which is what a habit buys over a fix.
    let audits = |t: &Value| audit_runs(t).len();
    metrics.insert(
        "check_first_pass".into(),
        json!(
            tasks
                .iter()
                .filter(|t| audit_runs(t).first().is_some_and(|r| r["exit"] == 0))
                .count()
        ),
    );
    metrics.insert(
        "check_runs_by_task".into(),
        json!(tasks.iter().map(audits).collect::<Vec<_>>()),
    );
    metrics.insert(
        "check_rate".into(),
        json!(round4(
            tasks
                .iter()
                .filter(|t| {
                    let log: Vec<&Value> = t["log"].as_array().into_iter().flatten().collect();
                    log.iter()
                        .rposition(|r| r["tool"] == "write")
                        .is_some_and(|w| {
                            log[w..].iter().any(|r| {
                                r["exit"] == 0
                                    && r["target"].as_str().is_some_and(|c| c.contains("check.py"))
                            })
                        })
                })
                .count() as f64
                / tasks.len().max(1) as f64
        )),
    );
    let mut best = 0.0;
    metrics.insert(
        "regressions".into(),
        json!(
            tasks
                .iter()
                .filter(|t| {
                    let all = t["hidden_all"].as_f64().unwrap_or(0.0);
                    let fell = all + 1e-9 < best;
                    best = best.max(all);
                    fell
                })
                .count()
        ),
    );
    metrics.insert("think_self".into(), json!(thinks("self")));
    metrics.insert("think_surprise".into(), json!(thinks("surprise")));
    metrics.insert("think_stall".into(), json!(thinks("stall")));
    metrics.insert(
        "stall".into(),
        stalls(
            &tasks
                .iter()
                .flat_map(|t| t["log"].as_array().cloned().unwrap_or_default())
                .collect::<Vec<_>>(),
        ),
    );
    metrics.insert(
        "recalls".into(),
        json!(all_logs.iter().filter(|r| r["recall"].is_array()).count()),
    );
    let mut loop_ops: BTreeMap<String, usize> = BTreeMap::new();
    for r in all_logs.iter().filter(|r| r["kind"] == "loop") {
        *loop_ops
            .entry(format!(
                "{}:{}",
                r["op"].as_str().unwrap_or(""),
                r["loop"].as_str().unwrap_or("")
            ))
            .or_default() += 1;
    }
    metrics.insert("loops".into(), json!(loop_ops));
    metrics.insert(
        "idle_loops_closed".into(),
        json!(
            idle["log"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["kind"] == "loop" && r["op"] == "close")
                .count()
        ),
    );
    metrics.insert(
        "credit_updates".into(),
        json!(
            all_logs
                .iter()
                .filter(|r| r["kind"] == "credit")
                .map(|r| r["memories"].as_u64().unwrap_or(0))
                .sum::<u64>()
        ),
    );
    for t in &tasks {
        if let Some(m) = t["metric"].as_str() {
            metrics.insert(m.into(), t["hidden_task"].clone());
        }
    }
    metrics.insert(
        "asks".into(),
        json!(all_logs.iter().filter(|r| r["tool"] == "ask").count()),
    );
    metrics.insert("surprise".into(), surprises(&task_logs));
    metrics.insert("brier_first_half".into(), brier(&runs[..half]));
    metrics.insert("brier_second_half".into(), brier(&runs[half..]));
    metrics.insert(
        "habits".into(),
        json!(tasks.iter().map(|t| t["flags"].clone()).collect::<Vec<_>>()),
    );
    metrics.insert("habit_stability".into(), stability(&tasks));
    metrics.insert("style".into(), style);
    metrics.insert("experienced_traits".into(), json!(traits.len()));
    metrics.insert(
        "traits_citing_observations".into(),
        json!(
            traits
                .iter()
                .filter(|t| t["observations_supporting"].as_u64().unwrap_or(0)
                    + t["observations_contradicting"].as_u64().unwrap_or(0)
                    > 0)
                .count()
        ),
    );
    metrics.insert("goals_by_origin".into(), json!(goals));
    metrics.insert("journal_entries".into(), json!(journal.len()));
    metrics.insert("maintenance_skipped".into(), json!(maintenance));
    metrics.insert("idle_acts".into(), idle["acts"].clone());
    metrics.insert(
        "idle_writes".into(),
        json!(
            idle["log"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["tool"] == "write")
                .count()
        ),
    );
    metrics.insert("idle_hidden_change".into(), idle["hidden_change"].clone());
    metrics.insert("llm_calls".into(), json!(c.llm_calls));
    metrics.insert("embed_calls".into(), json!(c.embed_calls));
    metrics.insert("prompt_tokens".into(), json!(c.prompt_tokens));
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
    metrics.insert("calls_by_kind".into(), json!(c.calls_by_kind));
    metrics.insert(
        "prompt_tokens_by_kind".into(),
        json!(c.prompt_tokens_by_kind),
    );
    metrics.insert("cache_misses".into(), json!(c.cache_misses));
    metrics.insert(
        "seconds".into(),
        json!((t0.elapsed().as_secs_f64() * 10.0).round() / 10.0),
    );

    let doc = json!({"scenario": scenario.file_stem().unwrap().to_string_lossy(), "arm": arm.name(),
        "trial": trial, "drop_streams": morpho::config::settings().drop_streams, "workshop_version": 3,
        "metrics": metrics, "tasks": tasks, "idle": idle, "traits": traits,
        "loops": loops, "journal": journal, "self_state": self_state, "memories": memories,
        "narrative": narrative, "files": final_files});
    for (k, v) in &metrics {
        if !matches!(k.as_str(), "calls_by_kind" | "prompt_tokens_by_kind") {
            println!("{k:28} {v}");
        }
    }
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&db);
    let mut code = 0;
    if let Some(b) = baseline {
        let base: Value = serde_json::from_str(&std::fs::read_to_string(b)?)?;
        for (k, v) in base["metrics"].as_object().into_iter().flatten() {
            if !VOLATILE.contains(&k.as_str()) && doc["metrics"][k] != *v {
                println!("MISMATCH {k}: {} vs {v}", doc["metrics"][k]);
                code = 1;
            }
        }
        if base["tasks"] != doc["tasks"] || base["idle"] != doc["idle"] {
            println!("MISMATCH action log");
            code = 1;
        }
    }
    if strict && c.cache_misses > 0 {
        code = 1;
    }
    if !strict {
        let out = root().join("results").join(format!("{name}.json"));
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
        serde::Serialize::serialize(
            &doc,
            &mut serde_json::Serializer::with_formatter(&mut buf, fmt),
        )?;
        std::fs::write(&out, buf)?;
        println!("wrote {}", out.display());
    }
    Ok(code)
}

type Outcome = (Vec<Value>, Value, usize);

async fn work(
    svc: &Services,
    ws: &Path,
    arm: Arm,
    data: &Value,
    limits: &Limits,
    idle_steps: usize,
) -> Result<Outcome> {
    let skipped = |stats: &[Value]| {
        stats
            .iter()
            .filter(|s| !s["maintenance"].is_null() || !s["error"].is_null())
            .count()
    };
    let mut maintenance = 0;
    let mut hidden = Vec::new();
    let mut tasks = Vec::new();
    for (i, task) in data["tasks"]
        .as_array()
        .context("tasks")?
        .iter()
        .enumerate()
    {
        let text = task["text"].as_str().context("task text")?;
        hidden.push(root().join(task["hidden"].as_str().context("hidden")?));
        let existing: HashSet<String> = workshop::list(ws)?.text.lines().map(norm).collect();
        let emb = svc.llm.embed(&[text.to_string()]).await?.remove(0);
        let event_id = {
            let mut st = svc.store.lock().unwrap();
            let slot = st.vectors.append(&emb)?;
            st.append_event(
                "user_message",
                "user",
                json!({"text": text, "request_id": format!("workshop-{i}")}),
                Some(SESSION),
                Some(slot),
            )?["event_id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let before = svc.llm.counts();
        let assignment = Assignment {
            event_id: &event_id,
            text,
        };
        let log = act::work(svc, ws, arm, Some(&assignment), limits).await?;
        if arm.stateful() {
            maintenance += skipped(&drain(svc, None).await?);
        }
        let after = svc.llm.counts();
        let grades = grade(ws, &hidden)?;
        let (p, t) = grades.iter().fold((0, 0), |(p, t), (a, b)| (p + a, t + b));
        let last = grades[i];
        let acts: Vec<&Value> = log.iter().filter(|r| r["kind"] == "act").collect();
        let ended = match acts.last().and_then(|r| r["tool"].as_str()) {
            Some("done") => "done",
            Some("rest") => "rest",
            _ if acts.len() >= limits.steps => "limit",
            _ => "empty",
        };
        // Commands the workspace cannot run: a missing interpreter, an absent test runner or a
        // module off the path.
        let env_mistakes = acts
            .iter()
            .filter(|r| {
                r["tool"] == "run"
                    && (r["exit"] == 127
                        || r["target"].as_str().is_some_and(|c| c.contains("pytest"))
                        || r["output"]
                            .as_str()
                            .is_some_and(|o| o.contains("No module named")))
            })
            .count();
        tasks.push(
            json!({"task": i + 1, "text": text, "ended": ended, "acts": acts.len(), "metric": task["metric"],
            "env_mistakes": env_mistakes,
            "flags": flags(&log, &existing),
            "hidden_task": round4(last.0 as f64 / last.1.max(1) as f64),
            "hidden_all": round4(p as f64 / t.max(1) as f64),
            "prompt_tokens": after.prompt_tokens - before.prompt_tokens,
            "calls": after.llm_calls - before.llm_calls, "log": log}),
        );
    }
    let score = |g: &[(usize, usize)]| {
        let (p, t) = g.iter().fold((0, 0), |(p, t), (a, b)| (p + a, t + b));
        p as f64 / t.max(1) as f64
    };
    let before = grade(ws, &hidden)?;
    let log = if arm.stateful() {
        let idle = Limits {
            steps: idle_steps,
            ..*limits
        };
        let log = act::work(svc, ws, arm, None, &idle).await?;
        maintenance += skipped(&drain(svc, None).await?);
        log
    } else {
        Vec::new()
    };
    let after = grade(ws, &hidden)?;
    let idle = json!({"acts": log.iter().filter(|r| r["kind"] == "act").count(), "log": log,
        "hidden_change": round4(score(&after) - score(&before)),
        "hidden_after": after.iter().map(|(p, t)| json!([p, t])).collect::<Vec<_>>()});
    Ok((tasks, idle, maintenance))
}
