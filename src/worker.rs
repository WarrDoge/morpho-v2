//! Bounded idle revision through the same coordinator that consumes the durable inbox.
use crate::{
    Services,
    agents::{consolidation, reflection},
    config::settings,
    interact,
    pyfmt::{dt_of, iso, now},
    snapshots::take_snapshot,
    state::{engine, models::Proposal},
    store::{Record, obj},
};
use anyhow::Result;
use serde_json::{Value, json};

pub const REFLECT: &str = "reflection";

pub async fn commit_all(
    svc: &Services,
    proposals: Vec<Proposal>,
    fallback: Option<&str>,
) -> Result<Value> {
    let mut accepted = 0;
    let mut rejected = 0;
    // All proposals survive; the configured cap bounds each embedding batch, not retention.
    for chunk in proposals.chunks(settings().max_proposals_per_cycle.max(1)) {
        let results = engine::commit(&svc.store, &svc.llm, chunk, fallback).await?;
        accepted += results.iter().filter(|r| r.accepted).count();
        rejected += results.iter().filter(|r| !r.accepted).count();
    }
    Ok(json!({"accepted":accepted,"rejected":rejected}))
}

pub async fn cycle(svc: &Services, force: bool) -> Result<Value> {
    let Ok(_guard) = svc.cycle_lock.try_lock() else {
        return Ok(json!({}));
    };
    if interact::has_ready_input(svc)? {
        return Ok(json!({}));
    }
    let (last, new, idle, dirty, due_changed, due_ids, failures, new_since_failure) = {
        let st = svc.store.lock().unwrap();
        let row = st.state.cursors.get(REFLECT);
        let last = st.state.last_event_id();
        let new = last - st.state.cursor(REFLECT);
        let idle = row
            .and_then(|r| dt_of(r.get("updated_at")))
            .map_or(f64::INFINITY, |t| (now() - t).num_seconds() as f64);
        let dirty = row
            .and_then(|r| r.get("transition"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            < st.state.transitions.len() as u64;
        let due_ids: Vec<_> = st
            .state
            .table("predictions")
            .rows
            .iter()
            .filter(|p| {
                p["verified"].is_null() && dt_of(p.get("deadline")).is_some_and(|t| t <= now())
            })
            .map(|p| p["id"].clone())
            .collect();
        let due_changed =
            !due_ids.is_empty() && row.and_then(|r| r.get("due")) != Some(&json!(due_ids));
        let failures = row.and_then(|r| r["failures"].as_i64()).unwrap_or(0);
        let new_since_failure = last
            > row
                .and_then(|r| r.get("failed_last_event"))
                .and_then(Value::as_i64)
                .unwrap_or(0)
            || st.state.transitions.len() as u64
                > row
                    .and_then(|r| r.get("failed_transition"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
        (
            last,
            new,
            idle,
            dirty,
            due_changed,
            due_ids,
            failures,
            new_since_failure,
        )
    };
    let cfg = settings();
    if !force
        && !(new >= cfg.reflect_every_n_events
            || due_changed
            || ((new > 0 || dirty) && idle >= cfg.idle_reflect_seconds))
    {
        return Ok(json!({}));
    }
    if !force
        && failures > 0
        && (idle < cfg.idle_reflect_seconds
            || (failures >= cfg.consumer_max_failures && !new_since_failure && !due_changed))
    {
        return Ok(json!({}));
    }
    let budget_key = format!("maintenance_tokens:{}", now().format("%Y-%m-%d"));
    let reservation = 2 * (cfg.max_prompt_tokens + cfg.max_completion_tokens);
    let spent = {
        let st = svc.store.lock().unwrap();
        let spent = st
            .metadata(&budget_key)?
            .map(|s| s.parse::<u64>())
            .transpose()?
            .unwrap_or(0);
        if spent.saturating_add(reservation) > cfg.background_daily_token_budget {
            return Ok(json!({"maintenance":"daily token budget exhausted"}));
        }
        // Reserve before inference: a process crash must not reset the daily spending bound.
        st.set_metadata(&budget_key, &(spent + reservation).to_string())?;
        spent
    };
    let before = svc.llm.counts();
    let staged = svc.staged();
    let result: Result<Value> = async {
        let merged = consolidation::run(&staged).await?;
        let a = commit_all(&staged, merged, None).await?;
        let revised = reflection::run(&staged).await?;
        let b = commit_all(&staged, revised, None).await?;
        let snapshot = take_snapshot(&staged)?;
        let mut st = staged.store.lock().unwrap();
        let transition = st.state.transitions.len();
        st.append(Record::Cursor(obj(
            json!({"consumer":REFLECT,"last_event_id":last,"failures":0,
            "updated_at":iso(&now()),"transition":transition,"due":due_ids}),
        )))?;
        Ok(json!({"consolidation":a,"reflection":b,"snapshot":snapshot["id"]}))
    }
    .await;
    let after = svc.llm.counts();
    let used = (after.prompt_tokens + after.completion_tokens)
        .saturating_sub(before.prompt_tokens + before.completion_tokens);
    // Unknown usage (e.g. transport failure) retains the reservation conservatively.
    let missing_usage = matches!(svc.llm.as_ref(), crate::llm::Llm::Live(_))
        && after
            .usage_reported_calls
            .saturating_sub(before.usage_reported_calls)
            < after.llm_calls.saturating_sub(before.llm_calls);
    if result.is_ok() && !missing_usage {
        svc.store
            .lock()
            .unwrap()
            .set_metadata(&budget_key, &(spent + used).to_string())?;
    }
    match result {
        Ok(stats) => {
            svc.publish(staged, None)?;
            Ok(stats)
        }
        Err(e) => {
            let mut st = svc.store.lock().unwrap();
            let previous = st.state.cursor(REFLECT);
            let transition = st
                .state
                .cursors
                .get(REFLECT)
                .and_then(|r| r.get("transition"))
                .cloned()
                .unwrap_or(json!(0));
            st.append_event(
                "runtime_failure",
                "harness",
                json!({"operation":"maintenance","error":format!("{e:#}")}),
                None,
                None,
            )?;
            let failed_last_event = st.state.last_event_id();
            let failed_transition = st.state.transitions.len();
            st.append(Record::Cursor(obj(json!({"consumer":REFLECT,"last_event_id":previous,"failures":failures+1,
                "updated_at":iso(&now()),"transition":transition,"due":due_ids,"error":format!("{e:#}"),
                "failed_last_event":failed_last_event,"failed_transition":failed_transition}))))?;
            Err(e)
        }
    }
}

pub async fn run_forever(svc: &Services) {
    loop {
        let processed = {
            let _guard = svc.cycle_lock.lock().await;
            interact::process_next(svc).await
        };
        match processed {
            Ok(true) => continue,
            Err(e) => tracing::error!("input remains queued: {e:#}"),
            _ => {}
        }
        if settings().worker_inprocess
            && let Err(e) = cycle(svc, false).await
        {
            tracing::error!("maintenance remains due: {e:#}");
        }
        tokio::time::sleep(std::time::Duration::from_secs_f64(
            settings().worker_poll_seconds.max(0.01),
        ))
        .await;
    }
}
