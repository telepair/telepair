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
async fn create_invite_still_requires_auth() {
    // Post-invite-flow-rewrite: redeem is now anonymous, but *creating*
    // an invite is still an owner-only privileged action. An
    // unauthenticated caller must not be able to mint invite tokens.
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

    // Redeem with a bogus token (no auth) — must NOT 401 anymore.
    // The endpoint accepts anonymous calls; it rejects bogus tokens
    // with 400 because the lookup fails, not because auth is missing.
    // This is the main contract change: collaborators can hit
    // /api/invite/redeem without any credentials.
    let resp = app
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"token":"bogus"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "anonymous redeem of an invalid token should be 400, not 401 (the redeem endpoint no longer requires auth)"
    );
}

#[tokio::test]
async fn redeem_without_auth_issues_guest_token() {
    // The heart of the new invite flow: a browser with no existing
    // telepair token should be able to POST /api/invite/redeem and
    // receive (a) the session_id/role it joined as, and (b) a freshly
    // minted guest token it can use for all subsequent API + WS calls.
    // Before this change, the only way to get a token was to share
    // the admin token, which broke real multi-user collaboration.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Owner mints an invite for an operator.
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

    // Visitor has no token — this is the cold-start collaborator case.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "token": raw_token }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["session_id"], session_id);
    assert_eq!(body["role"], "operator");

    let guest_token = body["token"]
        .as_str()
        .expect("anonymous redeem must return a fresh token");
    assert!(
        !guest_token.is_empty(),
        "guest token must be non-empty when no auth was provided"
    );

    // The token must immediately validate as a real, non-admin user.
    // This is the handoff contract the frontend depends on: store it,
    // then navigate straight into the session page with full auth.
    let guest_user = state.auth.validate(guest_token).await.unwrap();
    assert!(!guest_user.is_admin, "guest must never be granted admin");
    assert!(
        guest_user.name.starts_with("guest-"),
        "guest name should use guest- prefix, got {}",
        guest_user.name
    );

    // And the guest should be recorded as a participant on the session
    // so the WS handshake's NOT_PARTICIPANT check lets them in.
    let participants = state
        .sessions
        .storage()
        .list_participants(&session_id)
        .await
        .unwrap();
    assert!(
        participants.iter().any(|p| p.user_id == guest_user.id),
        "guest should be listed as an active participant"
    );
}

#[tokio::test]
async fn redeem_with_auth_reuses_existing_user() {
    // Complementary case to the guest test: when the caller already
    // has a bearer token, the redeem handler should NOT create a
    // throwaway guest — it should reuse the caller's identity and
    // omit the `token` field from the response. This is the path
    // an admin takes when testing their own invite link.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Create an invite + a distinct "invitee" user who will redeem it.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invite"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let invite_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let raw_token = invite_body["token"].as_str().unwrap().to_owned();

    let invitee_token = state.create_test_user("invitee-with-token").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {invitee_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "token": raw_token }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["session_id"], session_id);
    assert_eq!(body["role"], "viewer");
    assert!(
        body["token"].is_null(),
        "authenticated redeem must NOT issue a new token (would duplicate the user's identity); got {}",
        body["token"]
    );
}

#[tokio::test]
async fn redeem_issues_distinct_guest_per_redemption() {
    // A multi-use invite should mint a unique guest per redemption.
    // If the handler accidentally reused a single guest account, all
    // invitees would collide on the same participant row and only
    // one of them would actually be present in the session.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // max_uses = 3 so we can redeem three times over.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invite"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer","max_uses":3}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let invite_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let raw_token = invite_body["token"].as_str().unwrap().to_owned();

    let mut guest_ids = Vec::new();
    for _ in 0..3 {
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/invite/redeem")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "token": raw_token }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let guest_token = body["token"].as_str().unwrap();
        let guest = state.auth.validate(guest_token).await.unwrap();
        guest_ids.push(guest.id);
    }

    // All three guest users must be distinct.
    let unique: std::collections::HashSet<_> = guest_ids.iter().collect();
    assert_eq!(
        unique.len(),
        3,
        "three redemptions should produce three distinct guest users, got {guest_ids:?}"
    );

    // And all three must be listed as active participants.
    let participants = state
        .sessions
        .storage()
        .list_participants(&session_id)
        .await
        .unwrap();
    for id in &guest_ids {
        assert!(
            participants.iter().any(|p| p.user_id == *id),
            "guest {id} should be an active participant"
        );
    }
}

#[tokio::test]
async fn create_session_rejects_unknown_input_mode() {
    // Before the error-handling pass, an unknown `input_mode` value
    // silently collapsed to Serialized. That's a client-side typo
    // masquerading as a successful request, and it could flip input
    // permissions in a way the caller never asked for. Loud 400.
    let (_state, app, owner_token) = setup().await;

    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"target_name":"local-shell","input_mode":"not-a-real-mode"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_invite_rejects_zero_max_uses_with_400() {
    // The storage layer returns InvalidInput for max_uses < 1, but
    // the old handler mapped every error to 500. A bad client request
    // should come back as a 4xx so the caller knows it's their fault,
    // not the server's.
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let resp = app
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invite"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer","max_uses":0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn redeem_invalid_token_without_auth_returns_400_not_500() {
    // Regression for the error-handling audit: anonymous bogus redeem
    // must report BAD_REQUEST, not INTERNAL_SERVER_ERROR. A client-side
    // mistake (wrong/rotten token) should not look like a server crash.
    let (_state, app, _owner_token) = setup().await;
    let resp = app
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"token":"definitely-not-a-real-token"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
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
