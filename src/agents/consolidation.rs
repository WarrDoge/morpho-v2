//! Merges, generalizes, detects contradictions, decays (§9.3, §14). Runs periodically.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::base::{clamp, fmt_rows};
use crate::Services;
use crate::config::settings;
use crate::context::ranking::decayed_importance;
use crate::llm::CONSOLIDATION;
use crate::pyfmt::{Row, now};
use crate::state::models::{LIVE_MEMORY, Proposal, strings, union};

pub const SYSTEM: &str = "You are the consolidation agent of a persistent-state assistant.
Given memories (episodic and semantic): (1) merge groups that describe the same fact or episode
into one memory, (2) generalize repeated observations into semantic statements with provenance,
(3) flag pairs that contradict each other. Only merge true redundancy; keep distinct episodes
distinct. Use only the given memory ids. Merged and generalized summaries describe the user or
the world in plain language; never describe memories, ids, or this process itself.
Return only the JSON object.";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Merge {
    pub source_ids: Vec<String>,
    pub summary: String,
    pub importance: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generalization {
    pub statement: String,
    pub source_ids: Vec<String>,
    pub confidence: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contradiction {
    pub memory_ids: Vec<String>,
    pub description: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consolidation {
    pub merges: Vec<Merge>,
    pub generalizations: Vec<Generalization>,
    pub contradictions: Vec<Contradiction>,
}

fn id(r: &Row) -> &str {
    r["id"].as_str().unwrap_or_default()
}

/// Deterministic, code-only decay: archive unreinforced memories below the importance floor.
pub fn decay_proposals(memories: &[Row], now: &DateTime<Utc>) -> Vec<Proposal> {
    let s = settings();
    memories
        .iter()
        .filter(|m| m["kind"] == json!("episodic"))
        .filter(|m| m["access_count"].as_i64() == Some(0))
        .filter(|m| decayed_importance(m, now, s.memory_half_life_days) < s.memory_archive_floor)
        .map(|m| {
            Proposal::new(
                "consolidation",
                "update_memory",
                json!({"status": "archived", "expected_version": m["version"]}),
            )
            .target(id(m))
            .evidence(vec!["decay".into()])
            .confidence(1.0)
        })
        .collect()
}

fn dedupe(ids: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for i in ids {
        if !out.contains(&i.as_str()) {
            out.push(i);
        }
    }
    out
}

#[tracing::instrument(name = "consolidation", skip_all)]
pub async fn run(svc: &Services) -> Result<Vec<Proposal>> {
    let now = now();
    let live = svc
        .store
        .lock()
        .unwrap()
        .state
        .list_rows("memories", Some(LIVE_MEMORY), 200);
    let mut proposals = decay_proposals(&live, &now);
    let archived: HashSet<String> = proposals.iter().filter_map(|p| p.target.clone()).collect();
    let candidates: Vec<&Row> = live
        .iter()
        .filter(|m| !archived.contains(id(m)))
        .take(30)
        .collect();
    if candidates.len() < 2 {
        return Ok(proposals);
    }
    let by_id: HashMap<&str, &Row> = candidates.iter().map(|m| (id(m), *m)).collect();
    let rows: Vec<Row> = candidates.iter().map(|m| (*m).clone()).collect();
    let user = format!(
        "MEMORIES:\n{}",
        fmt_rows(&rows, &["kind", "summary", "importance", "created_at"])
    );
    let out: Consolidation = svc.llm.complete_json(SYSTEM, &user, &CONSOLIDATION).await?;
    let mut used: HashSet<&str> = HashSet::new();
    for mg in &out.merges {
        let ids: Vec<&str> = dedupe(&mg.source_ids)
            .into_iter()
            .filter(|i| by_id.contains_key(i) && !used.contains(i))
            .collect();
        if ids.len() < 2 {
            continue;
        }
        used.extend(ids.iter().copied());
        let payload = json!({
            "source_ids": ids,
            "summary": mg.summary,
            "kind": by_id[ids[0]]["kind"],
            "importance": clamp(mg.importance),
        });
        proposals.push(
            Proposal::new("consolidation", "merge_memories", payload)
                .evidence(ids.iter().map(|s| s.to_string()).collect())
                .confidence(0.8),
        );
    }
    for g in &out.generalizations {
        let ids: Vec<&str> = dedupe(&g.source_ids)
            .into_iter()
            .filter(|i| by_id.contains_key(i))
            .collect();
        if ids.is_empty() {
            continue;
        }
        let flat = |k: &str| {
            ids.iter()
                .flat_map(|i| by_id[i].get(k).map(strings).unwrap_or_default())
                .collect::<Vec<_>>()
        };
        let importance = ids
            .iter()
            .map(|i| by_id[i]["importance"].as_f64().unwrap_or_default())
            .fold(f64::NEG_INFINITY, f64::max);
        let payload = json!({
            "kind": "semantic",
            "summary": g.statement,
            "importance": importance,
            "confidence": clamp(g.confidence),
            "status": "active",
            "source_events": union(&[], &flat("source_events")),
            "entity_ids": union(&[], &flat("entity_ids")),
        });
        proposals.push(
            Proposal::new("consolidation", "create_memory", payload)
                .evidence(ids.iter().map(|s| s.to_string()).collect())
                .confidence(clamp(g.confidence)),
        );
    }
    for c in &out.contradictions {
        let ids: Vec<&str> = dedupe(&c.memory_ids)
            .into_iter()
            .filter(|i| by_id.contains_key(i))
            .collect();
        if ids.len() < 2 {
            continue;
        }
        let payload =
            json!({"proposition": c.description, "confidence": 0.5, "status": "uncertain"});
        proposals.push(
            Proposal::new("consolidation", "create_belief", payload)
                .evidence(ids.iter().map(|s| s.to_string()).collect())
                .confidence(0.5),
        );
    }
    let _: Option<&Value> = None;
    Ok(proposals)
}
