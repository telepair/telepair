use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use telepair_core::permission::Role;
use telepair_core::session::CloseReason;
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

/// Mint a fresh viewer invite for `session_id` as `owner_token` and
/// return just the raw token string — the test bodies that use this
/// don't care about the rest of the response.
async fn mint_invite(app: &axum::Router, owner_token: &str, session_id: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Redeem an invite without credentials and return the minted guest
/// bearer token. Panics if the response didn't include a token; any
/// non-200 or missing token is a bug in the production code or the
/// test setup that the test should fail loudly on, not retry.
async fn anonymous_redeem(app: &axum::Router, raw_invite_token: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "token": raw_invite_token }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .expect("anonymous redeem must return a fresh guest token")
        .to_owned()
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
            Request::post(format!("/api/sessions/{session_id}/invites"))
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
            Request::post(format!("/api/sessions/{session_id}/invites"))
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
            Request::post(format!("/api/sessions/{session_id}/invites"))
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
    let participants = state.storage.list_participants(&session_id).await.unwrap();
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
            Request::post(format!("/api/sessions/{session_id}/invites"))
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
async fn redeem_by_owner_does_not_burn_invite_uses() {
    // An owner who visits their own invite link (common smoke-test
    // flow — "does this link work?") must NOT consume one use of the
    // invite. On a default `max_uses = 1` invite that would drain it
    // to zero before the real guest arrived, producing a useless link.
    // `redeem_invite` short-circuits when the caller is already a
    // participant of the session; this test pins that behaviour so a
    // future refactor can't reintroduce the burn.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"operator","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let invite_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let raw_token = invite_body["token"].as_str().unwrap().to_owned();

    // Owner redeems their own link — should succeed, report their
    // real `owner` role, and MUST NOT consume the invite use.
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {owner_token}"))
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
    assert_eq!(
        body["role"], "owner",
        "owner self-redeem must report the real role, not the invite's target role"
    );
    assert!(
        body["token"].is_null(),
        "owner self-redeem must not mint a guest token"
    );

    // The invite should still be fully redeemable by a fresh guest.
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
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "max_uses=1 invite must still have one use left after owner self-redeem"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["role"], "operator");
    let guest_token = body["token"].as_str().expect("real guest gets a token");
    let guest = state.auth.validate(guest_token).await.unwrap();
    assert!(!guest.is_admin);
}

#[tokio::test]
async fn redeem_by_existing_participant_does_not_burn_invite_uses() {
    // Same principle as the owner case: once a user is already a
    // participant, re-clicking the same link should be a no-op instead
    // of eating another `max_uses`. Exercised with an operator who
    // previously redeemed the link, then clicks it again from a stale
    // tab / bookmark.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"operator","max_uses":2}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let invite_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let raw_token = invite_body["token"].as_str().unwrap().to_owned();

    // First redemption — consumes 1 of 2 uses and seeds a participant.
    let invitee_token = state.create_test_user("operator-regular").await;
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

    // Second click from the same authenticated user — no-op, should
    // leave the remaining use intact.
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
    assert!(
        body["token"].is_null(),
        "repeat-click redeem for an existing participant must not issue a new token"
    );

    // Now a fresh guest should still be able to claim the second use.
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
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "second use must still be available after the no-op repeat-click"
    );
}

#[tokio::test]
async fn guest_token_cannot_list_targets() {
    // The entire point of `scoped_session_id`: a guest bearer token
    // must not be accepted on account-level routes. Before this
    // fix, `redeem_invite` minted a plain non-admin user whose
    // token was indistinguishable from a normal account — the
    // holder could leave their invited session and call
    // `GET /api/targets` to enumerate everything on the box. 403
    // (not 401) because the token is valid, it just doesn't have
    // the scope for this route.
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Owner mints an invite; a fresh browser redeems anonymously
    // and we pull the guest token out of the response.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let raw_token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": raw_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let guest_token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // Guest hits the dashboard endpoint — must be 403 Forbidden.
    let resp = app
        .oneshot(
            Request::get("/api/targets")
                .header("Authorization", format!("Bearer {guest_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "scoped guest must not be able to list targets"
    );
}

#[tokio::test]
async fn guest_token_cannot_create_session() {
    // Teeth of the invite privilege-escalation fix: even with a
    // valid guest token, `POST /api/sessions` must refuse. Before
    // the scope check, a redeemed viewer invite could be used to
    // spawn a brand-new shell session behind the scenes — the
    // holder was effectively a full non-admin account. This test
    // fails if a future refactor drops `require_unscoped` from
    // `create_session`.
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"operator","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let raw_token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": raw_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let guest_token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {guest_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"local-shell"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "scoped guest must not be able to create a new session"
    );
}

#[tokio::test]
async fn guest_token_is_scoped_to_redeemed_session() {
    // Directly asserts the scope binding made by `redeem_invite`.
    // The explicit field check is cheap insurance against the
    // whole-class of future bugs where somebody "simplifies"
    // `create_guest(&invite.session_id)` back to `create_guest()`.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let raw_token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let resp = app
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": raw_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let guest_token = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let guest = state.auth.validate(&guest_token).await.unwrap();
    assert_eq!(
        guest.scoped_session_id.as_deref(),
        Some(session_id.as_str()),
        "anonymous redeem must scope the guest to the invite's session"
    );
    assert!(guest.is_guest(), "is_guest helper must report true");
}

#[tokio::test]
async fn scoped_guest_cannot_redeem_other_session_invite() {
    // Cross-session pivot attempt: a guest already bound to session
    // A tries to redeem an invite targeted at session B while
    // carrying their existing token. Must be 403 — otherwise a
    // guest in any live session could collect invite URLs from
    // other sessions and use their existing identity to pivot,
    // which partially undoes the scope.
    //
    // (The guest can still open the new session the *correct* way
    // — fresh tab, no credentials — and get a second, correctly-
    // scoped guest identity.)
    let (_state, app, owner_token) = setup().await;
    let session_a = create_session(&app, &owner_token).await;
    let session_b = create_session(&app, &owner_token).await;

    // Guest redeems session A anonymously.
    let invite_a = mint_invite(&app, &owner_token, &session_a).await;
    let guest_token = anonymous_redeem(&app, &invite_a).await;

    // Owner mints an invite for session B.
    let invite_b = mint_invite(&app, &owner_token, &session_b).await;

    // Guest (scoped to A) tries to redeem B with their token.
    let resp = app
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {guest_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": invite_b}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "scoped guest must not pivot to another session's invite"
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
            Request::post(format!("/api/sessions/{session_id}/invites"))
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
    let participants = state.storage.list_participants(&session_id).await.unwrap();
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
async fn create_invite_with_expires_in_minutes_populates_expires_at() {
    // The UI-facing knob is a relative TTL ("expire in 60 minutes") —
    // easier to reason about than an absolute ISO timestamp. The HTTP
    // handler resolves it to an absolute `expires_at` before the DB
    // sees anything, so the response must echo back a concrete
    // timestamp that clients can show or count down against.
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let before = chrono::Utc::now();
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"role":"viewer","max_uses":2,"expires_in_minutes":60}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let expires_at_raw = body["expires_at"]
        .as_str()
        .expect("expires_at must be present and non-null when TTL is supplied");
    let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at_raw)
        .expect("expires_at should be a valid RFC3339 timestamp")
        .with_timezone(&chrono::Utc);

    let delta = expires_at - before;
    // The handler computes `now + 60 minutes`; give a generous slack
    // for the test to stay stable on slow runners.
    assert!(
        delta >= chrono::Duration::minutes(59) && delta <= chrono::Duration::minutes(61),
        "expires_at should be ~60 minutes in the future, got delta {delta:?}"
    );
}

#[tokio::test]
async fn create_invite_rejects_absolute_expires_at_in_the_past() {
    // A timestamp in the past is always a client bug — the UI either
    // picked the wrong timezone or the user fat-fingered a date. Better
    // to 400 loudly than to mint a pre-expired invite that looks fine
    // in the UI but dies on the first redeem.
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let body = serde_json::json!({
        "role": "viewer",
        "max_uses": 1,
        "expires_at": past,
    })
    .to_string();

    let resp = app
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_invite_rejects_excessive_max_uses() {
    // `max_uses = 10_000` is not an invite, it's a public URL. The
    // server caps at 100 so a typo in the UI can't produce one
    // accidentally. Anything above the cap is a 400 (not a silent
    // clamp) so the UI can surface the limit to the user.
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let resp = app
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer","max_uses":10000}"#))
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
            Request::post(format!("/api/sessions/{session_id}/invites"))
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
        .storage
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
    let participants = state.storage.list_participants(&session_id).await.unwrap();
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
async fn guest_token_only_sees_its_own_session_on_list() {
    // F5-q1 regression: a scoped guest minted via invite redemption
    // must only see the session they were invited to when they call
    // `GET /api/sessions`. The QA run observed a guest token listing
    // a session they had no participant row for — this test locks
    // down that a guest's view is the intersection of "sessions I
    // joined" and "everything", even when other sessions exist.
    //
    // Anchor setup: two independent owners, two sessions. The guest
    // is only invited into `session_a`; `session_b` must never leak.
    let (state, app, owner_a_token) = setup().await;
    let owner_b_token = state.create_test_user("owner_b").await;

    let session_a = create_session(&app, &owner_a_token).await;
    let session_b = create_session(&app, &owner_b_token).await;

    let raw_invite = mint_invite(&app, &owner_a_token, &session_a).await;
    let guest_token = anonymous_redeem(&app, &raw_invite).await;

    let resp = app
        .oneshot(
            Request::get("/api/sessions")
                .header("Authorization", format!("Bearer {guest_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sessions: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    let ids: Vec<&str> = sessions.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![session_a.as_str()]);
    assert!(
        !ids.iter().any(|id| *id == session_b),
        "guest must not see session_b in the list"
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
            Request::post(format!("/api/sessions/{session_id}/invites"))
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
    let participants = state.storage.list_participants(&session_id).await.unwrap();
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
            Request::post(format!("/api/sessions/{session_id}/invites"))
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
    state
        .sessions
        .close_session(&session_id, CloseReason::Owner, None)
        .await
        .unwrap();

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
    let invite = state.storage.find_invite(&raw_token).await.unwrap();
    assert_eq!(
        invite.used_count, 0,
        "rejected redemption must not burn an invite use"
    );
    let participants = state.storage.list_participants(&session_id).await.unwrap();
    assert!(
        participants.iter().all(|p| p.role != Role::Operator),
        "no ghost operator participant should exist after rejected redeem"
    );
}

#[tokio::test]
async fn create_invite_on_closed_session_returns_gone() {
    // Symmetric to `redeem_invite_on_closed_session_rejected`: the
    // *creation* side of the invite lifecycle must also reject a
    // closed session. Before this gate, an owner who closed a
    // session and then opened the invite dialog (or hit the API
    // directly) would get `201 Created` with a fresh token —
    // `redeem_invite` then bounced that token with 410 because the
    // status check on the redeem path *is* there. Net result: zombie
    // invite rows in the DB and a guaranteed-broken share link in
    // the owner's clipboard. The two halves of the lifecycle must
    // agree on what "alive" means.
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Close the session out-of-band so the next request hits a
    // genuinely closed row, same path the DELETE handler uses.
    state
        .sessions
        .close_session(&session_id, CloseReason::Owner, None)
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::GONE,
        "creating an invite on a closed session must report 410 Gone — otherwise the owner gets \
         a 201 with a token that redeem will reject, leaving zombie invite rows in the DB"
    );

    // Belt-and-braces: confirm the response body did NOT include a
    // token field. If a future refactor moves the gate to *after*
    // `create_invite` runs, the storage write would already be
    // visible and the response shape would silently leak it back.
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    if !bytes.is_empty() {
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("error response body must be valid JSON");
        assert!(
            body.get("token").is_none(),
            "rejected create_invite must not return a token field, got: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Invite management — GET list + DELETE revoke
// ---------------------------------------------------------------------------

/// Happy path for the management dialog: mint three invites, list
/// them, assert all three come back with the owner-only fields the UI
/// needs (token_prefix, remaining_uses, role, session_id).
#[tokio::test]
async fn list_session_invites_returns_all_invites() {
    let (_, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Mint three invites with different roles so ordering is observable.
    let _a = mint_invite(&app, &owner_token, &session_id).await;
    let _b = mint_invite(&app, &owner_token, &session_id).await;
    let _c = mint_invite(&app, &owner_token, &session_id).await;

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(rows.len(), 3, "all three invites must list");

    // Each row must carry the UI-facing fields and MUST NOT leak the
    // raw bearer token.
    for row in &rows {
        assert!(row["token_sha256"].is_string());
        assert_eq!(
            row["token_prefix"].as_str().unwrap().len(),
            4,
            "token_prefix is 4 chars of the sha for a stable UI label"
        );
        assert_eq!(row["session_id"], session_id);
        assert!(row["max_uses"].is_number());
        assert!(row["used_count"].is_number());
        assert!(row["remaining_uses"].is_number());
        assert!(
            row.get("token").is_none(),
            "raw token MUST NOT appear in list response"
        );
    }
}

/// Non-owner callers must not be able to enumerate another session's
/// invites, even if they're authenticated as a real user.
#[tokio::test]
async fn list_session_invites_rejects_non_owner() {
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;
    let stranger_token = state.create_test_user("stranger").await;

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {stranger_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// Unauthenticated GET must be 401, not 403 — the handler does auth
/// before owner check and this distinction matters for client retry
/// logic.
#[tokio::test]
async fn list_session_invites_rejects_missing_auth() {
    let (_, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/sessions/{session_id}/invites"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// DELETE happy path: mint an invite, list it to get its sha, revoke
/// it, verify list is empty and that redeem on the raw token fails.
#[tokio::test]
async fn revoke_session_invite_hard_deletes() {
    let (_, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;
    let raw = mint_invite(&app, &owner_token, &session_id).await;

    // Pull the row to get its token_sha256.
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    let sha = rows[0]["token_sha256"].as_str().unwrap().to_owned();

    // Revoke.
    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/sessions/{session_id}/invites/{sha}"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // List is empty now.
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(rows.is_empty(), "revoke must remove the row from listings");

    // And the raw token fails redeem with 400 (the invite is gone).
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::json!({ "token": raw }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Revoking an already-revoked invite is idempotent — a double-DELETE
/// resolves to 204 so the UI doesn't have to special-case "already
/// gone" with an error toast when two admins race a revoke.
#[tokio::test]
async fn revoke_session_invite_twice_is_idempotent() {
    let (_, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;
    mint_invite(&app, &owner_token, &session_id).await;

    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    let sha = rows[0]["token_sha256"].as_str().unwrap().to_owned();

    // First revoke — succeeds.
    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/sessions/{session_id}/invites/{sha}"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Second revoke — still 204. Idempotent.
    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/sessions/{session_id}/invites/{sha}"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // A completely fabricated sha is also 204 — the server must not
    // leak whether the token ever existed.
    let fake_sha = "0".repeat(64);
    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/sessions/{session_id}/invites/{fake_sha}"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

/// Cross-session probe: stranger has their own session, tries to
/// revoke an invite that belongs to someone else's session via their
/// own path. Must read as 204 (indistinguishable from "already gone"),
/// with the real invite untouched — idempotency on the wire + strict
/// session scoping for side effects.
#[tokio::test]
async fn revoke_session_invite_cross_session_probe_is_silent_noop() {
    let (state, app, owner_a_token) = setup().await;
    let session_a = create_session(&app, &owner_a_token).await;
    mint_invite(&app, &owner_a_token, &session_a).await;

    // Get session A's invite sha (with owner A's credentials).
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/sessions/{session_a}/invites"))
                .header("Authorization", format!("Bearer {owner_a_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    let sha_a = rows[0]["token_sha256"].as_str().unwrap().to_owned();

    // Second user creates their own session; then tries to DELETE
    // session A's invite via their own path.
    let owner_b_token = state.create_test_user("owner-b").await;
    let session_b = create_session(&app, &owner_b_token).await;

    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/sessions/{session_b}/invites/{sha_a}"))
                .header("Authorization", format!("Bearer {owner_b_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // 204. A 400 here would light up the UI with an error toast AND
    // leak that the attempted DELETE went down a different path from
    // a regular "already gone" — a 204/400 branch on the wire is a
    // yes/no oracle on "does this sha exist in session Y?".
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Owner A's invite must still be intact — the stranger's probe
    // must not have side effects.
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/sessions/{session_a}/invites"))
                .header("Authorization", format!("Bearer {owner_a_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(rows.len(), 1, "cross-session probe must not delete");
}

/// Regression for a QA finding where a caller that sent
/// `expires_in_secs` got silently ignored: the backend only accepted
/// `expires_in_minutes`, `serde` dropped the unknown field on the
/// floor, and the response came back with `expires_at: null`. The
/// fix adds `expires_in_secs` as a first-class field, resolved to an
/// absolute `expires_at` at the HTTP boundary so the service-layer
/// validation (past / ceiling) catches bad values uniformly.
#[tokio::test]
async fn create_invite_with_expires_in_secs_populates_expires_at() {
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let before = chrono::Utc::now();
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"role":"viewer","max_uses":2,"expires_in_secs":120}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let expires_at_raw = body["expires_at"]
        .as_str()
        .expect("expires_at must be populated when expires_in_secs is supplied");
    let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at_raw)
        .expect("expires_at should be a valid RFC3339 timestamp")
        .with_timezone(&chrono::Utc);
    let delta = expires_at - before;
    assert!(
        delta >= chrono::Duration::seconds(110) && delta <= chrono::Duration::seconds(130),
        "expires_at should be ~120 seconds in the future, got delta {delta:?}"
    );
}

/// `expires_in_secs` wins over `expires_in_minutes` when both are
/// supplied — the finer-grained field is treated as the more recent
/// expression of caller intent. Prevents a silent "one of these wins"
/// situation that pre-fix callers had no way to check against.
#[tokio::test]
async fn create_invite_expires_in_secs_beats_minutes() {
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    let before = chrono::Utc::now();
    let resp = app
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                // 30 s vs 60 min: seconds must win.
                .body(Body::from(
                    r#"{"role":"viewer","expires_in_secs":30,"expires_in_minutes":60}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let expires_at = chrono::DateTime::parse_from_rfc3339(body["expires_at"].as_str().unwrap())
        .unwrap()
        .with_timezone(&chrono::Utc);
    let delta = expires_at - before;
    assert!(
        delta <= chrono::Duration::seconds(45),
        "expires_in_secs must take precedence over expires_in_minutes, got delta {delta:?}"
    );
}

/// Regression for F6-q1: the `CreateInvite` response must echo back
/// the effective `max_uses` (defaulting to 1 when omitted) and any
/// `expires_at` so the UI / CLI renders the server-of-record values
/// rather than guessing from the request. Before this was explicit in
/// a test, future refactors could quietly drop these fields from the
/// response body.
#[tokio::test]
async fn create_invite_response_echoes_max_uses_and_expires_at() {
    let (_state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Omit max_uses entirely; the default (1) must surface in the response.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invites"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["max_uses"].as_i64(), Some(1));
    assert_eq!(body["role"].as_str(), Some("viewer"));
    // No TTL requested → expires_at is null.
    assert!(body["expires_at"].is_null());
    assert!(
        body["token"].as_str().is_some_and(|t| !t.is_empty()),
        "raw token must flow through the response"
    );
}

/// Regression for a QA finding where `/api/join/{token}` was assumed
/// to exist and returned 405 (the mistaken read was a real bug). The
/// canonical redeem endpoint is `POST /api/invite/redeem`; no route
/// exists at `/api/join/*`, so any method on that path must 404 via
/// the router fallback rather than misleadingly 405.
#[tokio::test]
async fn api_join_path_is_unrouted_404() {
    let (_state, app, _owner_token) = setup().await;

    for method in ["GET", "POST", "PUT", "DELETE"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/api/join/some-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "{method} /api/join/<token> must 404, got {}",
            resp.status()
        );
    }
}
