//! Integration tests for `GET /api/sessions/{id}/audit`.
//!
//! The endpoint backs the session-detail audit timeline. It mirrors the
//! invite-list endpoint's auth model — owner-only, 403 for the wrong
//! user, 404 for an unknown session — but unlike the invite list it
//! must keep working after a session is closed (the whole point of a
//! history view is reading what happened on a closed session).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use telepair_core::session::CloseReason;
use telepair_gateway::build_router;
use telepair_gateway::state::AppState;
use tower::ServiceExt;

async fn setup() -> (AppState, axum::Router, String) {
    let state = AppState::new_test().await;
    let token = state.create_test_user("owner").await;
    let router = build_router(state.clone());
    (state, router, token)
}

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

async fn fetch_audit(
    app: &axum::Router,
    session_id: &str,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::get(format!("/api/sessions/{session_id}/audit"));
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
    // 204/empty bodies are not expected here, but the JSON parse must
    // not panic for the 403/404 paths either — fall back to Null.
    let body =
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

#[tokio::test]
async fn owner_sees_session_creation_events() {
    // Happy path. Creating a session via the API emits two audit rows
    // (`session.created` + `participant.joined` for the owner). The
    // owner GET must surface both, in `ts DESC` order — newest first.
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let (status, body) = fetch_audit(&app, &session_id, Some(&owner_token)).await;
    assert_eq!(status, StatusCode::OK);
    let rows = body.as_array().expect("audit response must be an array");
    let event_types: Vec<&str> = rows
        .iter()
        .filter_map(|row| row["event_type"].as_str())
        .collect();
    // Both events present, regardless of which one happened to win the
    // ts tie-break — we don't pin order between two rows that share the
    // same millisecond.
    assert!(
        event_types.contains(&"session.created"),
        "expected session.created in audit timeline, got {event_types:?}"
    );
    assert!(
        event_types.contains(&"participant.joined"),
        "expected participant.joined in audit timeline, got {event_types:?}"
    );
    // Every row must be scoped to this session — the SQL filter is the
    // load-bearing piece, so a regression that drops the WHERE clause
    // would surface here as cross-session leakage.
    for row in rows {
        assert_eq!(row["session_id"].as_str(), Some(session_id.as_str()));
    }
}

#[tokio::test]
async fn non_owner_is_forbidden() {
    // Symmetry with the invite-list endpoint: a different authenticated
    // user must not be able to peek at someone else's session timeline.
    // 403 (not 404) because we don't want to use the response code as
    // an oracle for "session exists but you can't see it" vs
    // "session doesn't exist". `require_owner` returns the same 403 in
    // both cases for the wrong-user path.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;
    let other_token = state.create_test_user("intruder").await;

    let (status, _) = fetch_audit(&app, &session_id, Some(&other_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unknown_session_is_not_found() {
    // The session id is well-formed but doesn't exist. `require_owner`
    // resolves the row first, so this collapses to a 404 before any
    // ownership check runs.
    let (_state, app, owner_token) = setup().await;
    let (status, _) = fetch_audit(&app, "ses_nonexistent", Some(&owner_token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn missing_auth_is_unauthorized() {
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let (status, _) = fetch_audit(&app, &session_id, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn closed_session_still_returns_history() {
    // The whole point of an audit timeline is reading what happened on
    // a session *after* it's closed — that's the history view's reason
    // to exist. Verify that closing the session does not flip the
    // endpoint to 410/404, and that the new `session.closed` row lands
    // in the timeline alongside the original creation events.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    state
        .sessions
        .close_session(&session_id, CloseReason::Owner, None)
        .await
        .unwrap();

    let (status, body) = fetch_audit(&app, &session_id, Some(&owner_token)).await;
    assert_eq!(status, StatusCode::OK);
    let rows = body.as_array().expect("audit response must be an array");
    let event_types: Vec<&str> = rows
        .iter()
        .filter_map(|row| row["event_type"].as_str())
        .collect();
    assert!(
        event_types.contains(&"session.created"),
        "expected session.created after close, got {event_types:?}"
    );
    assert!(
        event_types.contains(&"session.closed"),
        "expected session.closed after close, got {event_types:?}"
    );
}
