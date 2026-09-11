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

fn append(
    svc: &Services,
    kind: &str,
    source: &str,
    payload: Value,
    session: Option<&str>,
    emb: &[f32],
) -> Result<serde_json::Map<String, Value>> {
    let mut st = svc.store.lock().unwrap();
    let slot = st.vectors.append(emb)?;
    st.append_event(kind, source, payload, session, Some(slot))
}

pub async fn interact(svc: &Services, text: &str, session_id: Option<&str>) -> Result<Value> {
    let emb = svc.llm.embed(&[text.to_string()]).await?.remove(0);
    let event = append(
        svc,
        "user_message",
        "user",
        json!({"text": text}),
        session_id,
        &emb,
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
    let (context, manifest) = compose(svc, &emb, None)?;
    let system = format!("{RESPONSE_SYSTEM}\n\n# STATE\n{context}");
    let reply = svc.llm.complete_text(&system, text).await?;
    let reply_emb = svc.llm.embed(std::slice::from_ref(&reply)).await?.remove(0);
    let payload = json!({"text": reply, "in_reply_to": eid});
    let reply_event = append(
        svc,
        "assistant_message",
        "assistant",
        payload,
        session_id,
        &reply_emb,
    )?;
    Ok(json!({
        "response": reply,
        "event_id": eid,
        "response_event_id": reply_event["event_id"],
        "context": manifest,
    }))
}
