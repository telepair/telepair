//! Regression tests for the `session_enabled = FALSE` gate.
//!
//! The v0.1.2 critical fix is: a freshly email-registered account
//! lands with `session_enabled = FALSE` and must not be able to
//! create or attach to a session until an admin flips the bit. We
//! enforce it on two surfaces:
//!
//! - `POST /api/sessions` — `require_session_enabled` returns 403
//!   before the handler ever touches the target registry.
//! - `GET /ws/session/{id}` — after the `SessionJoin` handshake,
//!   `handle_socket` sends a `SESSION_DISABLED` error frame and
//!   closes the connection before the participant lookup runs.
//!
//! Admins bypass both gates so the bootstrap path (first admin
//! token on a blank db) never self-locks. Scoped guests are minted
//! with `session_enabled = TRUE` and ride the scope pin, so they
//! never hit this code path — the only callers that can be denied
//! here are real accounts that an admin either created disabled or
//! later disabled via `/api/admin/users/{id}/disable`.

#![deny(unsafe_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tower::ServiceExt;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::audit::{AuditEventType, AuditFilter};
use telepair_core::permission::Role;
use telepair_core::protocol::{ServerMessage, error_codes};
use telepair_core::session::{InputMode, SessionListFilter};
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router, build_router_with_options};

// ── Shared helpers ───────────────────────────────────────────────────

/// Build a router + shared storage handle. Tests drive the storage
/// directly to seed disabled users (the public HTTP register→verify
/// path is tested elsewhere and would require a live SMTP stub).
async fn setup() -> (axum::Router, AppState, Arc<SqliteStorage>) {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let state = AppState::new(
        storage.clone(),
        TargetEngine::empty(),
        None,
        None,
        std::path::PathBuf::from("/tmp/telepair-test"),
    )
    .await;
    let router = build_router_with_options(state.clone(), None, CorsMode::AllowAny).unwrap();
    (router, state, storage)
}

/// Seed a user and force `session_enabled` to FALSE. `create_user`
/// seeds TRUE (the legacy-admin / programmatic path), so we flip
/// immediately to mirror what `verify_pending_registration` writes
/// for a self-registered email signup.
async fn seed_disabled(storage: &Arc<SqliteStorage>, name: &str) -> (uuid::Uuid, String) {
    let (user, token) = storage.create_user(name, false).await.unwrap();
    storage.set_session_enabled(user.id, false).await.unwrap();
    (user.id, token)
}

// ── HTTP gate: POST /api/sessions ────────────────────────────────────

#[tokio::test]
async fn create_session_disabled_user_is_403_and_emits_audit() {
    let (app, state, storage) = setup().await;
    let (user_id, token) = seed_disabled(&storage, "pending").await;

    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"local-shell"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // The gate must fire BEFORE session creation — there should be
    // zero session rows for this user. A regression that moved the
    // gate after `create_session_with_owner` would leak a row here
    // even though the HTTP response is 403, and the next time an
    // admin enables the account the orphan session would appear on
    // their dashboard.
    let rows = state
        .storage
        .list_sessions_for_user(user_id, SessionListFilter::default())
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "disabled user leaked session row: {rows:?}"
    );

    // The rejection must be audited so operators can see disabled
    // accounts probing the endpoint. `path` distinguishes HTTP from
    // WS so dashboards can split the two surfaces.
    let sink = telepair_core::audit::AuditSink::new(storage.clone());
    let events = sink.query(AuditFilter::default()).await.unwrap();
    let row = events
        .iter()
        .find(|e| e.event_type == AuditEventType::AuthSessionAccessDenied)
        .expect("expected auth.session_access_denied row");
    assert_eq!(row.detail["path"], "POST /api/sessions");
    assert_eq!(
        row.actor_id.map(|id| id.to_string()),
        Some(user_id.to_string())
    );
}

#[tokio::test]
async fn create_session_admin_bypasses_gate_even_if_bit_is_false() {
    // The admin bootstrap path must not be able to lock itself out:
    // an admin whose `session_enabled` is somehow FALSE (e.g. a
    // legacy row that existed before the column was added, or an
    // operator who poked the db by hand) must still be able to
    // create a session. The gate is `!session_enabled && !is_admin`,
    // so admin short-circuits. This test pins that branch.
    let (app, _, storage) = setup().await;
    let (admin, admin_token) = storage.create_user("root", true).await.unwrap();
    storage.set_session_enabled(admin.id, false).await.unwrap();

    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {admin_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"local-shell"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn create_session_enabled_user_succeeds() {
    // Happy-path sanity — `session_enabled = TRUE` (the default for
    // `create_user`) clears the gate cleanly. Pairs with the 403
    // test above so a regression that always-allows or always-denies
    // is caught by whichever test flips.
    let (app, _, storage) = setup().await;
    let (_user, token) = storage.create_user("normal", false).await.unwrap();

    let resp = app
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
}

// ── WS gate: GET /ws/session/{id} ────────────────────────────────────

/// Bind the router to an ephemeral port so tungstenite can upgrade.
/// Tests that only need the axum handler use `oneshot`; WS handshake
/// needs a live socket. Returns the addr and the live state handle so
/// the test can still seed storage rows directly.
async fn start_server() -> (String, AppState, Arc<SqliteStorage>) {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let state = AppState::new(
        storage.clone(),
        TargetEngine::empty(),
        None,
        None,
        std::path::PathBuf::from("/tmp/telepair-test"),
    )
    .await;
    let router = build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (addr, state, storage)
}

fn session_join_msg(session_id: &str, token: &str) -> Message {
    Message::Text(
        serde_json::json!({
            "type": "SessionJoin",
            "session_id": session_id,
            "token": token,
        })
        .to_string()
        .into(),
    )
}

/// Pull the first JSON frame the server sends us. Mirrors the helper
/// in `ws_test.rs` — the server can interleave binary PTY frames on
/// attach, so we skip anything that isn't a text message.
async fn recv_json<S>(stream: &mut S) -> Option<ServerMessage>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str::<ServerMessage>(&text).ok();
            }
            Some(Ok(Message::Close(_))) | None => return None,
            Some(Err(_)) => return None,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn ws_attach_disabled_user_gets_session_disabled_error() {
    let (addr, state, storage) = start_server().await;

    // Seed owner(admin) + a session they own. Then seed a second
    // user, make them a participant of that session, and disable
    // them. The gate must fire BEFORE the participant check — the
    // whole point of disabling is to render an already-approved
    // participant inert, not just to block new joins. If we only
    // gated on "not a participant" we'd miss the revocation case.
    let (admin, _admin_token) = storage.create_user("root", true).await.unwrap();
    let session = storage
        .create_session_with_owner(admin.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();

    let (disabled_id, disabled_token) = seed_disabled(&storage, "pending").await;
    storage
        .upsert_participant(&session.id, disabled_id, Role::Viewer)
        .await
        .unwrap();

    let url = format!("ws://{addr}/ws/session/{}", session.id);
    let (mut ws, _) = connect_async(url).await.expect("ws connect failed");
    ws.send(session_join_msg(&session.id, &disabled_token))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), recv_json(&mut ws))
        .await
        .expect("timed out waiting for error frame");
    match msg {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, error_codes::SESSION_DISABLED);
        }
        other => panic!("expected SESSION_DISABLED error, got: {other:?}"),
    }

    // Audit row with the WS-side path marker. Paired with the HTTP
    // test above, an operator can count disabled-account probes per
    // surface from a single query.
    let sink = telepair_core::audit::AuditSink::new(storage.clone());
    let events = sink.query(AuditFilter::default()).await.unwrap();
    let row = events
        .iter()
        .find(|e| {
            e.event_type == AuditEventType::AuthSessionAccessDenied
                && e.detail["path"] == "WS /ws/session/{id}"
        })
        .expect("expected auth.session_access_denied row for ws surface");
    assert_eq!(
        row.actor_id.map(|id| id.to_string()),
        Some(disabled_id.to_string())
    );
    assert_eq!(row.session_id.as_deref(), Some(session.id.as_str()));

    // `state` is held so the server task's AppState doesn't drop
    // mid-test (the spawned axum::serve would otherwise tear down).
    drop(state);
}

// ── HTTP gates: invite mint / revoke / redeem + participant-role ─────

/// Seed a session owned by a disabled user. The flow is: create the
/// user with `session_enabled = TRUE` (so `create_session_with_owner`
/// has a clean precondition), then disable them. This matches what
/// happens in production when an admin flips an active owner.
async fn seed_session_with_disabled_owner(
    storage: &Arc<SqliteStorage>,
    name: &str,
) -> (uuid::Uuid, String, telepair_core::session::Session) {
    let (user, token) = storage.create_user(name, false).await.unwrap();
    let session = storage
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    storage.set_session_enabled(user.id, false).await.unwrap();
    (user.id, token, session)
}

#[tokio::test]
async fn create_invite_disabled_owner_is_403_and_audited() {
    // A disabled owner must not keep minting invites against a session
    // that predates the disable. Before the gate, `POST /invites` only
    // checked ownership + session-active, so a disabled owner could
    // grow the participant roster until the session closed. The gate
    // fires before `InviteService::create`, so no invite row should
    // appear.
    let (app, _, storage) = setup().await;
    let (owner_id, owner_token, session) =
        seed_session_with_disabled_owner(&storage, "owner").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{}/invites", session.id))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"viewer","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // No invite row leaked — the gate beat the mint.
    let rows = storage.list_invites_for_session(&session.id).await.unwrap();
    assert!(
        rows.is_empty(),
        "disabled owner leaked invite row: {rows:?}"
    );

    let sink = telepair_core::audit::AuditSink::new(storage.clone());
    let events = sink.query(AuditFilter::default()).await.unwrap();
    let row = events
        .iter()
        .find(|e| {
            e.event_type == AuditEventType::AuthSessionAccessDenied
                && e.detail["path"] == "POST /api/sessions/{id}/invites"
        })
        .expect("expected auth.session_access_denied row for invite mint");
    assert_eq!(
        row.actor_id.map(|id| id.to_string()),
        Some(owner_id.to_string())
    );
    assert_eq!(row.session_id.as_deref(), Some(session.id.as_str()));
}

#[tokio::test]
async fn revoke_invite_disabled_owner_is_403() {
    // Revoke is a session-level mutation — gated alongside mint. A
    // disabled owner should not be able to tear down invites that
    // might be needed for a replacement operator to finish cleanup.
    let (app, _, storage) = setup().await;
    // Seed the invite while the owner is still enabled so the mint
    // succeeds at the storage layer, then disable and try to revoke.
    let (user, owner_token) = storage.create_user("owner", false).await.unwrap();
    let session = storage
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    let (invite, _raw) = storage
        .create_invite(&session.id, Role::Viewer, 1, None)
        .await
        .unwrap();
    storage.set_session_enabled(user.id, false).await.unwrap();

    let resp = app
        .oneshot(
            Request::delete(format!(
                "/api/sessions/{}/invites/{}",
                session.id, invite.token_sha256
            ))
            .header("Authorization", format!("Bearer {owner_token}"))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Invite row still present — revoke was blocked at the gate.
    let rows = storage.list_invites_for_session(&session.id).await.unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn redeem_invite_disabled_user_is_403_and_no_participant_row() {
    // A disabled user with a valid token must not redeem an invite.
    // Pre-fix, the redeem path consumed a use AND wrote a participant
    // row before the WS attach layer got a chance to reject — a
    // one-shot invite was effectively griefable. The gate fires before
    // the storage transaction, so `used_count` must stay 0 and no
    // participant row should appear.
    let (app, _, storage) = setup().await;
    let (admin, _admin_token) = storage.create_user("root", true).await.unwrap();
    let session = storage
        .create_session_with_owner(admin.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    let (_, raw_token) = storage
        .create_invite(&session.id, Role::Viewer, 1, None)
        .await
        .unwrap();
    let (disabled_id, disabled_token) = seed_disabled(&storage, "pending").await;

    let resp = app
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {disabled_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "token": raw_token }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Invite still pristine.
    let rows = storage.list_invites_for_session(&session.id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].used_count, 0, "disabled redeem burned a use");

    // No participant row for the disabled user.
    let role = storage
        .find_active_participant_role(&session.id, disabled_id)
        .await
        .unwrap();
    assert!(role.is_none(), "disabled redeem leaked participant row");

    // Audit row carries the redeem path. `session_id` is None here —
    // the gate fires before the invite preview resolves, so we don't
    // know the target session yet. That's a deliberate tradeoff: the
    // sooner we reject, the less state we touch.
    let sink = telepair_core::audit::AuditSink::new(storage.clone());
    let events = sink.query(AuditFilter::default()).await.unwrap();
    events
        .iter()
        .find(|e| {
            e.event_type == AuditEventType::AuthSessionAccessDenied
                && e.detail["path"] == "POST /api/invite/redeem"
        })
        .expect("expected auth.session_access_denied row for invite redeem");
}

#[tokio::test]
async fn update_participant_role_disabled_owner_is_403() {
    // Role changes are a session-level mutation — same gate as mint.
    let (app, _, storage) = setup().await;
    let (owner_id, owner_token, session) =
        seed_session_with_disabled_owner(&storage, "owner").await;
    let (target, _) = storage.create_user("peer", false).await.unwrap();
    storage
        .upsert_participant(&session.id, target.id, Role::Viewer)
        .await
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/sessions/{}/participants/{}/role",
                    session.id, target.id
                ))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"operator"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Role unchanged in storage.
    let role = storage
        .find_active_participant_role(&session.id, target.id)
        .await
        .unwrap();
    assert_eq!(role, Some(Role::Viewer));

    // Audit row with the role-update path.
    let sink = telepair_core::audit::AuditSink::new(storage.clone());
    let events = sink.query(AuditFilter::default()).await.unwrap();
    let row = events
        .iter()
        .find(|e| {
            e.event_type == AuditEventType::AuthSessionAccessDenied
                && e.detail["path"] == "PUT /api/sessions/{id}/participants/{user_id}/role"
        })
        .expect("expected auth.session_access_denied row for role update");
    assert_eq!(
        row.actor_id.map(|id| id.to_string()),
        Some(owner_id.to_string())
    );
    assert_eq!(row.session_id.as_deref(), Some(session.id.as_str()));
}
