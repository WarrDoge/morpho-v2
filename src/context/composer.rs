//! Disposable, budgeted views of the eight persistent memory streams.
use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    Services,
    config::settings,
    context::ranking::score,
    pyfmt::{Row, now, tokens},
    state::models::{LIVE_BELIEF, LIVE_GOAL, LIVE_MEMORY, table_for},
    store::state::State,
};

pub const SECTIONS: [(&str, &str, f64); 8] = [
    ("working", "WORKING STATE", 0.10),
    ("memories", "RELEVANT MEMORIES", 0.25),
    ("beliefs", "RELEVANT BELIEFS", 0.15),
    ("world", "WORLD MODEL", 0.10),
    ("self", "SELF MODEL", 0.10),
    ("goals", "ACTIVE GOALS", 0.10),
    ("predictions", "PREDICTIONS", 0.05),
    ("recent", "RECENT EVENTS", 0.15),
];

fn s<'a>(r: &'a Row, key: &str) -> &'a str {
    r.get(key).and_then(Value::as_str).unwrap_or_default()
}
fn f(r: &Row, key: &str) -> f64 {
    r.get(key).and_then(Value::as_f64).unwrap_or_default()
}
fn evidence(r: &Row) -> Vec<String> {
    [
        "evidence",
        "source_events",
        "supporting_evidence",
        "contradicting_evidence",
    ]
    .iter()
    .flat_map(|key| r.get(*key).and_then(Value::as_array).into_iter().flatten())
    .filter_map(Value::as_str)
    .map(String::from)
    .collect::<BTreeSet<_>>()
    .into_iter()
    .collect()
}

/// Attribution is evidence-derived, not an ownership or authorization boundary.
fn speakers(state: &State, row: &Row) -> BTreeSet<String> {
    let mut pending = evidence(row);
    let mut seen = BTreeSet::new();
    let mut out = BTreeSet::new();
    if row.contains_key("event_id") {
        out.insert(s(row, "source").to_owned());
    }
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(e) = state.event(&id) {
            if e["type"] == "user_message" {
                out.insert(s(e, "source").to_owned());
            }
        } else if let Some(r) = table_for(&id).and_then(|t| state.get(t, &id)) {
            pending.extend(evidence(r));
        }
    }
    out
}

struct Item {
    text: String,
    metadata: Value,
    kept: bool,
}
fn item(state: &State, row: &Row, fields: &[&str], reason: &str) -> Item {
    let id = row
        .get("event_id")
        .or_else(|| row.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut body = json!({"id":id, "speakers":speakers(state, row), "version":row.get("version"), "evidence_ids":evidence(row)});
    for key in fields {
        if let Some(v) = row.get(*key) {
            body[*key] = v.clone();
        }
    }
    Item {
        text: body.to_string(),
        kept: false,
        metadata: json!({"id":id,"version":row.get("version"),"evidence_ids":evidence(row),"speakers":speakers(state,row),"score":f(row,"score"),"selection_reason":reason}),
    }
}
fn singleton(state: &State, row: &Row, name: &str) -> Vec<Item> {
    row.get("data")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(k, v)| {
            k.as_str() != "recent_events"
                && !v.is_null()
                && **v != json!([])
                && **v != json!({})
                && **v != json!("")
        })
        .map(|(k, v)| {
            let mut r = row.clone();
            r.insert("id".into(), json!(name));
            r.insert(k.clone(), v.clone());
            if let Some(t) = state.transitions.iter().rev().find(|t| {
                t["table_name"] == name
                    && t["after"]["version"].as_u64() <= row["version"].as_u64()
                    && t["after"]["data"].get(k) != t["before"]["data"].get(k)
            }) && let Some(p) = state.proposals.iter().find(|p| p["id"] == t["proposal_id"])
            {
                r.insert("evidence".into(), json!(evidence(p)));
            }
            let mut view = item(state, &r, &[k], "current singleton field");
            view.metadata["field"] = json!(k);
            view
        })
        .collect()
}

pub async fn compose_text(
    svc: &Services,
    input: &str,
    budget: Option<usize>,
) -> Result<(String, Value)> {
    compose_text_as(svc, input, "user", budget).await
}
pub async fn compose_text_as(
    svc: &Services,
    input: &str,
    speaker: &str,
    budget: Option<usize>,
) -> Result<(String, Value)> {
    let emb = svc.llm.embed(&[input.to_owned()]).await?.remove(0);
    compose_for(svc, &emb, input, speaker, None, budget)
}
pub fn compose(svc: &Services, emb: &[f32], budget: Option<usize>) -> Result<(String, Value)> {
    compose_for(svc, emb, "", "user", None, budget)
}

pub fn compose_for(
    svc: &Services,
    emb: &[f32],
    input: &str,
    speaker: &str,
    exclude_event: Option<&str>,
    budget: Option<usize>,
) -> Result<(String, Value)> {
    let budget = budget.unwrap_or(settings().context_token_budget);
    let st = svc.store.lock().unwrap();
    let state = &st.state;
    let ranked = |table, count, statuses| -> Result<Vec<Row>> {
        let mut rows = st.similar(table, emb, count, Some(statuses))?;
        for r in &mut rows {
            let value = score(
                r,
                f(r, "relevance").max(0.0),
                &now(),
                settings().memory_half_life_days,
            );
            r.insert("score".into(), json!(value));
        }
        rows.sort_by(|a, b| {
            f(b, "score").total_cmp(&f(a, "score")).then_with(|| {
                speakers(state, b)
                    .contains(speaker)
                    .cmp(&speakers(state, a).contains(speaker))
            })
        });
        Ok(rows)
    };
    let memories = ranked("memories", 20, LIVE_MEMORY)?;
    let beliefs = ranked("beliefs", 10, LIVE_BELIEF)?;
    let lower = input.to_lowercase();
    let words: BTreeSet<_> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() > 2)
        .collect();
    let relevance = |r: &Row, field: &str| {
        s(r, field)
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| words.contains(s))
            .count()
    };
    let mut entity_ids: BTreeSet<String> = memories
        .iter()
        .flat_map(|m| {
            m.get("entity_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(Value::as_str)
        .map(String::from)
        .collect();
    for n in state.working["data"]["active_entities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if let Some(e) = state.find_entity(n, None) {
            entity_ids.insert(s(&e, "id").to_owned());
        }
    }
    for e in &state.table("entities").rows {
        if !s(e, "name").is_empty()
            && (lower.contains(&s(e, "name").to_lowercase())
                || s(e, "name").eq_ignore_ascii_case(speaker))
        {
            entity_ids.insert(s(e, "id").to_owned());
        }
    }
    let entities: Vec<_> = entity_ids
        .iter()
        .filter_map(|id| state.get("entities", id))
        .collect();
    let relations = state.relationships_for(&entity_ids.into_iter().collect::<Vec<_>>());
    let mut goals = state.list_rows("goals", Some(LIVE_GOAL), usize::MAX);
    goals.extend(state.list_rows("goals", Some(&["completed", "abandoned", "superseded"]), 5));
    goals.sort_by(|a, b| {
        relevance(b, "description")
            .cmp(&relevance(a, "description"))
            .then_with(|| {
                speakers(state, b)
                    .contains(speaker)
                    .cmp(&speakers(state, a).contains(speaker))
            })
            .then_with(|| f(b, "priority").total_cmp(&f(a, "priority")))
            .then_with(|| s(a, "id").cmp(s(b, "id")))
    });
    let mut predictions: Vec<_> = state
        .table("predictions")
        .rows
        .iter()
        .filter(|r| r["verified"].is_null())
        .collect();
    predictions.sort_by_key(|r| {
        (
            std::cmp::Reverse(relevance(r, "prediction")),
            s(r, "deadline"),
        )
    });
    let recent: Vec<_> = state
        .events
        .iter()
        .rev()
        .filter(|e| Some(s(e, "event_id")) != exclude_event)
        .take(10)
        .collect();
    let mut sections = vec![
        singleton(state,&state.working,"working_state"),
        memories.iter().map(|r| item(state,r,&["kind","summary","importance","confidence","status"],"similarity, importance, recency, confidence")).collect(),
        beliefs.iter().map(|r| item(state,r,&["proposition","confidence","status"],"similarity and confidence; conflicts retained")).collect(),
        entities.into_iter().map(|r| item(state,r,&["name","kind","attributes"],"input, speaker, memory or working entity")).chain(relations.iter().map(|r| item(state,r,&["src","src_name","dst","dst_name","rel","confidence"],"relationship of selected entity"))).collect(),
        singleton(state,&state.self_state,"self_state"),
        goals.iter().map(|r| item(state,r,&["description","priority","origin","status"],"input overlap, speaker attribution, priority; recent terminal goals retained")).collect(),
        predictions.into_iter().map(|r| item(state,r,&["prediction","probability","deadline","verified"],"unresolved; input overlap then deadline")).collect(),
        recent.into_iter().map(|r| {
            let mut r = r.clone();
            let content = r["payload"].to_string();
            if content.chars().count()>600 { r.insert("payload".into(),json!({"excerpt":content.chars().take(600).collect::<String>(),"truncated":true})); }
            item(state,&r,&["ts","source","type","payload"],"recent observation; current request excluded")
        }).collect(),
    ];
    drop(st);
    let headers: usize = SECTIONS
        .iter()
        .map(|(_, title, _)| tokens(&format!("## {title}\n\n")))
        .sum();
    let available = budget.saturating_sub(headers);
    let mut remaining = available;
    for ((_, _, share), items) in SECTIONS.iter().zip(&mut sections) {
        let mut allowance = (available as f64 * share) as usize;
        for i in items {
            let cost = tokens(&i.text);
            if cost <= allowance {
                i.kept = true;
                allowance -= cost;
                remaining -= cost;
            }
        }
    }
    // Give unused shares back, prioritizing commitments before optional recall.
    for index in [5, 0, 1, 2, 3, 4, 6, 7] {
        for i in &mut sections[index] {
            let cost = tokens(&i.text);
            if !i.kept && cost <= remaining {
                i.kept = true;
                remaining -= cost;
            }
        }
    }
    let mut out = Vec::new();
    let mut manifest = json!({"budget":budget,"sections":{},"speaker":speaker});
    for ((name, title, _), items) in SECTIONS.iter().zip(&sections) {
        let kept: Vec<_> = items.iter().filter(|i| i.kept).collect();
        if !kept.is_empty() {
            out.push(format!(
                "## {title}\n{}",
                kept.iter()
                    .map(|i| i.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        manifest[*name] = kept.iter().map(|i| i.metadata.clone()).collect();
        manifest["sections"][*name] = json!({"included":kept.len(),"dropped":items.len()-kept.len(),"tokens":kept.iter().map(|i|tokens(&i.text)).sum::<usize>(),"items":items.iter().map(|i| {let mut m=i.metadata.clone();m["included"]=json!(i.kept);m["omission_reason"]=if i.kept {Value::Null} else {json!("context token budget")};m}).collect::<Vec<_>>()});
    }
    let text = out.join("\n\n");
    manifest["tokens"] = json!(if text.is_empty() { 0 } else { tokens(&text) });
    debug_assert!(manifest["tokens"].as_u64().unwrap() <= budget as u64);
    Ok((text, manifest))
}
