use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use telepair_core::session::{InputMode, Session, SessionStatus};
use telepair_core::storage::Storage;
use telepair_gateway::build_router;
use telepair_gateway::state::AppState;
use tower::ServiceExt; // for oneshot

async fn setup() -> (axum::Router, String) {
    let state = AppState::new_test().await;
    let token = state.create_test_user("tester").await;
    let router = build_router(state);
    (router, token)
}

#[tokio::test]
async fn health_check() {
    let (app, _) = setup().await;
    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn list_targets_requires_auth() {
    let (app, _) = setup().await;
    let resp = app
        .oneshot(Request::get("/api/targets").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_targets_with_auth() {
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/targets")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn create_session_and_list() {
    let (app, token) = setup().await;
    // Create session
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

    // List sessions
    let resp = app
        .oneshot(
            Request::get("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn list_sessions_target_name_query_param_narrows_results() {
    // Regression for the v0.1.1-dev bug where `ListSessionsQuery`
    // deserialized `target` instead of `target_name`, so the
    // frontend's `?target_name=local-shell` filter silently fell
    // through to the unfiltered query. Seed two sessions with
    // different `target_name`s for the same user (bypassing the
    // create_session handler, which only knows `local-shell`), then
    // GET /api/sessions?target_name=local-shell and assert only the
    // matching row comes back.
    let state = AppState::new_test().await;
    let (user, token) = state
        .storage
        .create_user("filter-tester", false)
        .await
        .unwrap();
    let kept = state
        .storage
        .create_session_with_owner(user.id, "local-shell", InputMode::Multiplexed)
        .await
        .unwrap();
    state
        .storage
        .create_session_with_owner(user.id, "other-target", InputMode::Multiplexed)
        .await
        .unwrap();
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::get("/api/sessions?target_name=local-shell")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sessions: Vec<Session> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sessions.len(), 1, "filter should narrow to one row");
    assert_eq!(sessions[0].id, kept.id);
    assert_eq!(sessions[0].target_name, "local-shell");
}

#[tokio::test]
async fn list_sessions_admin_sees_other_users_sessions() {
    // Regression for a v0.1.1 gap where `list_sessions` unconditionally
    // filtered by the caller's owner/participant membership, ignoring
    // admin status. The admin targets card advertises "N active
    // sessions on target X" via a global count, and its deep-link
    // navigates the admin to `/?target=X`. Before the fix, the list
    // the admin landed on was filtered by the admin's own membership,
    // so the count and the list could silently disagree — the admin
    // saw "5 active" and an empty list.
    //
    // The fix routes the HTTP handler through
    // `SessionService::list_sessions_visible_to`, which switches to
    // the unscoped storage query when `user.is_admin` is true. This
    // test exercises the full path: HTTP → service → storage.
    let state = AppState::new_test().await;
    let (alice, _) = state.storage.create_user("alice", false).await.unwrap();
    state
        .storage
        .create_session_with_owner(alice.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();
    state
        .storage
        .create_session_with_owner(alice.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();
    let admin_token = state.create_test_admin("root").await;
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::get("/api/sessions")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sessions: Vec<Session> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        sessions.len(),
        2,
        "admin must see sessions they neither own nor participate in"
    );
    assert!(sessions.iter().all(|s| s.owner_id == alice.id));
}

#[tokio::test]
async fn list_sessions_non_admin_is_still_owner_scoped() {
    // Mirror test for the admin bypass: a plain non-admin user must
    // NOT gain visibility into other users' sessions just because the
    // admin branch exists. Regression guard against a future refactor
    // that accidentally flips the default.
    let state = AppState::new_test().await;
    let (alice, _) = state.storage.create_user("alice", false).await.unwrap();
    state
        .storage
        .create_session_with_owner(alice.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();
    let bob_token = state.create_test_user("bob").await;
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::get("/api/sessions")
                .header("Authorization", format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sessions: Vec<Session> = serde_json::from_slice(&bytes).unwrap();
    assert!(
        sessions.is_empty(),
        "non-admin must not see sessions they did not own or join"
    );
}

#[tokio::test]
async fn delete_session_owner_succeeds_and_marks_closed() {
    // The HTTP `DELETE /api/sessions/:id` handler used to inline the
    // ownership check; the H4 refactor moved it into
    // `SessionService::close_session_as_owner`. Pin the wired path
    // end-to-end (HTTP → service → storage) so future contributors
    // can't quietly drop the auth check from the handler.
    let state = AppState::new_test().await;
    let (alice, alice_token) = state.storage.create_user("alice", false).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(alice.id, "local-shell", InputMode::Multiplexed)
        .await
        .unwrap();
    let storage = state.storage.clone();
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::delete(format!("/api/sessions/{}", session.id))
                .header("Authorization", format!("Bearer {alice_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let after = storage.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(after.status, SessionStatus::Closed);
}

#[tokio::test]
async fn delete_session_non_owner_gets_403_and_session_stays_active() {
    // Non-owner must hit 403 (not 401, not 404) and the session must
    // remain Active. The earlier inline check returned 403 directly;
    // after the refactor the same status comes from
    // `Error::PermissionDenied → http_status() == 403`. Asserting on
    // both the status AND the post-call session row guarantees we
    // didn't accidentally close-then-403.
    let state = AppState::new_test().await;
    let (alice, _) = state.storage.create_user("alice", false).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(alice.id, "local-shell", InputMode::Multiplexed)
        .await
        .unwrap();
    let bob_token = state.create_test_user("bob").await;
    let storage = state.storage.clone();
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::delete(format!("/api/sessions/{}", session.id))
                .header("Authorization", format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let still = storage.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(
        still.status,
        SessionStatus::Active,
        "403 path must not have side-effected the session row"
    );
}

#[tokio::test]
async fn delete_session_missing_id_returns_404() {
    // The third arm of the require_owner contract: missing session →
    // SessionNotFound → 404. Pin it from the HTTP edge so the mapping
    // through `Error::http_status` stays correct.
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::delete("/api/sessions/no-such-session")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
