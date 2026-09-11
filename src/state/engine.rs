//! Sole writer of derived state: proposal → validation → dedupe fold → transitions → journal.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use serde_json::{Value, json};

use crate::config::settings;
use crate::llm::Llm;
use crate::pyfmt::{Row, iso, now};
use crate::state::models::{self as m, Payload, Proposal, strings, union};
use crate::store::{Commit, Record, Shared, Store, obj};

#[derive(Clone, Debug)]
pub struct CommitResult {
    pub proposal_id: String,
    pub accepted: bool,
    pub reason: Option<String>,
    pub object_ids: Vec<String>,
}

struct Transition {
    table: &'static str,
    object_id: String,
    before: Option<Row>,
    after: Row,
    embed: bool,
}

fn embed_field(op: &str) -> Option<&'static str> {
    match op {
        "create_memory" | "update_memory" | "merge_memories" => Some("summary"),
        "create_belief" => Some("proposition"),
        _ => None,
    }
}

pub fn embed_text(p: &Proposal) -> Option<String> {
    let f = embed_field(&p.operation)?;
    p.payload
        .get(f)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn events_of(p: &Proposal) -> Vec<String> {
    p.evidence
        .iter()
        .filter(|e| e.starts_with("evt_"))
        .cloned()
        .collect()
}

fn s<'a>(r: &'a Row, k: &str) -> &'a str {
    r.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn list(r: &Row, k: &str) -> Vec<String> {
    r.get(k).map(strings).unwrap_or_default()
}

fn int(r: &Row, k: &str) -> i64 {
    r.get(k).and_then(Value::as_i64).unwrap_or_default()
}

fn updated(before: &Row, fields: Row) -> Row {
    let mut after = before.clone();
    after.extend(fields);
    after
}

fn bump(before: &Row, p: &Proposal, ts: &str) -> Row {
    obj(json!({
        "updated_at": ts,
        "version": int(before, "version") + 1,
        "evidence": union(&list(before, "evidence"), &p.evidence),
    }))
}

fn check_version(row: &Row, expected: Option<i64>) -> Result<(), String> {
    if let Some(e) = expected {
        let have = int(row, "version");
        if have != e {
            return Err(format!("version conflict: expected {e}, have {have}"));
        }
    }
    Ok(())
}

fn load(
    st: &Store,
    table: &str,
    target: Option<&str>,
    expected: Option<i64>,
) -> Result<Row, String> {
    let row = st
        .state
        .get(table, target.unwrap_or(""))
        .ok_or_else(|| format!("{table} {} not found", target.unwrap_or("None")))?;
    check_version(row, expected)?;
    Ok(row.clone())
}

pub async fn commit(
    store: &Shared,
    llm: &Llm,
    proposals: &[Proposal],
    source_event: Option<&str>,
) -> Result<Vec<CommitResult>> {
    let texts: Vec<(usize, String)> = proposals
        .iter()
        .enumerate()
        .filter_map(|(i, p)| embed_text(p).map(|t| (i, t)))
        .collect();
    let mut embs: HashMap<usize, Vec<f32>> = HashMap::new();
    if !texts.is_empty() {
        let vecs = llm
            .embed(&texts.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>())
            .await?;
        embs = texts.iter().map(|(i, _)| *i).zip(vecs).collect();
    }
    let mut out = Vec::with_capacity(proposals.len());
    for (i, p) in proposals.iter().enumerate() {
        out.push(commit_one(store, llm, p, source_event, embs.remove(&i)).await?);
    }
    Ok(out)
}

pub async fn commit_one(
    store: &Shared,
    llm: &Llm,
    p: &Proposal,
    source_event: Option<&str>,
    emb: Option<Vec<f32>>,
) -> Result<CommitResult> {
    let pid = store.lock().unwrap().ids.next("prop");
    let source_event = events_of(p)
        .into_iter()
        .next()
        .or_else(|| source_event.map(String::from));
    let validated = validate(p);
    let emb = match (&validated, emb, embed_text(p)) {
        (Ok(_), None, Some(t)) => Some(llm.embed(&[t]).await?.remove(0)),
        (_, e, _) => e,
    };
    let mut st = store.lock().unwrap();
    commit_locked(&mut st, pid, p, source_event, validated, emb)
}

fn commit_locked(
    st: &mut Store,
    pid: String,
    p: &Proposal,
    source_event: Option<String>,
    validated: Result<Payload, String>,
    emb: Option<Vec<f32>>,
) -> Result<CommitResult> {
    let created = iso(&now());
    let outcome = validated.and_then(|payload| apply(st, p, payload, emb.as_deref()));
    let (decision, reason, transitions, emb) = match outcome {
        Ok((transitions, reason, emb)) => ("accepted", reason, transitions, emb),
        Err(reason) => (
            "rejected",
            Some(reason.chars().take(500).collect()),
            Vec::new(),
            None,
        ),
    };
    let mut vectors = BTreeMap::new();
    if let Some(emb) = emb {
        for t in transitions.iter().filter(|t| t.embed) {
            vectors.insert(t.object_id.clone(), st.vectors.append(emb)?);
        }
    }
    let proposal = obj(json!({
        "id": pid, "agent": p.agent, "operation": p.operation, "target": p.target,
        "payload": p.payload, "evidence": p.evidence, "confidence": p.confidence,
        "decision": decision, "reason": reason, "source_event": source_event, "created_at": created,
    }));
    let base = st.state.transitions.len() as i64;
    let rows: Vec<Row> = transitions
        .iter()
        .enumerate()
        .map(|(i, t)| {
            obj(json!({
                "id": base + i as i64 + 1, "proposal_id": pid, "table_name": t.table,
                "object_id": t.object_id, "before": t.before, "after": t.after, "agent": p.agent,
                "event_id": source_event, "ts": created,
            }))
        })
        .collect();
    let object_ids = transitions.iter().map(|t| t.object_id.clone()).collect();
    st.append(Record::Commit(Commit {
        proposal,
        transitions: rows,
        vectors,
    }))?;
    Ok(CommitResult {
        proposal_id: pid,
        accepted: decision == "accepted",
        reason,
        object_ids,
    })
}

fn validate(p: &Proposal) -> Result<Payload, String> {
    let mut payload = m::parse_payload(&p.operation, &p.payload)?;
    let op = p.operation.as_str();
    if (op.starts_with("create_")
        || op.starts_with("merge_")
        || op.starts_with("upsert_")
        || op.starts_with("add_"))
        && p.evidence.is_empty()
    {
        return Err("evidence required".into());
    }
    if matches!(
        op,
        "update_memory" | "update_belief" | "update_goal" | "verify_prediction"
    ) && p.target.as_deref().unwrap_or("").is_empty()
    {
        return Err("target required".into());
    }
    if let Payload::CreateGoal(g) = &mut payload
        && !matches!(p.agent.as_str(), "interaction" | "harness")
    {
        g.origin = "inferred".into();
    }
    if let Payload::UpdateSelfState(sp) = &payload
        && p.agent != "harness"
        && (sp.patch.contains_key("capabilities") || sp.patch.contains_key("permissions"))
    {
        return Err("self-model may not grant capabilities or permissions".into());
    }
    Ok(payload)
}

type Applied<'a> = (Vec<Transition>, Option<String>, Option<&'a [f32]>);

fn apply<'a>(
    st: &mut Store,
    p: &Proposal,
    payload: Payload,
    emb: Option<&'a [f32]>,
) -> Result<Applied<'a>, String> {
    let (q, payload, reason) = fold_duplicate(st, p, payload, emb)?;
    let emb = if reason.is_some() { None } else { emb };
    let ts = iso(&now());
    let transitions = apply_op(st, &q, &payload, &ts)?;
    Ok((transitions, reason, emb))
}

/// Near-duplicate `create_*` becomes an update of the existing row (or a rejection).
fn fold_duplicate<'a>(
    st: &Store,
    p: &'a Proposal,
    payload: Payload,
    emb: Option<&[f32]>,
) -> Result<(Cow<'a, Proposal>, Payload, Option<String>), String> {
    if let Some(emb) = emb.filter(|e| !e.is_empty()) {
        let fold = match &payload {
            Payload::CreateMemory(_) => Some(("memories", m::LIVE_MEMORY)),
            Payload::CreateBelief(_) => Some(("beliefs", m::LIVE_BELIEF)),
            _ => None,
        };
        if let Some((table, live)) = fold {
            let hits = st.similar(table, emb, 1, Some(live));
            if let Some(hit) = hits.first()
                && hit["relevance"].as_f64().unwrap_or(0.0) >= settings().dedupe_threshold
            {
                let hid = s(hit, "id").to_string();
                let (op, fields) = match &payload {
                    Payload::CreateMemory(cm) => (
                        "update_memory",
                        json!({
                            "reinforce": true,
                            "add_source_events": if cm.source_events.is_empty() { events_of(p) } else { cm.source_events.clone() },
                            "add_entity_ids": cm.entity_ids,
                        }),
                    ),
                    Payload::CreateBelief(cb) => (
                        "update_belief",
                        json!({
                            "confidence": hit["confidence"].as_f64().unwrap_or(0.0).max(cb.confidence),
                            "add_supporting": events_of(p),
                        }),
                    ),
                    _ => unreachable!(),
                };
                let mut q = p.clone();
                q.operation = op.into();
                q.target = Some(hid.clone());
                q.payload = obj(fields);
                let qp = m::parse_payload(op, &q.payload)?;
                return Ok((Cow::Owned(q), qp, Some(format!("folded into {hid}"))));
            }
        }
    }
    if let Payload::CreateGoal(cg) = &payload {
        let lower = cg.description.to_lowercase();
        let dup = st.state.table("goals").rows.iter().find(|g| {
            s(g, "description").to_lowercase() == lower && m::LIVE_GOAL.contains(&s(g, "status"))
        });
        if let Some(g) = dup {
            return Err(format!("duplicate of {}", s(g, "id")));
        }
    }
    Ok((Cow::Borrowed(p), payload, None))
}

fn t(
    table: &'static str,
    object_id: &str,
    before: Option<Row>,
    after: Row,
    embed: bool,
) -> Transition {
    Transition {
        table,
        object_id: object_id.into(),
        before,
        after,
        embed,
    }
}

fn apply_op(
    st: &mut Store,
    p: &Proposal,
    payload: &Payload,
    ts: &str,
) -> Result<Vec<Transition>, String> {
    let target = p.target.as_deref();
    Ok(match payload {
        Payload::CreateMemory(pl) => {
            let id = st.ids.next("mem");
            let after = obj(json!({
                "id": id, "kind": pl.kind, "summary": pl.summary, "status": pl.status,
                "importance": pl.importance, "confidence": pl.confidence,
                "source_events": if pl.source_events.is_empty() { events_of(p) } else { pl.source_events.clone() },
                "evidence": p.evidence, "entity_ids": pl.entity_ids, "access_count": 0,
                "created_at": ts, "updated_at": ts, "last_reinforced_at": null,
                "valid_from": ts, "valid_until": null, "version": 1,
            }));
            vec![t("memories", &id, None, after, true)]
        }
        Payload::UpdateMemory(pu) => {
            let before = load(st, "memories", target, pu.expected_version)?;
            let mut fields = Row::new();
            if let Some(v) = &pu.summary {
                fields.insert("summary".into(), json!(v));
            }
            if let Some(v) = pu.importance {
                fields.insert("importance".into(), json!(v));
            }
            if let Some(v) = pu.confidence {
                fields.insert("confidence".into(), json!(v));
            }
            if let Some(v) = &pu.status {
                fields.insert("status".into(), json!(v));
            }
            if pu.reinforce {
                fields.insert(
                    "access_count".into(),
                    json!(int(&before, "access_count") + 1),
                );
                fields.insert("last_reinforced_at".into(), json!(ts));
                fields.entry("status").or_insert(json!("reinforced"));
            }
            if !pu.add_source_events.is_empty() {
                let u = union(&list(&before, "source_events"), &pu.add_source_events);
                fields.insert("source_events".into(), json!(u));
            }
            if !pu.add_entity_ids.is_empty() {
                let u = union(&list(&before, "entity_ids"), &pu.add_entity_ids);
                fields.insert("entity_ids".into(), json!(u));
            }
            if fields
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|x| m::TERMINAL_MEMORY.contains(&x))
            {
                fields.insert("valid_until".into(), json!(ts));
            }
            let embed = fields.contains_key("summary");
            fields.extend(bump(&before, p, ts));
            let after = updated(&before, fields);
            let id = s(&before, "id").to_string();
            vec![t("memories", &id, Some(before), after, embed)]
        }
        Payload::MergeMemories(pm) => {
            let mut sources = Vec::new();
            for sid in &pm.source_ids {
                sources.push(load(st, "memories", Some(sid), None)?);
            }
            let id = st.ids.next("mem");
            let flat = |k: &str| sources.iter().flat_map(|s| list(s, k)).collect::<Vec<_>>();
            let merged = obj(json!({
                "id": id, "kind": pm.kind, "summary": pm.summary, "status": "consolidated",
                "importance": pm.importance, "confidence": pm.confidence,
                "source_events": union(&[], &flat("source_events")),
                "evidence": union(&p.evidence, &pm.source_ids),
                "entity_ids": union(&[], &flat("entity_ids")),
                "access_count": sources.iter().map(|s| int(s, "access_count")).sum::<i64>(),
                "created_at": ts, "updated_at": ts, "last_reinforced_at": null,
                "valid_from": ts, "valid_until": null, "version": 1,
            }));
            let mut out = vec![t("memories", &id, None, merged, true)];
            for src in sources {
                let fields = obj(json!({
                    "status": "deprecated", "valid_until": ts, "updated_at": ts,
                    "version": int(&src, "version") + 1,
                    "evidence": union(&list(&src, "evidence"), std::slice::from_ref(&id)),
                }));
                let after = updated(&src, fields);
                let sid = s(&src, "id").to_string();
                out.push(t("memories", &sid, Some(src), after, false));
            }
            out
        }
        Payload::CreateBelief(pb) => {
            let id = st.ids.next("belief");
            let after = obj(json!({
                "id": id, "proposition": pb.proposition, "confidence": pb.confidence,
                "status": pb.status, "supporting_evidence": p.evidence, "contradicting_evidence": [],
                "created_at": ts, "updated_at": ts, "last_reviewed": ts, "valid_from": ts,
                "valid_until": null, "version": 1,
            }));
            vec![t("beliefs", &id, None, after, true)]
        }
        Payload::UpdateBelief(ub) => {
            let before = load(st, "beliefs", target, ub.expected_version)?;
            let mut fields = obj(json!({
                "last_reviewed": ts, "updated_at": ts, "version": int(&before, "version") + 1,
            }));
            if let Some(c) = ub.confidence {
                fields.insert("confidence".into(), json!(c));
            }
            if let Some(status) = &ub.status {
                fields.insert("status".into(), json!(status));
                if m::TERMINAL_BELIEF.contains(&status.as_str()) {
                    fields.insert("valid_until".into(), json!(ts));
                }
            }
            if !ub.add_supporting.is_empty() {
                let u = union(&list(&before, "supporting_evidence"), &ub.add_supporting);
                fields.insert("supporting_evidence".into(), json!(u));
            }
            if !ub.add_contradicting.is_empty() {
                let u = union(
                    &list(&before, "contradicting_evidence"),
                    &ub.add_contradicting,
                );
                fields.insert("contradicting_evidence".into(), json!(u));
            }
            let after = updated(&before, fields);
            let id = s(&before, "id").to_string();
            vec![t("beliefs", &id, Some(before), after, false)]
        }
        Payload::CreateGoal(pg) => {
            let id = st.ids.next("goal");
            let after = obj(json!({
                "id": id, "evidence": p.evidence, "description": pg.description,
                "priority": pg.priority, "origin": pg.origin, "status": pg.status,
                "parent_goal": pg.parent_goal, "deadline": pg.deadline,
                "created_at": ts, "updated_at": ts, "version": 1,
            }));
            vec![t("goals", &id, None, after, false)]
        }
        Payload::UpdateGoal(ug) => {
            let before = load(st, "goals", target, ug.expected_version)?;
            let mut fields = bump(&before, p, ts);
            if let Some(status) = &ug.status {
                fields.insert("status".into(), json!(status));
            }
            if let Some(pr) = ug.priority {
                fields.insert("priority".into(), json!(pr));
            }
            let after = updated(&before, fields);
            let id = s(&before, "id").to_string();
            vec![t("goals", &id, Some(before), after, false)]
        }
        Payload::CreatePrediction(pp) => {
            let id = st.ids.next("pred");
            let after = obj(json!({
                "id": id, "evidence": p.evidence, "prediction": pp.prediction,
                "probability": pp.probability, "deadline": pp.deadline, "verified": null,
                "verified_at": null, "created_at": ts, "version": 1,
            }));
            vec![t("predictions", &id, None, after, false)]
        }
        Payload::VerifyPrediction(vp) => {
            let before = load(st, "predictions", target, None)?;
            let fields = obj(json!({
                "verified": vp.verified, "verified_at": ts, "version": int(&before, "version") + 1,
                "evidence": union(&list(&before, "evidence"), &p.evidence),
            }));
            let after = updated(&before, fields);
            let id = s(&before, "id").to_string();
            vec![t("predictions", &id, Some(before), after, false)]
        }
        Payload::UpsertEntity(ue) => {
            let before = st.state.find_entity(&ue.name, Some(&ue.kind));
            let (before, after) = match before {
                None => {
                    let id = st.ids.next("ent");
                    let after = obj(json!({
                        "id": id, "name": ue.name, "kind": ue.kind, "attributes": ue.attributes,
                        "evidence": p.evidence, "created_at": ts, "updated_at": ts, "version": 1,
                    }));
                    (None, after)
                }
                Some(before) => {
                    let mut attrs = before
                        .get("attributes")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    attrs.extend(ue.attributes.clone());
                    let mut fields = obj(json!({"attributes": attrs}));
                    fields.extend(bump(&before, p, ts));
                    let after = updated(&before, fields);
                    (Some(before), after)
                }
            };
            let id = s(&after, "id").to_string();
            vec![t("entities", &id, before, after, false)]
        }
        Payload::AddRelationship(ar) => {
            let src = st.state.find_entity(&ar.src, None);
            let dst = st.state.find_entity(&ar.dst, None);
            let (Some(src), Some(dst)) = (src, dst) else {
                let missing = if st.state.find_entity(&ar.src, None).is_none() {
                    &ar.src
                } else {
                    &ar.dst
                };
                return Err(format!("unknown entity: {missing}"));
            };
            let id = st.ids.next("rel");
            let after = obj(json!({
                "id": id, "src": s(&src, "id"), "rel": ar.rel, "dst": s(&dst, "id"),
                "confidence": ar.confidence, "evidence": p.evidence, "valid_from": ts,
                "valid_until": null, "created_at": ts,
            }));
            vec![t("entity_relationships", &id, None, after, false)]
        }
        Payload::SetWorkingState(sp) | Payload::UpdateSelfState(sp) => {
            let table: &'static str = if matches!(payload, Payload::SetWorkingState(_)) {
                "working_state"
            } else {
                "self_state"
            };
            let before = st.state.singleton(table).clone();
            check_version(&before, sp.expected_version)?;
            let mut data = before
                .get("data")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            data.extend(sp.patch.clone());
            let fields = obj(json!({
                "data": data, "updated_at": ts, "version": int(&before, "version") + 1,
            }));
            let after = updated(&before, fields);
            vec![t(table, table, Some(before), after, false)]
        }
    })
}
