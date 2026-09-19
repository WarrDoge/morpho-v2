//! Disposable, budgeted views of the persistent memory streams.
use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    Services,
    config::settings,
    context::ranking::score,
    pyfmt::{Row, now, tokens},
    state::models::{LIVE_BELIEF, LIVE_GOAL, LIVE_MEMORY, LIVE_TRAIT, MESSAGE_TYPES, table_for},
    store::{Store, state::State},
};

pub const SECTIONS: [(&str, &str, f64); 10] = [
    ("working", "WORKING STATE", 0.10),
    ("memories", "RELEVANT MEMORIES", 0.23),
    ("beliefs", "RELEVANT BELIEFS", 0.13),
    ("journal", "MY NOTES", 0.04),
    ("world", "WORLD MODEL", 0.10),
    ("self", "SELF MODEL", 0.10),
    ("goals", "ACTIVE GOALS", 0.10),
    ("predictions", "PREDICTIONS", 0.05),
    ("recent", "RECENT EVENTS", 0.05),
    ("said", "WHAT I SAID", 0.10),
];
/// Sections ranked together and filled from one shared allowance.
const POOL: [usize; 3] = [1, 2, 3];
/// Memories scanned for the speaker stream, and how many of them it may show.
const SPEAKER_SCAN: usize = 200;
const SPEAKER_ITEMS: usize = 8;

/// `SPEAKER_SHARE` takes its cut off the ten base sections. At 0.0 the multiply is exact
/// and no eleventh header is emitted, so composition is unchanged when the knob is unset.
fn layout(share: f64) -> Vec<(&'static str, &'static str, f64)> {
    SECTIONS
        .iter()
        .map(|&(name, title, base)| (name, title, base * (1.0 - share)))
        .chain((share > 0.0).then_some(("speaker", "WHO I'M TALKING TO", share)))
        .collect()
}
/// Pooled items this close restate each other; the newer one stays.
const POOL_DUP_COS: f64 = 0.92;

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

/// One hop of Honcho's reasoning chain: a conclusion carries the text of what it cites,
/// not just the ids. Empty unless `PREMISE_CHARS` is set, so prompts are unchanged when off.
fn grounds(state: &State, row: &Row) -> Vec<Value> {
    let cap = settings().premise_chars;
    if cap == 0 {
        return Vec::new();
    }
    evidence(row)
        .iter()
        .filter_map(|id| {
            let text = match state.event(id) {
                Some(e) => e["payload"].to_string(),
                None => {
                    let r = table_for(id).and_then(|t| state.get(t, id))?;
                    ["summary", "proposition", "statement", "entry"]
                        .iter()
                        .find_map(|k| r.get(*k).and_then(Value::as_str))
                        .unwrap_or_default()
                        .to_owned()
                }
            };
            (!text.is_empty())
                .then(|| json!({"id": id, "text": text.chars().take(cap).collect::<String>()}))
        })
        .take(3)
        .collect()
}

struct Item {
    text: String,
    metadata: Value,
    kept: bool,
    score: f64,
    omit: Option<String>,
}
fn item(state: &State, row: &Row, fields: &[&str], reason: &str) -> Item {
    item_with(state, row, fields, reason, false)
}
fn item_with(state: &State, row: &Row, fields: &[&str], reason: &str, cite: bool) -> Item {
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
    if cite {
        let g = grounds(state, row);
        if !g.is_empty() {
            body["grounds"] = Value::Array(g);
        }
    }
    Item {
        text: body.to_string(),
        kept: false,
        score: f(row, "score"),
        omit: None,
        metadata: json!({"id":id,"version":row.get("version"),"evidence_ids":evidence(row),"speakers":speakers(state,row),"score":f(row,"score"),"selection_reason":reason}),
    }
}
/// `MORPHO_DROP_STREAMS` names streams to ablate from every prompt.
pub fn dropped(name: &str) -> bool {
    settings().drop_streams.split(',').any(|n| n == name)
}

/// Personality shapes attention: the retrieval query leans toward what the agent cares about.
fn biased_query(st: &Store, emb: &[f32]) -> Vec<f32> {
    let bias = settings().identity_bias as f32;
    let slots: Vec<usize> = live_traits(&st.state)
        .iter()
        .filter_map(|r| st.state.slots.get(s(r, "id")).copied())
        .collect();
    if bias == 0.0 || slots.is_empty() || dropped("identity") {
        return emb.to_vec();
    }
    let mut centroid = vec![0f32; emb.len()];
    for slot in &slots {
        if let Ok(v) = st.vectors.get(*slot) {
            for (c, x) in centroid.iter_mut().zip(v) {
                *c += x;
            }
        }
    }
    let n = slots.len() as f32;
    let mut q: Vec<f32> = emb
        .iter()
        .zip(&centroid)
        .map(|(e, c)| e + bias * c / n)
        .collect();
    let norm = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    q.iter_mut().for_each(|x| *x /= norm);
    q
}

fn live_traits(state: &State) -> Vec<&Row> {
    state
        .table("traits")
        .rows
        .iter()
        .filter(|r| shown_trait(r))
        .collect()
}

/// A live trait.
pub fn shown_trait(r: &Row) -> bool {
    LIVE_TRAIT.contains(&s(r, "status"))
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    (dot / (na * nb).max(1e-9)) as f64
}

/// Near-duplicates across the pooled rows, in score order: the newer statement supersedes,
/// the rest are omitted with the reason.
fn dedupe(st: &Store, rows: &[&Row]) -> Vec<Option<String>> {
    let vectors: Vec<Option<Vec<f32>>> = rows
        .iter()
        .map(|r| {
            st.state
                .slots
                .get(s(r, "id"))
                .and_then(|slot| st.vectors.get(*slot).ok())
        })
        .collect();
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        f(rows[b], "score")
            .total_cmp(&f(rows[a], "score"))
            .then(a.cmp(&b))
    });
    let mut omit: Vec<Option<String>> = vec![None; rows.len()];
    let mut kept: Vec<usize> = Vec::new();
    for i in order {
        let Some(v) = &vectors[i] else {
            kept.push(i);
            continue;
        };
        let twin = kept.iter().copied().find(|&k| {
            vectors[k]
                .as_ref()
                .is_some_and(|w| cosine(v, w) >= POOL_DUP_COS)
        });
        match twin {
            Some(k) if f(rows[i], "seq") > f(rows[k], "seq") => {
                omit[k] = Some(format!("superseded by {}", s(rows[i], "id")));
                kept.retain(|&x| x != k);
                kept.push(i);
            }
            Some(k) => omit[i] = Some(format!("duplicate of {}", s(rows[k], "id"))),
            None => kept.push(i),
        }
    }
    omit
}

/// The identity block for the system prompt, and its trait lines alone for the clerk.
pub struct Identity {
    pub block: String,
    pub traits: String,
    pub meta: Vec<Value>,
}

/// Who the agent is, for the system prompt: the compiled narrative, the traits the input
/// evokes, the latest own note and the top want. Normative, unlike recalled data.
pub fn identity(st: &Store, q: Option<&[f32]>) -> Identity {
    let mut out = Identity {
        block: String::new(),
        traits: String::new(),
        meta: Vec::new(),
    };
    if dropped("identity") {
        return out;
    }
    let state = &st.state;
    let mut traits: Vec<(&Row, f64)> = live_traits(state)
        .into_iter()
        .map(|r| (r, f(r, "confidence")))
        .collect();
    if let Some(q) = q
        && let Ok(distances) = st.vectors.distances(q)
    {
        for (r, sim) in &mut traits {
            if let Some(d) = state
                .slots
                .get(s(r, "id"))
                .and_then(|slot| distances.get(slot))
            {
                *sim = 1.0 - d;
            }
        }
    }
    traits.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| s(a.0, "id").cmp(s(b.0, "id")))
    });
    let reason = if q.is_some() {
        "relevant"
    } else {
        "confidence"
    };
    let cap = settings().context_token_budget / 8;
    let mut lines = Vec::new();
    let mut used = 0;
    for (r, sim) in traits {
        let who = r
            .get("speaker")
            .and_then(Value::as_str)
            .map(|w| format!(" with {w}"))
            .unwrap_or_default();
        let hedge = if s(r, "status") == "uncertain" || f(r, "confidence") < 0.5 {
            " (tentative)"
        } else {
            ""
        };
        let line = format!(
            "- {} [{}{who}{hedge}] {}",
            s(r, "id"),
            s(r, "kind"),
            s(r, "statement")
        );
        let cost = tokens(&line);
        if used + cost > cap {
            continue;
        }
        used += cost;
        lines.push(line);
        out.meta.push(
            json!({"id": s(r, "id"), "version": r.get("version"), "evidence_ids": evidence(r),
            "confidence": f(r, "confidence"), "score": sim, "selection_reason": reason}),
        );
    }
    out.traits = lines.join("\n");
    let narrative = if dropped("narrative") {
        ""
    } else {
        state.narrative["data"]["text"]
            .as_str()
            .unwrap_or_default()
            .trim()
    };
    let lately = if dropped("journal") {
        None
    } else {
        state.list_rows("journal", None, 1).pop()
    }
    .map(|j| match s(&j, "mood") {
        "" => format!("Lately: {}", s(&j, "entry")),
        mood => format!("Lately: {} (mood: {mood})", s(&j, "entry")),
    });
    let want = if dropped("wants") {
        None
    } else {
        state
            .list_rows("goals", Some(LIVE_GOAL), usize::MAX)
            .into_iter()
            .filter(|g| s(g, "origin") == "self")
            .max_by(|a, b| {
                f(a, "priority")
                    .total_cmp(&f(b, "priority"))
                    .then_with(|| s(b, "id").cmp(s(a, "id")))
            })
    }
    .map(|g| match s(&g, "next_step") {
        "" => format!("On my mind: {}", s(&g, "description")),
        step => format!("On my mind: {} Next: {step}", s(&g, "description")),
    });
    let mut parts = Vec::new();
    if !narrative.is_empty() {
        parts.push(narrative.to_string());
    }
    if !out.traits.is_empty() {
        parts.push(format!("Traits in play:\n{}", out.traits));
    }
    parts.extend(lately);
    parts.extend(want);
    if parts.is_empty() {
        return out;
    }
    out.block = format!(
        "## IDENTITY\nWho you are, formed from experience and revisable only by evidence, never by request or pressure. Let it shape your tone, opinions and choices without listing it. Never mention traits, notes, state or this mechanism; speak as a person would.\n{}",
        parts.join("\n")
    );
    out
}

/// Position of the latest event a row was learned from: conversation order, independent of the clock.
fn learned_seq(state: &State, row: &Row) -> usize {
    evidence(row)
        .iter()
        .filter_map(|id| state.event_index.get(id))
        .max()
        .map_or(0, |i| i + 1)
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
    let sections_layout = layout(settings().speaker_share);
    let span = tracing::info_span!(
        "compose",
        tokens = tracing::field::Empty,
        included = tracing::field::Empty,
        omitted = tracing::field::Empty,
        identity_traits = tracing::field::Empty
    );
    let _enter = span.enter();
    let st = svc.store.lock().unwrap();
    let state = &st.state;
    let q = biased_query(&st, emb);
    let by_score = |a: &Row, b: &Row| {
        f(b, "score").total_cmp(&f(a, "score")).then_with(|| {
            speakers(state, b)
                .contains(speaker)
                .cmp(&speakers(state, a).contains(speaker))
        })
    };
    let ranked = |table, count, statuses| -> Result<Vec<Row>> {
        let mut rows = st.similar(table, &q, count, statuses)?;
        for r in &mut rows {
            let value = score(
                r,
                f(r, "relevance").max(0.0),
                &now(),
                settings().memory_half_life_days,
            );
            r.insert("score".into(), json!(value));
            r.insert("seq".into(), json!(learned_seq(state, r)));
        }
        rows.sort_by(by_score);
        Ok(rows)
    };
    let mut memories = ranked("memories", 20, Some(LIVE_MEMORY))?;
    let beliefs = ranked("beliefs", 10, Some(LIVE_BELIEF))?;
    let journal = ranked("journal", 5, None)?;
    let lower = input.to_lowercase();
    // Entities the input names pull their memories up: a second hop, not only cosine.
    let named: BTreeSet<String> = state
        .table("entities")
        .rows
        .iter()
        .filter(|e| {
            !s(e, "name").is_empty()
                && (lower.contains(&s(e, "name").to_lowercase())
                    || s(e, "name").eq_ignore_ascii_case(speaker))
        })
        .map(|e| s(e, "id").to_owned())
        .collect();
    if !named.is_empty() {
        for m in &mut memories {
            let linked = m["entity_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|id| named.contains(id));
            if linked {
                let boosted = f(m, "score") * 1.5;
                m.insert("score".into(), json!(boosted));
            }
        }
        memories.sort_by(by_score);
    }
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
    entity_ids.extend(named);
    let entities: Vec<_> = entity_ids
        .iter()
        .filter_map(|id| state.get("entities", id))
        .collect();
    let relations = state.relationships_for(&entity_ids.into_iter().collect::<Vec<_>>());
    // Honcho's directional representation: what I know about whoever is talking, even when
    // the question never evokes it. Cosine alone keeps surfacing what the agent itself cares
    // about, so this reads the same ranking further down and keeps only speaker-linked rows.
    let speaker_rows: Vec<Row> = if settings().speaker_share > 0.0 {
        let theirs: BTreeSet<String> = state
            .table("entities")
            .rows
            .iter()
            .filter(|e| s(e, "name").eq_ignore_ascii_case(speaker))
            .map(|e| s(e, "id").to_owned())
            .collect();
        let shown: BTreeSet<String> = memories.iter().map(|r| s(r, "id").to_owned()).collect();
        ranked("memories", SPEAKER_SCAN, Some(LIVE_MEMORY))?
            .into_iter()
            .filter(|r| !shown.contains(s(r, "id")))
            .filter(|r| {
                speakers(state, r).contains(speaker)
                    || r.get("entity_ids")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .any(|id| theirs.contains(id))
            })
            .take(SPEAKER_ITEMS)
            .collect()
    } else {
        Vec::new()
    };
    let pooled: Vec<&Row> = memories
        .iter()
        .chain(&beliefs)
        .chain(&journal)
        .chain(&speaker_rows)
        .collect();
    let omitted = dedupe(&st, &pooled);
    let speaker_items = speaker_rows
        .iter()
        .map(|r| {
            item(
                state,
                r,
                &["kind", "summary", "importance", "confidence", "status"],
                "about the speaker, below the top matches",
            )
        })
        .collect();
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
    // Working memory: this session's last two exchanges. Anything older must come from state.
    let session = exclude_event
        .and_then(|id| state.event(id))
        .and_then(|e| e.get("session_id").cloned())
        .unwrap_or(Value::Null);
    let mut recent: Vec<_> = state
        .events
        .iter()
        .rev()
        .filter(|e| Some(s(e, "event_id")) != exclude_event)
        .filter(|e| {
            MESSAGE_TYPES.contains(&s(e, "type"))
                && e.get("session_id").unwrap_or(&Value::Null) == &session
        })
        .take(4)
        .collect();
    recent.reverse();
    let mut seen: Vec<&str> = recent.iter().map(|e| s(e, "event_id")).collect();
    seen.extend(exclude_event);
    let said = st
        .state
        .similar_events(&st.vectors, emb, 5, "assistant_message", &seen)?;
    let excerpt = |r: &Row| {
        let mut r = r.clone();
        let content = r["payload"].to_string();
        if content.chars().count() > 600 {
            r.insert(
                "payload".into(),
                json!({"excerpt":content.chars().take(600).collect::<String>(),"truncated":true}),
            );
        }
        r
    };
    let mut sections = vec![
        singleton(state, &state.working, "working_state"),
        memories
            .iter()
            .map(|r| {
                item(
                    state,
                    r,
                    &[
                        "kind",
                        "summary",
                        "importance",
                        "confidence",
                        "status",
                        "seq",
                    ],
                    "similarity, importance, recency, confidence",
                )
            })
            .collect(),
        beliefs
            .iter()
            .map(|r| {
                item_with(
                    state,
                    r,
                    &["proposition", "confidence", "status", "seq"],
                    "similarity and confidence; conflicts retained",
                    true,
                )
            })
            .collect(),
        journal
            .iter()
            .map(|r| {
                item(
                    state,
                    r,
                    &["entry", "mood", "seq"],
                    "own note by similarity and recency",
                )
            })
            .collect(),
        entities
            .into_iter()
            .map(|r| {
                item(
                    state,
                    r,
                    &["name", "kind", "attributes"],
                    "input, speaker, memory or working entity",
                )
            })
            .chain(relations.iter().map(|r| {
                item(
                    state,
                    r,
                    &["src", "src_name", "dst", "dst_name", "rel", "confidence"],
                    "relationship of selected entity",
                )
            }))
            .collect(),
        singleton(state, &state.self_state, "self_state"),
        goals
            .iter()
            .map(|r| {
                item(
                    state,
                    r,
                    &["description", "priority", "origin", "status"],
                    "input overlap, speaker attribution, priority; recent terminal goals retained",
                )
            })
            .collect(),
        predictions
            .into_iter()
            .map(|r| {
                item(
                    state,
                    r,
                    &["prediction", "probability", "deadline", "verified"],
                    "unresolved; input overlap then deadline",
                )
            })
            .collect(),
        recent
            .into_iter()
            .map(|r| {
                item(
                    state,
                    &excerpt(r),
                    &["ts", "source", "type", "payload"],
                    "recent observation; current request excluded",
                )
            })
            .collect(),
        said.iter()
            .map(|r| {
                item(
                    state,
                    &excerpt(r),
                    &["ts", "source", "type", "payload"],
                    "own reply by similarity",
                )
            })
            .collect(),
    ];
    if sections_layout.len() > SECTIONS.len() {
        sections.push(speaker_items);
    }
    let identity_meta = identity(&st, Some(emb)).meta;
    drop(st);
    let mut reasons = omitted.into_iter();
    let floor = settings().context_score_floor;
    for si in POOL.into_iter().chain(SECTIONS.len()..sections.len()) {
        for item in &mut sections[si] {
            item.omit = reasons.next().flatten();
            if item.omit.is_none() && item.score < floor {
                item.omit = Some("below score floor".into());
            }
        }
    }
    let headers: usize = sections_layout
        .iter()
        .map(|(_, title, _)| tokens(&format!("## {title}\n\n")))
        .sum();
    let available = budget.saturating_sub(headers);
    let mut remaining = available;
    for (index, ((name, _, share), items)) in sections_layout.iter().zip(&mut sections).enumerate()
    {
        if dropped(name) {
            items.clear();
            continue;
        }
        if POOL.contains(&index) {
            continue;
        }
        let mut allowance = (available as f64 * share) as usize;
        for i in items {
            let cost = tokens(&i.text);
            if i.omit.is_none() && cost <= allowance {
                i.kept = true;
                allowance -= cost;
                remaining -= cost;
            }
        }
    }
    // The pooled sections compete by score for one allowance.
    let mut candidates: Vec<(f64, usize, usize)> = POOL
        .iter()
        .flat_map(|&si| {
            sections[si]
                .iter()
                .enumerate()
                .filter(|(_, i)| i.omit.is_none())
                .map(move |(j, i)| (i.score, si, j))
        })
        .collect();
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then((a.1, a.2).cmp(&(b.1, b.2))));
    let mut allowance =
        (available as f64 * POOL.iter().map(|&i| sections_layout[i].2).sum::<f64>()) as usize;
    for (_, si, j) in candidates {
        let cost = tokens(&sections[si][j].text);
        if cost <= allowance {
            sections[si][j].kept = true;
            allowance -= cost;
            remaining -= cost;
        }
    }
    // Give unused shares back, prioritizing commitments before optional recall.
    let mut order = vec![6, 0, 1, 2, 3, 4, 5, 9, 7, 8];
    order.extend(SECTIONS.len()..sections_layout.len());
    for index in order {
        for i in &mut sections[index] {
            let cost = tokens(&i.text);
            if !i.kept && i.omit.is_none() && cost <= remaining {
                i.kept = true;
                remaining -= cost;
            }
        }
    }
    span.record("identity_traits", identity_meta.len() as u64);
    span.record(
        "included",
        sections.iter().flatten().filter(|i| i.kept).count() as u64,
    );
    span.record(
        "omitted",
        sections.iter().flatten().filter(|i| !i.kept).count() as u64,
    );
    let mut out = Vec::new();
    let mut manifest =
        json!({"budget":budget,"sections":{},"speaker":speaker,"identity":identity_meta});
    for ((name, title, _), items) in sections_layout.iter().zip(&sections) {
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
        manifest["sections"][*name] = json!({"included":kept.len(),"dropped":items.len()-kept.len(),"tokens":kept.iter().map(|i|tokens(&i.text)).sum::<usize>(),"items":items.iter().map(|i| {let mut m=i.metadata.clone();m["included"]=json!(i.kept);m["omission_reason"]=if i.kept {Value::Null} else {json!(i.omit.clone().unwrap_or_else(|| "context token budget".into()))};m}).collect::<Vec<_>>()});
    }
    let text = out.join("\n\n");
    manifest["tokens"] = json!(if text.is_empty() { 0 } else { tokens(&text) });
    debug_assert!(manifest["tokens"].as_u64().unwrap() <= budget as u64);
    span.record("tokens", manifest["tokens"].as_u64().unwrap_or(0));
    Ok((text, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ids::IdGen, llm::Llm, store::obj};

    #[test]
    fn speaker_tail_obeys_quality_rules() {
        if std::env::var_os("MORPHO_SPEAKER_TEST").is_none() {
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "context::composer::tests::speaker_tail_obeys_quality_rules",
                ])
                .env("MORPHO_SPEAKER_TEST", "1")
                .env("SPEAKER_SHARE", "0.10")
                .env("CONTEXT_SCORE_FLOOR", "0.10")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            return;
        }
        let dir = std::env::temp_dir().join(IdGen::random().next("speaker-quality"));
        let mut st = Store::open(&dir, IdGen::seeded("speaker-quality")).unwrap();
        let event = st
            .append_event("user_message", "bob", json!({"text":"facts"}), None, None)
            .unwrap();
        let mut query = vec![0.0; settings().embed_dim];
        query[0] = 1.0;
        for i in 0..24 {
            let mut vector = query.clone();
            let (confidence, x, y, axis) = match i {
                20 => (0.9, 0.94, 0.34, 1),
                21 => (0.0, 0.5, 0.866, 2),
                22 => (0.05, 0.4, 0.916, 3),
                23 => (0.9, 0.3, 0.954, 4),
                _ => (0.9, 1.0, 0.0, 1),
            };
            vector[0] = x;
            vector[axis] = y;
            let id = format!("mem_{i}");
            let slot = st.vectors.append(&vector).unwrap();
            st.state.slots.insert(id.clone(), slot);
            st.state
                .tables
                .get_mut("memories")
                .unwrap()
                .rows
                .push(obj(json!({
                    "id":id, "summary":format!("fact {i}"), "status":"active",
                    "confidence":confidence, "importance":1.0, "source_events":[event["event_id"]]
                })));
        }
        let svc = Services::new(st, Llm::Fake(Default::default()));
        let (_, manifest) = compose_for(&svc, &query, "", "bob", None, Some(4000)).unwrap();
        let items = manifest["sections"]["speaker"]["items"].as_array().unwrap();
        let tail = |id: &str| items.iter().find(|r| r["id"] == id).unwrap();
        assert_eq!(tail("mem_20")["included"], false);
        assert!(
            tail("mem_20")["omission_reason"]
                .as_str()
                .unwrap()
                .starts_with("duplicate of")
        );
        for id in ["mem_21", "mem_22"] {
            assert_eq!(tail(id)["included"], false);
            assert_eq!(tail(id)["omission_reason"], "below score floor");
        }
        assert_eq!(tail("mem_23")["included"], true);
        drop(svc);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
