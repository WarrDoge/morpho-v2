//! Bounded idle revision through the same coordinator that consumes the durable inbox.
use crate::{
    Services,
    agents::{consolidation, reflection},
    config::settings,
    interact,
    pyfmt::{Row, dt_of, iso, now},
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

fn due(s: &crate::store::state::State) -> Vec<Value> {
    s.table("predictions")
        .rows
        .iter()
        .filter(|p| p["verified"].is_null() && dt_of(p.get("deadline")).is_some_and(|d| d <= now()))
        .map(|p| p["id"].clone())
        .collect()
}

struct Reservation {
    key: String,
    spent: u64,
    before: crate::llm::Counts,
}
fn reserve(svc: &Services) -> Result<Option<Reservation>> {
    let cfg = settings();
    let key = format!("maintenance_tokens:{}", now().format("%Y-%m-%d"));
    let st = svc.store.lock().unwrap();
    let spent = st
        .metadata(&key)?
        .map(|s| s.parse::<u64>())
        .transpose()?
        .unwrap_or(0);
    let reserved = cfg.max_prompt_tokens + cfg.max_completion_tokens;
    if spent.saturating_add(reserved) > cfg.background_daily_token_budget {
        return Ok(None);
    }
    st.set_metadata(&key, &(spent + reserved).to_string())?;
    Ok(Some(Reservation {
        key,
        spent,
        before: svc.llm.counts(),
    }))
}
fn settle(svc: &Services, r: Reservation) -> Result<()> {
    let after = svc.llm.counts();
    let missing = matches!(svc.llm.as_ref(), crate::llm::Llm::Live(_))
        && after.usage_reported_calls - r.before.usage_reported_calls
            < after.llm_calls - r.before.llm_calls;
    if !missing {
        let used = (after.prompt_tokens + after.completion_tokens)
            .saturating_sub(r.before.prompt_tokens + r.before.completion_tokens);
        svc.store
            .lock()
            .unwrap()
            .set_metadata(&r.key, &r.spent.saturating_add(used).to_string())?;
    }
    Ok(())
}
fn cursor(svc: &Services) -> Row {
    svc.store
        .lock()
        .unwrap()
        .state
        .cursors
        .get(REFLECT)
        .cloned()
        .unwrap_or_else(|| obj(json!({"consumer":REFLECT,"last_event_id":0,"transition":0})))
}
fn save_cursor(svc: &Services, row: Row) -> Result<()> {
    svc.store.lock().unwrap().append(Record::Cursor(row))?;
    Ok(())
}
fn index(row: &Row, key: &str) -> usize {
    row.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

pub async fn cycle(svc: &Services, force: bool) -> Result<Value> {
    let Ok(_guard) = svc.cycle_lock.try_lock() else {
        return Ok(json!({}));
    };
    if interact::has_ready_input(svc)? {
        return Ok(json!({}));
    }
    let row = cursor(svc);
    let (last, transition, due_ids) = {
        let st = svc.store.lock().unwrap();
        (
            st.state.last_event_id() as usize,
            st.state.transitions.len(),
            due(&st.state),
        )
    };
    let new = last.saturating_sub(index(&row, "last_event_id"));
    let dirty = transition > index(&row, "transition");
    let idle = row
        .get("updated_at")
        .and_then(|v| dt_of(Some(v)))
        .map_or(f64::INFINITY, |t| (now() - t).num_seconds() as f64);
    let failures = index(&row, "failures");
    let due_changed = !due_ids.is_empty() && row.get("due") != Some(&json!(due_ids));
    let new_since_failure =
        last > index(&row, "failed_last_event") || transition > index(&row, "failed_transition");
    if !force
        && !(new >= settings().reflect_every_n_events as usize
            || due_changed
            || row.get("pending") == Some(&json!(true))
            || ((new > 0 || dirty) && idle >= settings().idle_reflect_seconds))
    {
        return Ok(json!({}));
    }
    if !force
        && failures > 0
        && (idle < settings().idle_reflect_seconds
            || (failures >= settings().consumer_max_failures as usize
                && !new_since_failure
                && !due_changed))
    {
        return Ok(json!({}));
    }
    let result = maintenance(svc).await;
    if let Err(e) = &result {
        let mut row = cursor(svc);
        let mut st = svc.store.lock().unwrap();
        st.append_event(
            "runtime_failure",
            "harness",
            json!({"operation":"maintenance","error":format!("{e:#}")}),
            None,
            None,
        )?;
        row.extend(obj(json!({"consumer":REFLECT,"last_event_id":index(&row,"last_event_id"),
            "failures":index(&row,"failures")+1,"updated_at":iso(&now()),"error":format!("{e:#}"),
            "failed_last_event":st.state.last_event_id(),"failed_transition":st.state.transitions.len(),"due":due_ids})));
        st.append(Record::Cursor(row))?;
    }
    result
}

async fn maintenance(svc: &Services) -> Result<Value> {
    let mut stats = json!({});
    let (work, consolidated) = {
        let st = svc.store.lock().unwrap();
        let work = st
            .state
            .events
            .iter()
            .rposition(|e| e["type"] != "runtime_failure")
            .map_or(0, |i| i + 1);
        (work, st.state.cursor("consolidation") as usize)
    };
    if work > consolidated {
        let Some(reservation) = reserve(svc)? else {
            return Ok(json!({"maintenance":"daily token budget exhausted"}));
        };
        let staged = svc.staged();
        let merged = consolidation::run(&staged).await?;
        stats["consolidation"] = commit_all(&staged, merged, None).await?;
        staged.store.lock().unwrap().append(Record::Cursor(obj(
            json!({"consumer":"consolidation","last_event_id":work,"updated_at":iso(&now())}),
        )))?;
        svc.publish(staged, None)?;
        settle(svc, reservation)?;
    }
    // Let newly queued input take ownership at the phase boundary.
    if interact::has_ready_input(svc)? {
        stats["pending"] = json!(true);
        return Ok(stats);
    }
    let Some(reservation) = reserve(svc)? else {
        stats["maintenance"] = json!("daily token budget exhausted");
        return Ok(stats);
    };
    let mut row = cursor(svc);
    let batch: reflection::Batch = if row.get("batch").is_some_and(|v| !v.is_null()) {
        serde_json::from_value(row["batch"].clone())?
    } else {
        let st = svc.store.lock().unwrap();
        let start = index(&row, "last_event_id").min(st.state.events.len());
        let transition = index(&row, "transition").min(st.state.transitions.len());
        reflection::Batch {
            event_start: start,
            event_end: (start + 10).min(st.state.events.len()),
            transition_start: transition,
            transition_end: (transition + 10).min(st.state.transitions.len()),
        }
    };
    row.insert("consumer".into(), json!(REFLECT));
    row.insert("batch".into(), json!(batch));
    save_cursor(svc, row.clone())?; // Persist selection before inference/cancellation.
    let staged = svc.staged();
    let before = staged.store.lock().unwrap().state.transitions.len();
    let (proposals, more) = reflection::run(&staged, &batch).await?;
    stats["reflection"] = commit_all(&staged, proposals, None).await?;
    let rejected = stats["reflection"]["rejected"].as_u64().unwrap_or(0) > 0;
    let no_progress = before == staged.store.lock().unwrap().state.transitions.len();
    let retry = rejected || (more && no_progress);
    let continue_batch = more || rejected;
    let (event_end, transition_end, pending, due_ids) = {
        let st = staged.store.lock().unwrap();
        let mut e = if continue_batch {
            batch.event_start
        } else {
            batch.event_end
        };
        let mut t = if continue_batch {
            batch.transition_start
        } else {
            batch.transition_end
        };
        if !continue_batch {
            // Maintenance failures were supplied separately; own revisions are already in state.
            while e < st.state.events.len()
                && st.state.events[e]["type"] == "runtime_failure"
                && st.state.events[e]["payload"]["operation"] == "maintenance"
            {
                e += 1;
            }
            while t < st.state.transitions.len() && st.state.transitions[t]["agent"] == "reflection"
            {
                t += 1;
            }
        }
        (
            e,
            t,
            continue_batch || e < st.state.events.len() || t < st.state.transitions.len(),
            due(&st.state),
        )
    };
    row.extend(obj(
        json!({"consumer":REFLECT,"last_event_id":event_end,"transition":transition_end,
        "batch":if continue_batch {json!(batch)}else{Value::Null},"pending":pending,"due":due_ids,
        "updated_at":iso(&now()),"failures":if retry {index(&row,"failures")+1}else{0}}),
    ));
    if retry {
        let st = staged.store.lock().unwrap();
        row.insert("failed_last_event".into(), json!(st.state.last_event_id()));
        row.insert(
            "failed_transition".into(),
            json!(st.state.transitions.len()),
        );
    }
    if !retry {
        row.remove("error");
        row.remove("failed_last_event");
        row.remove("failed_transition");
    }
    save_cursor(&staged, row)?;
    let snapshot = take_snapshot(&staged)?;
    stats["snapshot"] = snapshot["id"].clone();
    stats["pending"] = json!(pending);
    svc.publish(staged, None)?;
    settle(svc, reservation)?;
    Ok(stats)
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
