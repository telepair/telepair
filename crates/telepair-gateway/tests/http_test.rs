use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use telepair_agent::virtual_target::TargetEngine;
use telepair_core::session::{CreateUserTargetParams, InputMode, Session, SessionStatus};
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
        .create_session_with_owner(user.id, "local-shell", InputMode::Multiplexed, None)
        .await
        .unwrap();
    state
        .storage
        .create_session_with_owner(user.id, "other-target", InputMode::Multiplexed, None)
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
        .create_session_with_owner(alice.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    state
        .storage
        .create_session_with_owner(alice.id, "local-shell", InputMode::Serialized, None)
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
        .create_session_with_owner(alice.id, "local-shell", InputMode::Serialized, None)
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
    // `SessionService::close_session_by_user`. Pin the wired path
    // end-to-end (HTTP → service → storage) so future contributors
    // can't quietly drop the auth check from the handler.
    let state = AppState::new_test().await;
    let (alice, alice_token) = state.storage.create_user("alice", false).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(alice.id, "local-shell", InputMode::Multiplexed, None)
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
        .create_session_with_owner(alice.id, "local-shell", InputMode::Multiplexed, None)
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
async fn delete_session_is_idempotent_on_already_closed() {
    // Regression for F3: a UI double-click on Close used to flash a
    // "session not found" toast because the second DELETE matched
    // zero rows at the storage `UPDATE ... WHERE status='active'`
    // filter and bubbled up as 404. Contract now: already-closed is
    // indistinguishable from "just closed" to the caller — both
    // return 204.
    let state = AppState::new_test().await;
    let (alice, alice_token) = state.storage.create_user("alice", false).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(alice.id, "local-shell", InputMode::Multiplexed, None)
        .await
        .unwrap();
    let app = build_router(state);

    let first = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/sessions/{}", session.id))
                .header("Authorization", format!("Bearer {alice_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::NO_CONTENT);

    let second = app
        .oneshot(
            Request::delete(format!("/api/sessions/{}", session.id))
                .header("Authorization", format!("Bearer {alice_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        second.status(),
        StatusCode::NO_CONTENT,
        "double-close must be idempotent, not surface as 404"
    );
}

#[tokio::test]
async fn delete_session_admin_can_force_close_foreign_session() {
    // Regression for F4: after disabling a user, operators need a
    // way to clean up the sessions that user still owns without
    // waiting for the idle reaper. Admin DELETE must succeed against
    // a session they don't own, with the close reason stamped as
    // `admin` so history views don't misattribute the action.
    let state = AppState::new_test().await;
    let (alice, _alice_token) = state.storage.create_user("alice", false).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(alice.id, "local-shell", InputMode::Multiplexed, None)
        .await
        .unwrap();
    let admin_token = state.create_test_admin("admin").await;
    let storage = state.storage.clone();
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::delete(format!("/api/sessions/{}", session.id))
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let after = storage.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(after.status, SessionStatus::Closed);
    assert_eq!(
        after.closed_reason.map(|r| r.as_str()),
        Some("admin"),
        "admin force-close must stamp `admin`, not `owner`"
    );
}

// ── Fix #2: stable target identity end-to-end ────────────────────────────────
//
// The bug these tests guard against: `create_session` used to take only
// `target_name`, then resolved global-first / user-target-fallback. If a
// global target with the same name as a user target existed, the user could
// never launch their own — clicking it always launched the global one.
// Worse, on WS attach the resolution flowed back through the same name, so a
// global target added *after* session create could shadow the user target
// mid-session. The fix carries stable identity: callers pass exactly one of
// `target_id` (user-owned, by nanoid) or `target_name` (global, by name); the
// session row records `user_target_id` so the WS path resolves the right
// target unconditionally.

/// Build a `TargetEngine` containing a single virtual `vps` target so we
/// can stage a name collision against a user-owned `vps` target. We use
/// `from_yaml` (rather than poking the struct directly) so the `local-shell`
/// auto-injection inside `from_yaml` runs — production never sees a
/// `TargetEngine` without it.
fn engine_with_global_vps() -> TargetEngine {
    TargetEngine::from_yaml(
        "targets:\n  - name: vps\n    display: Global VPS\n    type: virtual\n    command: /bin/echo\n    args: [\"global\"]\n",
    )
    .expect("yaml fixture must parse")
}

#[tokio::test]
async fn create_session_with_target_id_resolves_user_target_even_when_global_collides() {
    // RED first: today the handler ignores `target_id` entirely (it only
    // accepts `target_name`), so this POST 400s. After the fix the user's
    // own `vps` is launched and the session row carries `user_target_id`
    // so WS attach can resolve it without ever consulting the global engine.
    let state = AppState::new_test().await;
    state.targets.store(Arc::new(engine_with_global_vps()));
    let (user, token) = state.storage.create_user("alice", false).await.unwrap();
    let user_target = state
        .storage
        .create_user_target(CreateUserTargetParams {
            user_id: user.id,
            name: "vps".into(),
            display: "Alice VPS".into(),
            command: "/bin/echo".into(),
            args: vec!["user".into()],
            env: Default::default(),
            tags: vec![],
        })
        .await
        .unwrap();
    let app = build_router(state);

    let body = format!(r#"{{"target_id":"{}"}}"#, user_target.id);
    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let session: Session = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        session.user_target_id.as_deref(),
        Some(user_target.id.as_str()),
        "session row must record the user_target_id so WS attach is unambiguous"
    );
    assert_eq!(session.target_name, "vps");
}

#[tokio::test]
async fn create_session_with_target_name_resolves_global_when_user_target_with_same_name_exists() {
    // Counterpart: even when the caller owns a `vps` user-target, asking
    // for the global `vps` by name MUST resolve the global one and leave
    // `user_target_id` unset on the session row. No silent fallback in
    // either direction.
    let state = AppState::new_test().await;
    state.targets.store(Arc::new(engine_with_global_vps()));
    let (user, token) = state.storage.create_user("alice", false).await.unwrap();
    state
        .storage
        .create_user_target(CreateUserTargetParams {
            user_id: user.id,
            name: "vps".into(),
            display: "Alice VPS".into(),
            command: "/bin/echo".into(),
            args: vec!["user".into()],
            env: Default::default(),
            tags: vec![],
        })
        .await
        .unwrap();
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"vps"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let session: Session = serde_json::from_slice(&bytes).unwrap();
    assert!(
        session.user_target_id.is_none(),
        "global launches must not stamp a user_target_id"
    );
    assert_eq!(session.target_name, "vps");
}

#[tokio::test]
async fn create_session_rejects_when_neither_target_id_nor_target_name() {
    // Empty body (or just `input_mode`) used to fall through to a 400
    // from JsonRejection because `target_name` was a required field.
    // After the fix it stays 400 — but as an explicit "must specify
    // exactly one of target_id / target_name" guard rather than a
    // serde-level missing-field error, so future shape changes don't
    // accidentally make `{}` mean "give me the local-shell".
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_session_rejects_when_both_target_id_and_target_name() {
    // Defence in depth: a client that sends both fields is confused
    // about which target it wants. We refuse rather than picking one
    // silently, otherwise a frontend regression that left the old
    // `target_name` field in place alongside the new `target_id` would
    // continue to launch the wrong target without any error surfacing.
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"target_id":"abc","target_name":"local-shell"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_session_with_target_id_returns_404_for_other_users_target() {
    // Bob must not be able to launch Alice's user-target by guessing or
    // sniffing her nanoid. We return 404 (not 403) so the response can't
    // be used to enumerate other users' target ids — the row's existence
    // stays hidden from anyone who doesn't own it.
    let state = AppState::new_test().await;
    let (alice, _) = state.storage.create_user("alice", false).await.unwrap();
    let alice_target = state
        .storage
        .create_user_target(CreateUserTargetParams {
            user_id: alice.id,
            name: "vps".into(),
            display: "Alice VPS".into(),
            command: "/bin/echo".into(),
            args: vec!["user".into()],
            env: Default::default(),
            tags: vec![],
        })
        .await
        .unwrap();
    let bob_token = state.create_test_user("bob").await;
    let app = build_router(state);

    let body = format!(r#"{{"target_id":"{}"}}"#, alice_target.id);
    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {bob_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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
