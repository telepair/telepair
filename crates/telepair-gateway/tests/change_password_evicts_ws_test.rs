//! Regression: `POST /api/auth/change-password` must evict the
//! caller's already-attached WebSocket(s).
//!
//! The auth-service rotates the bearer token atomically with the
//! password hash, so every future HTTP request carrying the old
//! token fails 401. WebSockets, however, only authenticate during
//! the `SessionJoin` handshake — once attached, the PTY pipe stays
//! open on the old identity until the client disconnects. Without
//! an explicit eviction step that leaves the exact threat password
//! rotation is supposed to mitigate — a leaked token — effective
//! against HTTP but toothless against a long-lived WS.
//!
//! The `change_password` handler therefore calls
//! `hub.evict_user(user.id)` on success, which broadcasts
//! `PeerEvicted` and closes the socket via the same code path
//! admin-disable already exercises.

#![deny(unsafe_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tower::ServiceExt;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::permission::Role;
use telepair_core::protocol::ServerMessage;
use telepair_core::session::InputMode;
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::build_router;
use telepair_gateway::state::AppState;

async fn start_server() -> (String, axum::Router, AppState, Arc<SqliteStorage>) {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let state = AppState::new(
        storage.clone(),
        TargetEngine::empty(),
        None,
        None,
        std::path::PathBuf::from("/tmp/telepair-test-change-pw"),
    )
    .await;
    let router = build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let serve = router.clone();
    tokio::spawn(async move {
        axum::serve(listener, serve).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (addr, router, state, storage)
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
async fn change_password_evicts_live_ws_connections() {
    let (addr, router, state, storage) = start_server().await;

    // Seed a password-login user. `admin_create_user` hashes the
    // password and mints a bearer, which is what `change_password`
    // expects callers to hold.
    let (owner, owner_token) = state
        .auth_service
        .admin_create_user("worker@example.test", "worker", "hunter2xx", false, true)
        .await
        .expect("seed user");
    let session = storage
        .create_session_with_owner(owner.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    storage
        .upsert_participant(&session.id, owner.id, Role::Owner)
        .await
        .unwrap();

    // Attach a WS with the pre-rotation token and wait for the
    // handshake to settle, so the subsequent change-password can't
    // race the attach.
    let url = format!("ws://{addr}/ws/session/{}", session.id);
    let (mut ws, _) = connect_async(url).await.expect("ws connect");
    ws.send(session_join_msg(&session.id, &owner_token))
        .await
        .unwrap();
    let first = tokio::time::timeout(std::time::Duration::from_secs(3), recv_json(&mut ws))
        .await
        .expect("timed out waiting for SessionState")
        .expect("ws closed before SessionState");
    assert!(
        matches!(first, ServerMessage::SessionState { .. }),
        "expected SessionState first, got {first:?}"
    );

    // Rotate the password via the real HTTP handler on the shared
    // state. The response body is ignored; the eviction is what
    // we're actually testing.
    let resp = router
        .clone()
        .oneshot(
            Request::post("/api/auth/change-password")
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"current_password":"hunter2xx","new_password":"newpass321"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "change-password should succeed"
    );

    // PeerEvicted targeting the caller must land on the WS before
    // it closes — exactly the contract the admin-disable path pins
    // in `session_enabled_gate_test.rs`.
    let mut saw_evicted = false;
    for _ in 0..10 {
        let next =
            tokio::time::timeout(std::time::Duration::from_secs(3), recv_json(&mut ws)).await;
        match next {
            Ok(Some(ServerMessage::PeerEvicted { user_id, reason })) => {
                assert_eq!(user_id, owner.id, "PeerEvicted targeted wrong user");
                assert_eq!(
                    reason,
                    telepair_core::protocol::EvictReason::TokenRotated,
                    "password change must evict with TokenRotated, not AccountDisabled — \
                     the user's account is fine, only their bearer rotated",
                );
                saw_evicted = true;
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => panic!("timed out waiting for PeerEvicted"),
        }
    }
    assert!(saw_evicted, "WS closed without ever delivering PeerEvicted");

    let closed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None => return true,
                Some(Ok(_)) => continue,
                Some(Err(_)) => return true,
            }
        }
    })
    .await
    .expect("ws did not close within 3s after change-password");
    assert!(closed, "ws next() returned something other than Close");

    drop(state);
    drop(storage);
}
