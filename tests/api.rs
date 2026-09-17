mod common;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn call(app: Router, method: &str, url: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(url)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned())),
    )
}

#[tokio::test]
async fn accepted_errors_keep_request_id_and_retry_preserves_response_shape() {
    let svc = Arc::new(common::services("api"));
    let app = morpho::api::router(svc.clone());
    assert_eq!(
        call(app.clone(), "POST", "/interact", json!({"text":""}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(app.clone(), "GET", "/requests/missing", Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    common::fake(&svc).fail("model offline");
    let (status, pending) = call(
        app.clone(),
        "POST",
        "/interact",
        json!({"text":"hello","speaker":"alice"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id = pending["request_id"].as_str().unwrap();
    assert_eq!(pending["status"], "pending");
    let body = json!({"text":"hello","speaker":"alice","request_id":id});
    let (status, reply) = call(app.clone(), "POST", "/interact", body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    for key in [
        "response",
        "event_id",
        "response_event_id",
        "context",
        "request_id",
    ] {
        assert!(!reply[key].is_null());
    }
    assert_eq!(call(app.clone(), "POST", "/interact", body).await.1, reply);
    assert_eq!(
        call(
            app.clone(),
            "POST",
            "/interact",
            json!({"text":"changed","speaker":"alice","request_id":id})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, usage) = call(app, "GET", "/usage", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(usage["llm_calls"], 3); // a failed reply call, then reply and clerk
}
