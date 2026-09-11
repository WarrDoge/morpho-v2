//! Projects persistent state into a token-budgeted prompt (SPEC §12, §32).

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::Services;
use crate::config::settings;
use crate::context::ranking::score;
use crate::pyfmt::{Row, dt_of, fixed, jsonb_keys, now, py_dumps, round4, tokens, truthy};
use crate::state::models::{LIVE_BELIEF, LIVE_MEMORY};
use crate::store::Store;

pub const SECTIONS: [(&str, &str, f64); 7] = [
    ("working", "WORKING STATE", 0.10),
    ("memories", "RELEVANT MEMORIES", 0.30),
    ("beliefs", "RELEVANT BELIEFS", 0.15),
    ("world", "WORLD MODEL", 0.10),
    ("self", "SELF MODEL", 0.10),
    ("goals", "ACTIVE GOALS", 0.10),
    ("recent", "RECENT EVENTS", 0.15),
];

fn s<'a>(r: &'a Row, k: &str) -> &'a str {
    r.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn f(r: &Row, k: &str) -> f64 {
    r.get(k).and_then(Value::as_f64).unwrap_or_default()
}

fn fill(lines: &[String], budget: i64) -> (Vec<String>, i64, i64) {
    let (mut kept, mut used, mut dropped) = (Vec::new(), 0i64, 0i64);
    for line in lines {
        let t = tokens(line) as i64;
        if used + t <= budget {
            kept.push(line.clone());
            used += t;
        } else {
            dropped += 1;
        }
    }
    (kept, used, dropped)
}

fn ranked(
    st: &Store,
    table: &str,
    emb: &[f32],
    k: usize,
    statuses: &[&str],
    now: &DateTime<Utc>,
) -> Vec<Row> {
    let mut rows = st.similar(table, emb, k, Some(statuses));
    for r in rows.iter_mut() {
        let sc = score(
            r,
            f(r, "relevance").max(0.0),
            now,
            settings().memory_half_life_days,
        );
        r.insert("score".into(), json!(sc));
    }
    rows.sort_by(|a, b| f(b, "score").total_cmp(&f(a, "score")));
    rows
}

fn data(row: &Row) -> Row {
    row.get("data")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub async fn compose_text(
    svc: &Services,
    input: &str,
    budget: Option<usize>,
) -> Result<(String, Value)> {
    let emb = svc.llm.embed(&[input.to_string()]).await?.remove(0);
    compose(svc, &emb, budget)
}

pub fn compose(svc: &Services, emb: &[f32], budget: Option<usize>) -> Result<(String, Value)> {
    let budget = budget.unwrap_or(settings().context_token_budget) as i64;
    let now = now();
    let st = svc.store.lock().unwrap();
    let working = data(&st.state.working);
    let self_data = data(&st.state.self_state);
    let mems = ranked(&st, "memories", emb, 20, LIVE_MEMORY, &now);
    let beliefs = ranked(&st, "beliefs", emb, 10, LIVE_BELIEF, &now);
    let ents: Vec<Row> = working
        .get("active_entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|n| st.state.find_entity(n, None))
        .collect();
    let rels = if ents.is_empty() {
        Vec::new()
    } else {
        st.state.relationships_for(
            &ents
                .iter()
                .map(|e| s(e, "id").to_string())
                .collect::<Vec<_>>(),
        )
    };
    let mut goals = st
        .state
        .list_rows("goals", Some(&["active", "blocked"]), 100);
    goals.sort_by(|a, b| f(b, "priority").total_cmp(&f(a, "priority")));
    let recent = st.state.recent_events(10);
    drop(st);

    let sections: Vec<Vec<String>> = vec![
        jsonb_keys(&working)
            .into_iter()
            .filter(|k| k.as_str() != "recent_events")
            .filter(|k| !matches!(&working[*k], Value::Null) && !is_empty(&working[*k]))
            .map(|k| format!("{k}: {}", py_dumps(&working[k])))
            .collect(),
        mems.iter()
            .map(|r| {
                format!(
                    "[{}] ({}, imp={}, conf={}) {}",
                    s(r, "id"),
                    s(r, "kind"),
                    fixed(f(r, "importance"), 2),
                    fixed(f(r, "confidence"), 2),
                    s(r, "summary")
                )
            })
            .collect(),
        beliefs
            .iter()
            .map(|r| {
                format!(
                    "[{}] ({}, conf={}) {}",
                    s(r, "id"),
                    s(r, "status"),
                    fixed(f(r, "confidence"), 2),
                    s(r, "proposition")
                )
            })
            .collect(),
        ents.iter()
            .map(|e| {
                let attrs = e.get("attributes").cloned().unwrap_or(json!({}));
                format!("{} ({}): {}", s(e, "name"), s(e, "kind"), py_dumps(&attrs))
            })
            .chain(rels.iter().map(|r| {
                format!(
                    "{} --{}--> {}",
                    s(r, "src_name"),
                    s(r, "rel"),
                    s(r, "dst_name")
                )
            }))
            .collect(),
        jsonb_keys(&self_data)
            .into_iter()
            .filter(|k| truthy(&self_data[*k]))
            .map(|k| format!("{k}: {}", py_dumps(&self_data[k])))
            .collect(),
        goals
            .iter()
            .map(|g| {
                format!(
                    "[{}] ({}, {}, p={}) {}",
                    s(g, "id"),
                    s(g, "status"),
                    s(g, "origin"),
                    fixed(f(g, "priority"), 2),
                    s(g, "description")
                )
            })
            .collect(),
        recent
            .iter()
            .map(|e| {
                let ts = dt_of(e.get("ts")).map(|t| t.format("%Y-%m-%d %H:%M").to_string());
                let payload: String = py_dumps(&e["payload"]).chars().take(300).collect();
                format!(
                    "{} {}/{}: {payload}",
                    ts.unwrap_or_default(),
                    s(e, "source"),
                    s(e, "type")
                )
            })
            .collect(),
    ];

    let mut out: Vec<String> = Vec::new();
    let mut manifest = json!({"budget": budget, "sections": {}});
    let mut carry = 0i64;
    for ((name, title, share), lines) in SECTIONS.iter().zip(&sections) {
        let section_budget = (budget as f64 * share) as i64 + carry;
        let (kept, used, dropped) = fill(lines, section_budget);
        carry = section_budget - used;
        manifest["sections"][*name] =
            json!({"included": kept.len(), "dropped": dropped, "tokens": used});
        if !kept.is_empty() {
            out.push(format!("## {title}\n{}", kept.join("\n")));
        }
    }
    for (name, rows) in [("memories", &mems), ("beliefs", &beliefs)] {
        let included = manifest["sections"][name]["included"].as_u64().unwrap_or(0) as usize;
        manifest[name] = rows
            .iter()
            .take(included)
            .map(|r| json!({"id": s(r, "id"), "score": round4(f(r, "score"))}))
            .collect();
    }
    let text = out.join("\n\n");
    manifest["tokens"] = json!(tokens(&text));
    Ok((text, manifest))
}

/// Python `v in ([], "", {})`.
fn is_empty(v: &Value) -> bool {
    match v {
        Value::Array(a) => a.is_empty(),
        Value::String(s) => s.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}
