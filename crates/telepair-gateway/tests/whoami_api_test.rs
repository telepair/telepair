//! Integration tests for `GET /api/auth/whoami`.
//!
//! The endpoint exists so the frontend auth store can cache the
//! caller's `user_id` immediately after login. The dashboard uses that
//! id to gate the closed-row click on `session.owner_id === current
//! UserId` — without it, non-owner participants can still see closed
//! sessions in their history list (because `list_sessions_for_user`
//! returns "owned OR joined") and clicking through would deterministi
//! cally hit the owner-only `/audit` endpoint and 403.
//!
//! These tests pin the exact response shape (`user_id` / `name` /
//! `is_admin` / `is_guest`) the frontend depends on, plus the 401 path
//! for missing or bogus bearers.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use telepair_gateway::build_router;
use telepair_gateway::state::AppState;
use tower::ServiceExt;

async fn fetch_whoami(app: &axum::Router, token: Option<&str>) -> (StatusCode, serde_json::Value) {
    let mut req = Request::get("/api/auth/whoami");
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body =
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

#[tokio::test]
async fn whoami_returns_caller_identity() {
    // Happy path: a real (non-guest) user gets back their id, name,
    // and `is_admin=false`. The id MUST be a parseable UUID string —
    // the frontend compares it against `session.owner_id` which the
    // backend serializes the same way, so any drift would silently
    // break the closed-row click gating.
    let state = AppState::new_test().await;
    let token = state.create_test_user("alice").await;
    let app = build_router(state);

    let (status, body) = fetch_whoami(&app, Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"].as_str(), Some("alice"));
    assert_eq!(body["is_admin"].as_bool(), Some(false));
    assert_eq!(body["is_guest"].as_bool(), Some(false));
    let user_id = body["user_id"].as_str().expect("user_id must be a string");
    uuid::Uuid::parse_str(user_id).expect("user_id must be a parseable uuid");
}

#[tokio::test]
async fn whoami_without_bearer_is_unauthorized() {
    // Defense-in-depth: there's no fallback identity. A missing
    // Authorization header must produce a clean 401 — never a 200
    // with a default user, never a 500.
    let state = AppState::new_test().await;
    let app = build_router(state);

    let (status, _) = fetch_whoami(&app, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn whoami_with_bogus_bearer_is_unauthorized() {
    // A syntactically-valid Authorization header that doesn't match
    // any user must still 401, not 500. This is the path a stale
    // localStorage token would hit after the server's DB was reset.
    let state = AppState::new_test().await;
    let app = build_router(state);

    let (status, _) = fetch_whoami(&app, Some("definitely-not-a-real-token")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
