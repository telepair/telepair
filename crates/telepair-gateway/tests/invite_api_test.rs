use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use telepair_core::permission::Role;
use telepair_core::storage::Storage;
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

#[tokio::test]
async fn redeem_expired_invite_rejected() {
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Create an invite that already expired (expires_at in the past)
    let expired = chrono::Utc::now() - chrono::Duration::hours(1);
    let (_invite, raw_token) = state
        .sessions
        .storage()
        .create_invite(&session_id, Role::Operator, 5, Some(expired))
        .await
        .unwrap();

    // Try to redeem with a different user — should be rejected
    let joiner_token = state.create_test_user("joiner_expired").await;

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
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Verify no participant was added
    let participants = state
        .sessions
        .storage()
        .list_participants(&session_id)
        .await
        .unwrap();
    assert!(
        participants.iter().all(|p| p.role != Role::Operator),
        "expired invite should not add a participant"
    );
}

#[tokio::test]
async fn list_sessions_only_shows_own_sessions() {
    let (state, app, owner_token) = setup().await;

    // Owner creates a session
    let _session_id = create_session(&app, &owner_token).await;

    // Create a second user who has no sessions
    let other_token = state.create_test_user("other").await;

    let resp = app
        .clone()
        .oneshot(
            Request::get("/api/sessions")
                .header("Authorization", format!("Bearer {other_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sessions: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(
        sessions.is_empty(),
        "other user should not see owner's sessions"
    );
}

#[tokio::test]
async fn redeem_exhausted_invite_rejected() {
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Create an invite with max_uses = 1
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invite"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"operator","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let invite_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let raw_token = invite_body["token"].as_str().unwrap().to_owned();

    // First user redeems successfully
    let joiner1_token = state.create_test_user("joiner1").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {joiner1_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": raw_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Second user should be rejected
    let joiner2_token = state.create_test_user("joiner2").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {joiner2_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": raw_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Verify joiner2 was NOT added as a participant
    let participants = state
        .sessions
        .storage()
        .list_participants(&session_id)
        .await
        .unwrap();
    let operator_count = participants
        .iter()
        .filter(|p| p.role == Role::Operator)
        .count();
    assert_eq!(
        operator_count, 1,
        "only the first redeemer should be a participant"
    );
}

#[tokio::test]
async fn redeem_invite_on_closed_session_rejected() {
    // Redeeming an invite against a closed session used to burn a use
    // and still insert a ghost participant — the invite counter drained
    // without doing anything useful. The fix rejects with GONE before
    // consuming the invite.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Create a valid invite.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invite"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"operator","max_uses":5}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let invite_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let raw_token = invite_body["token"].as_str().unwrap().to_owned();

    // Close the session out-of-band (same path DELETE /api/sessions/:id uses).
    state.sessions.close_session(&session_id).await.unwrap();

    // Try to redeem — should be rejected with 410 Gone.
    let joiner_token = state.create_test_user("joiner_after_close").await;
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
    assert_eq!(
        resp.status(),
        StatusCode::GONE,
        "redeeming against a closed session should report 410 Gone"
    );

    // Verify the invite use counter is still 0 (not burned) and no
    // ghost participant was added.
    let invite = state
        .sessions
        .storage()
        .find_invite(&raw_token)
        .await
        .unwrap();
    assert_eq!(
        invite.used_count, 0,
        "rejected redemption must not burn an invite use"
    );
    let participants = state
        .sessions
        .storage()
        .list_participants(&session_id)
        .await
        .unwrap();
    assert!(
        participants.iter().all(|p| p.role != Role::Operator),
        "no ghost operator participant should exist after rejected redeem"
    );
}
