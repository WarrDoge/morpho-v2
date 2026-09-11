//! Interaction lifecycle (§30): event → interpretation → commit → context → reply → event.

use anyhow::Result;
use serde_json::{Value, json};

use crate::Services;
use crate::agents::interaction;
use crate::context::composer::compose;
use crate::llm::is_miss;
use crate::state::engine;

pub const RESPONSE_SYSTEM: &str =
    "You are an assistant with a persistent cognitive state. The STATE below is a
projection of what you currently remember, believe, and are working on; it is your only source of
continuity. Treat it as a fallible model, not ground truth, and say when you are uncertain.
Answer the user's message helpfully and concisely.";

pub async fn interact(svc: &Services, text: &str, session_id: Option<&str>) -> Result<Value> {
    let event = svc.store.lock().unwrap().append_event(
        "user_message",
        "user",
        json!({"text": text}),
        session_id,
    )?;
    let eid = event["event_id"].as_str().unwrap_or_default().to_string();
    let interpreted = match interaction::run(svc, &event).await {
        Ok(props) => engine::commit(&svc.store, &svc.llm, &props, Some(&eid))
            .await
            .map(|_| ()),
        Err(e) => Err(e),
    };
    if let Err(e) = interpreted {
        if is_miss(&e) {
            return Err(e);
        }
        tracing::error!("interaction agent failed; responding from existing state: {e:#}");
    }
    let (context, manifest) = compose(svc, text, None).await?;
    let system = format!("{RESPONSE_SYSTEM}\n\n# STATE\n{context}");
    let reply = svc.llm.complete_text(&system, text).await?;
    let reply_event = svc.store.lock().unwrap().append_event(
        "assistant_message",
        "assistant",
        json!({"text": reply, "in_reply_to": eid}),
        session_id,
    )?;
    Ok(json!({
        "response": reply,
        "event_id": eid,
        "response_event_id": reply_event["event_id"],
        "context": manifest,
    }))
}
