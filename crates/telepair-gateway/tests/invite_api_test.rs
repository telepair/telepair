use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use telepair_gateway::build_router;
use telepair_gateway::state::AppState;
use tower::ServiceExt;

async fn setup() -> (AppState, axum::Router, String) {
    let state = AppState::new_test().await;
    let token = state.create_test_user("owner").await;
    let router = build_router(state.clone());
    (state, router, token)
}

/// Helper to create a session via the API, returning the session id.
async fn create_session(app: &axum::Router, token: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"local-shell"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    body["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn create_and_redeem_invite() {
    let (state, app, owner_token) = setup().await;

    // Create a session as the owner
    let session_id = create_session(&app, &owner_token).await;

    // Create an invite token for the session
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invite"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"operator","max_uses":3}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let invite_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(invite_body["role"], "operator");
    assert_eq!(invite_body["max_uses"], 3);
    assert_eq!(invite_body["session_id"], session_id);
    let raw_token = invite_body["token"].as_str().unwrap();

    // Create a second user to redeem the invite
    let joiner_token = state.create_test_user("joiner").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {joiner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": raw_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let redeem_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(redeem_body["session_id"], session_id);
    assert_eq!(redeem_body["role"], "operator");
}

#[tokio::test]
async fn invite_requires_auth() {
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Create invite without auth header
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invite"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Redeem invite without auth header
    let resp = app
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"token":"bogus"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
