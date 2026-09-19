//! Sole writer of derived state: proposal → validation → dedupe fold → transitions → journal.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use serde_json::{Value, json};

use crate::llm::Llm;
use crate::pyfmt::{Row, iso, now, round4};
use crate::state::models::{self as m, Payload, Proposal, strings, union};
use crate::store::{Commit, Record, Shared, Store, obj};

#[derive(Clone, Debug, serde::Serialize)]
pub struct CommitResult {
    pub proposal_id: String,
    pub accepted: bool,
    pub reason: Option<String>,
    pub object_ids: Vec<String>,
}

/// One contrary observation moves a disposition at most this far.
const TRAIT_MAX_STEP: f64 = 0.25;
/// A new trait this close to a live one restates it.
const TRAIT_FOLD_COS: f64 = 0.9;

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
        "create_trait" | "update_trait" => Some("statement"),
        "create_journal" => Some("entry"),
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

#[tracing::instrument(name = "commit", skip_all, fields(proposals = proposals.len(),
    accepted = tracing::field::Empty, rejected = tracing::field::Empty))]
pub async fn commit(
    store: &Shared,
    llm: &Llm,
    proposals: &[Proposal],
    source_event: Option<&str>,
) -> Result<Vec<CommitResult>> {
    let texts: Vec<(usize, String)> = proposals
        .iter()
        .enumerate()
        .filter(|(_, p)| validate(p).is_ok())
        .filter_map(|(i, p)| embed_text(p).map(|t| (i, t)))
        .collect();
    let mut embs: HashMap<usize, Vec<f32>> = HashMap::new();
    if !texts.is_empty() {
        let vecs = llm
            .embed(&texts.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>())
            .await?;
        embs = texts.iter().map(|(i, _)| *i).zip(vecs).collect();
    }
    let mut st = store.lock().unwrap();
    let mut staged = st.fork();
    let mut out = Vec::with_capacity(proposals.len());
    for (i, p) in proposals.iter().enumerate() {
        let pid = staged.ids.next("prop");
        let source = events_of(p)
            .into_iter()
            .next()
            .or_else(|| source_event.map(String::from));
        out.push(commit_locked(
            &mut staged,
            pid,
            p,
            source,
            validate(p),
            embs.remove(&i),
        )?);
    }
    st.publish(staged, None)?;
    report(proposals, &out);
    Ok(out)
}

fn report(proposals: &[Proposal], results: &[CommitResult]) {
    let span = tracing::Span::current();
    span.record(
        "accepted",
        results.iter().filter(|r| r.accepted).count() as u64,
    );
    span.record(
        "rejected",
        results.iter().filter(|r| !r.accepted).count() as u64,
    );
    for (p, r) in proposals.iter().zip(results) {
        if !r.accepted {
            tracing::info!(agent = %p.agent, operation = %p.operation,
                reason = r.reason.as_deref().unwrap_or(""), "rejected");
        }
        crate::telemetry::record_change(&p.agent, &p.operation, r.accepted);
    }
}

pub async fn commit_one(
    store: &Shared,
    llm: &Llm,
    p: &Proposal,
    source_event: Option<&str>,
    emb: Option<Vec<f32>>,
) -> Result<CommitResult> {
    let emb = match (emb, embed_text(p)) {
        (None, Some(text)) => Some(llm.embed(&[text]).await?.remove(0)),
        (emb, _) => emb,
    };
    let mut st = store.lock().unwrap();
    let mut staged = st.fork();
    let pid = staged.ids.next("prop");
    let source = events_of(p)
        .into_iter()
        .next()
        .or_else(|| source_event.map(String::from));
    let result = commit_locked(&mut staged, pid, p, source, validate(p), emb)?;
    st.publish(staged, None)?;
    report(std::slice::from_ref(p), std::slice::from_ref(&result));
    Ok(result)
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
    let outcome = validated.and_then(|payload| {
        if !p.confidence.is_finite() || !(0.0..=1.0).contains(&p.confidence) {
            return Err("invalid proposal confidence".into());
        }
        let cited: Vec<&String> = match &payload {
            Payload::UpdateBelief(b) => b
                .add_supporting
                .iter()
                .chain(&b.add_contradicting)
                .collect(),
            Payload::UpdateTrait(t) => t
                .add_supporting
                .iter()
                .chain(&t.add_contradicting)
                .collect(),
            _ => Vec::new(),
        };
        for id in p.evidence.iter().chain(cited) {
            if st.state.event(id).is_none()
                && !m::table_for(id).is_some_and(|t| st.state.get(t, id).is_some())
                && !(id == "decay" && p.agent == "consolidation")
            {
                return Err(format!("unknown evidence: {id}"));
            }
        }
        apply(st, p, payload, emb.as_deref())
    });
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
        "decision": decision, "reason": reason, "rationale": p.reason, "source_event": source_event, "created_at": created,
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
    if p.agent == "reflection"
        && !matches!(
            op,
            "update_belief"
                | "create_belief"
                | "create_trait"
                | "update_trait"
                | "create_goal"
                | "create_memory"
                | "set_working_state"
                | "update_self_state"
                | "create_prediction"
                | "verify_prediction"
                | "update_goal"
                | "create_journal"
        )
    {
        return Err("operation not allowed during reflection".into());
    }
    if op == "set_narrative" && !matches!(p.agent.as_str(), "narrative" | "harness") {
        return Err("only the narrative agent compiles the narrative".into());
    }
    if p.agent != "harness" && p.evidence.is_empty() {
        return Err("evidence required".into());
    }

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
        "update_memory" | "update_belief" | "update_trait" | "update_goal" | "verify_prediction"
    ) && p.target.as_deref().unwrap_or("").is_empty()
    {
        return Err("target required".into());
    }
    if let Payload::CreateGoal(g) = &mut payload {
        let cites_trait = p.evidence.iter().any(|e| e.starts_with("trait_"));
        if p.agent == "reflection" {
            if !cites_trait {
                return Err("a goal of your own must cite the trait it follows from".into());
            }
            g.origin = "self".into();
        } else if p.agent == "loops" {
            g.origin = "self".into();
        } else if !matches!(p.agent.as_str(), "interaction" | "harness") {
            g.origin = "inferred".into();
        } else if g.origin == "self" && !cites_trait {
            return Err("origin self requires a trait in evidence".into());
        }
    }
    if let Payload::UpdateSelfState(sp) = &payload
        && p.agent != "harness"
        && (sp.patch.contains_key("capabilities") || sp.patch.contains_key("permissions"))
    {
        return Err("self-model may not grant capabilities or permissions".into());
    }
    if p.agent != "harness" {
        if let Payload::UpdateSelfState(patch) = &payload {
            for (key, value) in &patch.patch {
                if ![
                    "limitations",
                    "commitments",
                    "recent_actions",
                    "known_failures",
                    "uncertainties",
                    "current_objectives",
                ]
                .contains(&key.as_str())
                {
                    return Err(format!("unknown self-model field: {key}"));
                }
                if !value
                    .as_array()
                    .is_some_and(|a| a.len() <= 8 && a.iter().all(Value::is_string))
                {
                    return Err(format!("{key}: expected at most eight strings"));
                }
            }
        }
        if let Payload::SetWorkingState(patch) = &payload {
            for (key, value) in &patch.patch {
                let valid = match key.as_str() {
                    "current_topic" | "current_task" | "mood" => {
                        value.is_string() || value.is_null()
                    }
                    "active_entities" | "open_questions" | "current_constraints" => value
                        .as_array()
                        .is_some_and(|a| a.len() <= 10 && a.iter().all(Value::is_string)),
                    _ => false,
                };
                if !valid {
                    return Err(format!("invalid working-state field: {key}"));
                }
            }
        }
        if p.agent == "reflection"
            && serde_json::to_string(&p.payload)
                .unwrap_or_default()
                .chars()
                .count()
                > 1200
        {
            return Err(
                "reflection payload exceeds 1200 characters; use a smaller partial update".into(),
            );
        }
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

/// Text-identical `create_*` becomes an update of the existing row (or a rejection);
/// a trait restated in other words folds by embedding.
fn fold_duplicate<'a>(
    st: &Store,
    p: &'a Proposal,
    payload: Payload,
    emb: Option<&[f32]>,
) -> Result<(Cow<'a, Proposal>, Payload, Option<String>), String> {
    let candidate = match &payload {
        Payload::CreateMemory(cm) => {
            Some(("memories", "summary", cm.summary.as_str(), m::LIVE_MEMORY))
        }
        Payload::CreateBelief(cb) => Some((
            "beliefs",
            "proposition",
            cb.proposition.as_str(),
            m::LIVE_BELIEF,
        )),
        Payload::CreateTrait(ct) => {
            Some(("traits", "statement", ct.statement.as_str(), m::LIVE_TRAIT))
        }
        _ => None,
    };
    let nearest_trait = |emb: &[f32]| {
        let distances = st.vectors.distances(emb).ok()?;
        st.state
            .table("traits")
            .rows
            .iter()
            .filter(|r| m::LIVE_TRAIT.contains(&s(r, "status")))
            .filter_map(|r| {
                let d = distances.get(st.state.slots.get(s(r, "id"))?)?;
                Some((r, 1.0 - d))
            })
            .filter(|(_, cos)| *cos >= TRAIT_FOLD_COS)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(r, _)| r)
    };
    let hit = candidate.and_then(|(table, field, text, live)| {
        st.state
            .table(table)
            .rows
            .iter()
            .find(|r| {
                live.contains(&s(r, "status"))
                    && s(r, field).trim().eq_ignore_ascii_case(text.trim())
            })
            .or_else(|| match (&payload, emb) {
                (Payload::CreateTrait(_), Some(emb)) => nearest_trait(emb),
                _ => None,
            })
    });
    if let Some(hit) = hit {
        let hid = s(hit, "id").to_string();
        let (op, fields) = match &payload {
            Payload::CreateMemory(cm) => (
                "update_memory",
                json!({"reinforce":true,
                "add_source_events":if cm.source_events.is_empty() {events_of(p)} else {cm.source_events.clone()}, "add_entity_ids":cm.entity_ids}),
            ),
            Payload::CreateBelief(cb) => (
                "update_belief",
                json!({"confidence":hit["confidence"].as_f64().unwrap_or(0.0).max(cb.confidence),"add_supporting":events_of(p)}),
            ),
            Payload::CreateTrait(ct) => (
                "update_trait",
                json!({"confidence":hit["confidence"].as_f64().unwrap_or(0.0).max(ct.confidence),"add_supporting":events_of(p)}),
            ),
            _ => unreachable!(),
        };
        let mut q = p.clone();
        q.operation = op.into();
        q.target = Some(hid.clone());
        q.payload = obj(fields);
        let parsed = m::parse_payload(op, &q.payload)?;
        return Ok((Cow::Owned(q), parsed, Some(format!("folded into {hid}"))));
    }
    if let Payload::CreatePrediction(cp) = &payload {
        let normalized = |s: &str| {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let deadline = cp.deadline.as_deref().and_then(crate::pyfmt::parse_dt);
        if let Some(r) = st.state.table("predictions").rows.iter().find(|r| {
            r["verified"].is_null()
                && normalized(s(r, "prediction")) == normalized(&cp.prediction)
                && crate::pyfmt::dt_of(r.get("deadline")) == deadline
        }) {
            return Err(format!("duplicate unresolved prediction {}", s(r, "id")));
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
            for (key, add) in [("successes", pu.add_success), ("failures", pu.add_failure)] {
                if add > 0.0 {
                    let total = before.get(key).and_then(Value::as_f64).unwrap_or(0.0) + add;
                    fields.insert(key.into(), json!(round4(total)));
                }
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
        Payload::CreateTrait(pt) => {
            let id = st.ids.next("trait");
            let after = obj(json!({
                "id": id, "kind": pt.kind, "statement": pt.statement, "confidence": pt.confidence,
                "origin": if p.agent == "harness" { "seed" } else { "experienced" },
                "speaker": pt.speaker, "status": "active", "evidence": p.evidence,
                "supporting_evidence": p.evidence, "contradicting_evidence": [],
                "created_at": ts, "updated_at": ts, "valid_from": ts, "valid_until": null, "version": 1,
            }));
            vec![t("traits", &id, None, after, true)]
        }
        Payload::UpdateTrait(ut) => {
            let before = load(st, "traits", target, ut.expected_version)?;
            let old = before["confidence"].as_f64().unwrap_or(0.0);
            let mut confidence = ut.confidence.unwrap_or(old);
            let observed = p.evidence.iter().any(|id| {
                st.state
                    .event(id)
                    .is_some_and(|e| m::is_outcome(e) && !list(&before, "evidence").contains(id))
            });
            if p.agent != "harness" && confidence < old {
                if !observed {
                    return Err("lowering a trait requires a new contrary observation".into());
                }
                confidence = confidence.max(old - TRAIT_MAX_STEP);
            }
            if p.agent != "harness"
                && ut
                    .statement
                    .as_ref()
                    .is_some_and(|v| v != s(&before, "statement"))
                && !observed
            {
                return Err("revising a trait requires a new contrary observation".into());
            }
            if ut.status.as_deref() == Some("retired") && p.agent != "harness" && confidence > 0.3 {
                return Err("retiring a trait requires confidence at or below 0.3".into());
            }
            let mut fields = bump(&before, p, ts);
            fields.insert("confidence".into(), json!(round4(confidence)));
            if let Some(v) = &ut.statement {
                fields.insert("statement".into(), json!(v));
            }
            if let Some(status) = &ut.status {
                fields.insert("status".into(), json!(status));
                if status == "retired" {
                    fields.insert("valid_until".into(), json!(ts));
                }
            }
            if !ut.add_supporting.is_empty() {
                let u = union(&list(&before, "supporting_evidence"), &ut.add_supporting);
                fields.insert("supporting_evidence".into(), json!(u));
            }
            if !ut.add_contradicting.is_empty() {
                let u = union(
                    &list(&before, "contradicting_evidence"),
                    &ut.add_contradicting,
                );
                fields.insert("contradicting_evidence".into(), json!(u));
            }
            let after = updated(&before, fields);
            let id = s(&before, "id").to_string();
            vec![t(
                "traits",
                &id,
                Some(before),
                after,
                ut.statement.is_some(),
            )]
        }
        Payload::CreateGoal(pg) => {
            let id = st.ids.next("goal");
            let mut after = obj(json!({
                "id": id, "evidence": p.evidence, "description": pg.description,
                "priority": pg.priority, "origin": pg.origin, "status": pg.status,
                "parent_goal": pg.parent_goal, "deadline": pg.deadline,
                "created_at": ts, "updated_at": ts, "version": 1,
            }));
            if let Some(kind) = &pg.kind {
                after.insert("kind".into(), json!(kind));
            }
            vec![t("goals", &id, None, after, false)]
        }
        Payload::UpdateGoal(ug) => {
            let before = load(st, "goals", target, ug.expected_version)?;
            if p.agent == "reflection" && s(&before, "origin") != "self" {
                return Err("reflection may only revise its own goals".into());
            }
            if ug.status.as_deref() == Some("completed")
                && p.agent != "harness"
                && !p.evidence.iter().any(|id| {
                    st.state.event(id).is_some_and(|e| {
                        m::is_outcome(e) && !list(&before, "evidence").contains(id)
                    })
                })
            {
                return Err("goal completion requires a new observed outcome event".into());
            }

            let mut fields = bump(&before, p, ts);
            if let Some(status) = &ug.status {
                fields.insert("status".into(), json!(status));
            }
            if let Some(pr) = ug.priority {
                fields.insert("priority".into(), json!(pr));
            }
            if let Some(step) = &ug.next_step {
                fields.insert("next_step".into(), json!(step));
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
            if !p.evidence.iter().any(|id| {
                st.state
                    .event(id)
                    .is_some_and(|e| m::is_outcome(e) && !list(&before, "evidence").contains(id))
            }) {
                return Err("prediction verification requires a new observed outcome event".into());
            }

            let fields = obj(json!({
                "verified": vp.verified, "verified_at": ts, "version": int(&before, "version") + 1,
                "evidence": union(&list(&before, "evidence"), &p.evidence),
            }));
            let after = updated(&before, fields);
            let id = s(&before, "id").to_string();
            vec![t("predictions", &id, Some(before), after, false)]
        }
        Payload::UpsertEntity(ue) => {
            let before = if let Some(target) = target {
                let row = st
                    .state
                    .find_entity(target, ue.kind.as_deref())
                    .ok_or_else(|| format!("unknown entity: {target}"))?;
                if ue
                    .name
                    .as_deref()
                    .is_some_and(|name| !name.eq_ignore_ascii_case(s(&row, "name")))
                {
                    return Err("entity name conflicts with target".into());
                }
                Some(row)
            } else {
                let name = ue
                    .name
                    .as_deref()
                    .ok_or("name required for entity creation")?;
                st.state.find_entity(name, ue.kind.as_deref())
            };
            let (before, after) = match before {
                None => {
                    let id = st.ids.next("ent");
                    let after = obj(json!({
                        "id": id, "name": ue.name, "kind": ue.kind.as_deref().unwrap_or("thing"), "attributes": ue.attributes,
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
        Payload::CreateJournal(pj) => {
            let id = st.ids.next("journal");
            let after = obj(json!({
                "id": id, "entry": pj.entry, "mood": pj.mood, "evidence": p.evidence,
                "created_at": ts, "version": 1,
            }));
            vec![t("journal", &id, None, after, true)]
        }
        Payload::SetNarrative(sn) => {
            let before = st.state.narrative.clone();
            let fields = obj(json!({
                "data": {"text": sn.text, "sources": sn.sources}, "updated_at": ts,
                "version": int(&before, "version") + 1,
            }));
            let after = updated(&before, fields);
            vec![t("narrative", "narrative", Some(before), after, false)]
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
            if before.get("data") == Some(&json!(data)) {
                return Ok(Vec::new());
            }
            let fields = obj(json!({
                "data": data, "updated_at": ts, "version": int(&before, "version") + 1,
            }));
            let after = updated(&before, fields);
            vec![t(table, table, Some(before), after, false)]
        }
    })
}
