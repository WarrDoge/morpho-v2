//! Background cycle: event consumers + periodic consolidation, reflection, snapshot.

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::Services;
use crate::agents::{beliefs, consolidation, goals, memory, reflection, self_model};
use crate::config::settings;
use crate::llm::is_miss;
use crate::pyfmt::{dt_of, now};
use crate::snapshots::take_snapshot;
use crate::state::engine;
use crate::state::models::Proposal;

pub const CONSUMERS: [&str; 4] = ["memory", "beliefs", "self_model", "goals"];
pub const REFLECT: &str = "reflection";

pub async fn commit_all(
    svc: &Services,
    proposals: Vec<Proposal>,
    fallback: Option<&str>,
) -> Result<Value> {
    let n = settings().max_proposals_per_cycle.min(proposals.len());
    let proposals = &proposals[..n];
    let results = engine::commit(&svc.store, &svc.llm, proposals, fallback).await?;
    for (p, r) in proposals.iter().zip(&results) {
        if !r.accepted {
            tracing::info!(
                "rejected {} {}: {}",
                p.agent,
                p.operation,
                r.reason.as_deref().unwrap_or("")
            );
        }
    }
    let accepted = results.iter().filter(|r| r.accepted).count();
    Ok(json!({"accepted": accepted, "rejected": results.len() - accepted}))
}

pub async fn cycle(svc: &Services, force_reflect: bool) -> Result<Value> {
    let Ok(_guard) = svc.cycle_lock.try_lock() else {
        return Ok(json!({}));
    };
    let mut stats = Map::new();
    for name in CONSUMERS {
        let events = {
            let st = svc.store.lock().unwrap();
            st.state.events_after(st.state.cursor(name), 20)
        };
        if events.is_empty() {
            continue;
        }
        let run = match name {
            "memory" => memory::run(svc, &events).await,
            "beliefs" => beliefs::run(svc, &events).await,
            "self_model" => self_model::run(svc, &events).await,
            _ => goals::run(svc, &events).await,
        };
        let proposals = match run {
            Ok(p) => p,
            Err(e) if is_miss(&e) => return Err(e),
            Err(e) => {
                let failures = svc.store.lock().unwrap().bump_failures(name)?;
                if failures < settings().consumer_max_failures {
                    tracing::error!("consumer {name} failed ({failures}); will retry: {e:#}");
                    continue;
                }
                tracing::error!("consumer {name} failed {failures} times; skipping batch: {e:#}");
                Vec::new()
            }
        };
        let last = events.last().unwrap();
        let fallback = last["event_id"].as_str().map(String::from);
        stats.insert(
            name.into(),
            commit_all(svc, proposals, fallback.as_deref()).await?,
        );
        svc.store
            .lock()
            .unwrap()
            .set_cursor(name, last["id"].as_i64().unwrap_or(0))?;
    }

    let (last_id, row) = {
        let st = svc.store.lock().unwrap();
        (
            st.state.last_event_id(),
            st.state.cursors.get(REFLECT).cloned(),
        )
    };
    let new = last_id
        - row
            .as_ref()
            .and_then(|r| r["last_event_id"].as_i64())
            .unwrap_or(0);
    let idle = row
        .as_ref()
        .and_then(|r| dt_of(r.get("updated_at")))
        .map_or(1e9, |t| {
            (now() - t).num_microseconds().unwrap_or(0) as f64 / 1e6
        });
    let s = settings();
    if force_reflect
        || new >= s.reflect_every_n_events
        || (new > 0 && idle >= s.idle_reflect_seconds)
    {
        for name in ["consolidation", REFLECT] {
            let run = if name == REFLECT {
                reflection::run(svc).await
            } else {
                consolidation::run(svc).await
            };
            let result = match run {
                Ok(p) => commit_all(svc, p, None).await,
                Err(e) => Err(e),
            };
            match result {
                Ok(v) => {
                    stats.insert(name.into(), v);
                }
                Err(e) if is_miss(&e) => return Err(e),
                Err(e) => tracing::error!("{name} failed: {e:#}"),
            }
        }
        let snap = take_snapshot(svc)?;
        stats.insert("snapshot".into(), snap["id"].clone());
        svc.store.lock().unwrap().set_cursor(REFLECT, last_id)?;
    }
    Ok(Value::Object(stats))
}

pub async fn run_forever(svc: &Services) {
    loop {
        match cycle(svc, false).await {
            Ok(stats) if !stats.as_object().is_some_and(Map::is_empty) => {
                tracing::info!("cycle {stats}")
            }
            Ok(_) => {}
            Err(e) => tracing::error!("worker cycle failed: {e:#}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs_f64(
            settings().worker_poll_seconds,
        ))
        .await;
    }
}
