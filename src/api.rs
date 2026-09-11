//! HTTP surface: interaction lifecycle (§30) and state inspection (§25).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Services;
use crate::context::composer::compose_text;
use crate::interact::{await_reply, enqueue, request};
use crate::state::models::table_for;
use crate::worker::cycle;

type App = Arc<Services>;
type Reply = Result<Json<Value>, (StatusCode, String)>;

fn internal(e: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
}

fn table_of(name: &str) -> Option<&'static str> {
    Some(match name {
        "memories" => "memories",
        "beliefs" => "beliefs",
        "goals" => "goals",
        "predictions" => "predictions",
        "entities" => "entities",
        "relationships" => "entity_relationships",
        _ => return None,
    })
}

#[derive(Deserialize)]
pub struct Interact {
    pub text: String,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
    pub speaker: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub limit: Option<usize>,
    pub after: Option<i64>,
    pub decision: Option<String>,
    pub status: Option<String>,
    pub text: Option<String>,
    pub reflect: Option<bool>,
}

pub fn router(svc: App) -> Router {
    Router::new()
        .route("/interact", post(interact_h))
        .route("/requests/{id}", get(request_h))
        .route("/usage", get(usage_h))
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/state", get(state_h))
        .route("/events", get(events_h))
        .route("/proposals", get(proposals_h))
        .route("/transitions", get(transitions_h))
        .route("/snapshots", get(snapshots_h))
        .route("/why/{object_id}", get(why_h))
        .route("/context/preview", get(preview_h))
        .route("/admin/cycle", post(cycle_h))
        .route("/{table}", get(list_h))
        .with_state(svc)
}

async fn interact_h(
    State(svc): State<App>,
    Json(body): Json<Interact>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    let id = enqueue(
        &svc,
        &body.text,
        body.session_id.as_deref(),
        body.speaker.as_deref().unwrap_or("user"),
        body.request_id.as_deref(),
    )
    .map_err(|e| {
        let message = e.to_string();
        let status = if message == "request id conflict" {
            StatusCode::CONFLICT
        } else if message.starts_with("invalid ")
            || message.starts_with("text ")
            || message == "session id too long"
        {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (status, message)
    })?;
    match await_reply(&svc, &id).await {
        Ok(reply) => Ok((StatusCode::OK, Json(reply))),
        Err(_) => Ok((
            StatusCode::ACCEPTED,
            Json(request(&svc, &id).map_err(internal)?),
        )),
    }
}

async fn usage_h(State(svc): State<App>) -> Json<Value> {
    Json(json!(svc.llm.counts()))
}

async fn state_h(State(svc): State<App>) -> Json<Value> {
    let st = svc.store.lock().unwrap();
    Json(json!({"working_state": st.state.working, "self_state": st.state.self_state}))
}

async fn events_h(State(svc): State<App>, Query(q): Query<ListQuery>) -> Json<Value> {
    Json(json!(
        svc.store
            .lock()
            .unwrap()
            .state
            .recent_events(q.limit.unwrap_or(50))
    ))
}

async fn proposals_h(State(svc): State<App>, Query(q): Query<ListQuery>) -> Json<Value> {
    let st = svc.store.lock().unwrap();
    Json(json!(
        st.state
            .proposals(q.decision.as_deref(), q.limit.unwrap_or(100))
    ))
}

async fn transitions_h(State(svc): State<App>, Query(q): Query<ListQuery>) -> Json<Value> {
    let st = svc.store.lock().unwrap();
    Json(json!(
        st.state
            .transitions(q.limit.unwrap_or(100), q.after.unwrap_or(0))
    ))
}

async fn snapshots_h(State(svc): State<App>, Query(q): Query<ListQuery>) -> Reply {
    let st = svc.store.lock().unwrap();
    st.snapshots(q.limit.unwrap_or(20))
        .map(|s| Json(json!(s)))
        .map_err(internal)
}

/// Provenance chain: object → transitions → proposals → evidence (§25).
async fn why_h(State(svc): State<App>, Path(object_id): Path<String>) -> Reply {
    let table =
        table_for(&object_id).ok_or((StatusCode::NOT_FOUND, "unknown object id prefix".into()))?;
    let st = svc.store.lock().unwrap();
    let obj = st
        .state
        .get(table, &object_id)
        .ok_or((StatusCode::NOT_FOUND, "not found".into()))?;
    let history = st.state.transitions_for(&object_id, 50);
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let collect = |v: Option<&Value>, ids: &mut BTreeSet<String>| {
        for i in v
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            ids.insert(i.to_string());
        }
    };
    for key in [
        "evidence",
        "source_events",
        "supporting_evidence",
        "contradicting_evidence",
    ] {
        collect(obj.get(key), &mut ids);
    }
    for t in &history {
        collect(t.get("evidence"), &mut ids);
    }
    let event_ids: Vec<String> = ids
        .iter()
        .filter(|i| i.starts_with("evt_"))
        .cloned()
        .collect();
    let events = st.state.events_many(&event_ids);
    let derived: Vec<Value> = ids
        .iter()
        .filter(|i| *i != &object_id)
        .filter_map(|i| table_for(i).and_then(|t| st.state.get(t, i)))
        .map(|r| json!(r))
        .collect();
    Ok(Json(json!({
        "object": obj, "table": table, "history": history, "events": events, "derived_from": derived,
    })))
}

async fn preview_h(State(svc): State<App>, Query(q): Query<ListQuery>) -> Reply {
    let (context, manifest) = compose_text(&svc, q.text.as_deref().unwrap_or(""), None)
        .await
        .map_err(internal)?;
    Ok(Json(json!({"context": context, "manifest": manifest})))
}

async fn cycle_h(State(svc): State<App>, Query(q): Query<ListQuery>) -> Reply {
    cycle(&svc, q.reflect.unwrap_or(false))
        .await
        .map(Json)
        .map_err(internal)
}

async fn list_h(
    State(svc): State<App>,
    Path(table): Path<String>,
    Query(q): Query<ListQuery>,
) -> Reply {
    let table = table_of(&table).ok_or((StatusCode::NOT_FOUND, "not found".into()))?;
    let statuses: Option<Vec<&str>> = q.status.as_deref().map(|s| s.split(',').collect());
    let st = svc.store.lock().unwrap();
    Ok(Json(json!(st.state.list_rows(
        table,
        statuses.as_deref(),
        q.limit.unwrap_or(100)
    ))))
}

async fn request_h(State(svc): State<App>, Path(id): Path<String>) -> Reply {
    request(&svc, &id).map(Json).map_err(|e| {
        if e.to_string() == "request not found" {
            (StatusCode::NOT_FOUND, e.to_string())
        } else {
            internal(e)
        }
    })
}
