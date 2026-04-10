#![deny(unsafe_code)]

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use uuid::Uuid;

use telepair_core::permission::Role;
use telepair_core::protocol::ServerMessage;
use telepair_core::session::{CloseReason, InputMode, Session};
use telepair_core::storage::Storage;
use telepair_gateway::state::AppState;

/// Create a user and a session they own (owner participant row inserted
/// atomically). Returns `(token, user_id, session)` so tests can skip
/// a few lines of setup.
async fn owned_session(state: &AppState, username: &str) -> (String, Uuid, Session) {
    let token = state.create_test_user(username).await;
    let user = state.auth.validate(&token).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();
    (token, user.id, session)
}

async fn start_server() -> (String, AppState) {
    let state = AppState::new_test().await;
    let router = telepair_gateway::build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (addr, state)
}

fn ws_url(addr: &str, session_id: &str) -> String {
    format!("ws://{addr}/ws/session/{session_id}")
}

fn session_join_msg(session_id: &str, token: &str) -> Message {
    Message::Text(
        serde_json::json!({
            "type": "SessionJoin",
            "session_id": session_id,
            "token": token
        })
        .to_string()
        .into(),
    )
}

/// Parse a text WebSocket frame into a ServerMessage.
fn parse_server_msg(msg: &Message) -> Option<ServerMessage> {
    match msg {
        Message::Text(text) => serde_json::from_str::<ServerMessage>(text).ok(),
        _ => None,
    }
}

/// Receive the next JSON (text) server message, skipping binary frames.
async fn recv_json<S>(stream: &mut S) -> Option<ServerMessage>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match stream.next().await {
            Some(Ok(msg)) => {
                if let Some(server_msg) = parse_server_msg(&msg) {
                    return Some(server_msg);
                }
                // Skip binary frames (PTY output) and other non-text frames
                if matches!(msg, Message::Close(_)) {
                    return None;
                }
            }
            _ => return None,
        }
    }
}

// --------------------------------------------------------------------------
// Test 1: Auth timeout — connect but send nothing, server closes connection
// --------------------------------------------------------------------------
#[tokio::test]
async fn ws_auth_timeout() {
    let (addr, state) = start_server().await;
    let (_token, _user_id, session) = owned_session(&state, "tester").await;

    let (mut ws, _) = connect_async(ws_url(&addr, &session.id))
        .await
        .expect("failed to connect");

    // Don't send anything — the server should close within ~5 seconds
    let result = tokio::time::timeout(std::time::Duration::from_secs(7), recv_json(&mut ws)).await;

    match result {
        Ok(Some(ServerMessage::Error { code, .. })) => {
            assert_eq!(code, "AUTH_TIMEOUT");
        }
        Ok(None) => {
            // Connection closed without a message — also acceptable
        }
        Err(_) => panic!("server did not close the connection within 7 seconds"),
        Ok(Some(other)) => panic!("expected AUTH_TIMEOUT error, got: {other:?}"),
    }

    // Connection should be closed now
    let _ = ws.close(None).await;
}

// --------------------------------------------------------------------------
// Test 2: Invalid token — send SessionJoin with bad token
// --------------------------------------------------------------------------
#[tokio::test]
async fn ws_invalid_token_rejected() {
    let (addr, state) = start_server().await;
    let (_token, _user_id, session) = owned_session(&state, "tester").await;

    let (mut ws, _) = connect_async(ws_url(&addr, &session.id))
        .await
        .expect("failed to connect");

    ws.send(session_join_msg(&session.id, "totally-bogus-token"))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), recv_json(&mut ws))
        .await
        .expect("timed out waiting for error");

    match msg {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "AUTH_FAILED");
        }
        other => panic!("expected AUTH_FAILED error, got: {other:?}"),
    }
}

// --------------------------------------------------------------------------
// Test 3: Non-participant — user B tries to join user A's session
// --------------------------------------------------------------------------
#[tokio::test]
async fn ws_non_participant_rejected() {
    let (addr, state) = start_server().await;

    // User A creates a session
    let (_token_a, _user_a_id, session) = owned_session(&state, "alice").await;

    // User B is NOT added as participant
    let token_b = state.create_test_user("bob").await;

    let (mut ws, _) = connect_async(ws_url(&addr, &session.id))
        .await
        .expect("failed to connect");

    ws.send(session_join_msg(&session.id, &token_b))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), recv_json(&mut ws))
        .await
        .expect("timed out waiting for error");

    match msg {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "NOT_PARTICIPANT");
        }
        other => panic!("expected NOT_PARTICIPANT error, got: {other:?}"),
    }
}

// --------------------------------------------------------------------------
// Test 4: Closed session — try to connect to a closed session
// --------------------------------------------------------------------------
#[tokio::test]
async fn ws_closed_session_rejected() {
    let (addr, state) = start_server().await;

    let (token, _user_id, session) = owned_session(&state, "tester").await;

    // Close the session
    state
        .storage
        .close_session(&session.id, CloseReason::Owner)
        .await
        .unwrap();

    let (mut ws, _) = connect_async(ws_url(&addr, &session.id))
        .await
        .expect("failed to connect");

    ws.send(session_join_msg(&session.id, &token))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), recv_json(&mut ws))
        .await
        .expect("timed out waiting for error");

    match msg {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "SESSION_CLOSED");
        }
        other => panic!("expected SESSION_CLOSED error, got: {other:?}"),
    }
}

// --------------------------------------------------------------------------
// Test 5: Successful join — verify SessionState response
// --------------------------------------------------------------------------
#[tokio::test]
async fn ws_successful_join_receives_session_state() {
    let (addr, state) = start_server().await;

    let (token, user_id, session) = owned_session(&state, "tester").await;

    let (mut ws, _) = connect_async(ws_url(&addr, &session.id))
        .await
        .expect("failed to connect");

    ws.send(session_join_msg(&session.id, &token))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), recv_json(&mut ws))
        .await
        .expect("timed out waiting for SessionState");

    match msg {
        Some(ServerMessage::SessionState {
            session: sess,
            participants,
            your_role,
            your_user_id,
        }) => {
            assert_eq!(sess.id, session.id);
            assert_eq!(your_role, Role::Owner);
            assert_eq!(your_user_id, user_id);
            assert!(
                !participants.is_empty(),
                "expected at least one participant"
            );
            assert_eq!(participants[0].user_id, user_id);
            assert_eq!(participants[0].name, "tester");
            assert_eq!(participants[0].role, Role::Owner);
        }
        other => panic!("expected SessionState, got: {other:?}"),
    }

    // Clean up: close connection so the PTY shuts down
    let _ = ws.close(None).await;
}

// --------------------------------------------------------------------------
// Scoped-guest session pinning: a guest bound to session A must not be
// allowed to open a WebSocket to session B even if they're technically a
// participant of both (which they can't normally be, but a future bug
// could put them there — the WS check is belt-and-braces on top of the
// participant row). The live prod attack is simpler: a guest's bearer
// token must not be reusable against any session other than its scope.
// --------------------------------------------------------------------------
#[tokio::test]
async fn ws_scoped_guest_rejected_from_other_session() {
    use telepair_core::permission::Role;

    let (addr, state) = start_server().await;

    // Session A and Session B exist with real owners.
    let (_owner_a_token, _owner_a_id, session_a) = owned_session(&state, "alice").await;
    let (_owner_b_token, _owner_b_id, session_b) = owned_session(&state, "bob").await;

    // Mint a guest scoped to A and force them into the participants
    // table for B as well. In production `redeem_invite` would never
    // let this happen (the cross-session check in http.rs 403s), but
    // we're asserting the WS layer is an independent line of defence
    // — if a future refactor loosens the HTTP check, the WS handshake
    // must still catch it.
    let (guest_a, guest_a_token) = state.auth.create_guest(&session_a.id).await.unwrap();
    state
        .storage
        .upsert_participant(&session_b.id, guest_a.id, Role::Viewer)
        .await
        .unwrap();

    // Guest-A tries to connect to session B's WS endpoint.
    let (mut ws, _) = connect_async(ws_url(&addr, &session_b.id))
        .await
        .expect("failed to connect");

    ws.send(session_join_msg(&session_b.id, &guest_a_token))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), recv_json(&mut ws))
        .await
        .expect("timed out waiting for error");

    match msg {
        Some(ServerMessage::Error { code, message }) => {
            assert_eq!(
                code, "NOT_PARTICIPANT",
                "scoped guest must be rejected with NOT_PARTICIPANT on cross-session WS join"
            );
            assert!(
                message.to_lowercase().contains("guest"),
                "error message should mention the guest scope, got: {message}"
            );
        }
        other => panic!("expected NOT_PARTICIPANT error, got: {other:?}"),
    }
}

// --------------------------------------------------------------------------
// Companion to the above: the same guest DOES get through for their own
// session, so the scope check isn't a blanket "all guests rejected".
// --------------------------------------------------------------------------
#[tokio::test]
async fn ws_scoped_guest_accepted_for_own_session() {
    use telepair_core::permission::Role;

    let (addr, state) = start_server().await;
    let (_owner_token, _owner_id, session) = owned_session(&state, "owner").await;

    let (guest, guest_token) = state.auth.create_guest(&session.id).await.unwrap();
    state
        .storage
        .upsert_participant(&session.id, guest.id, Role::Viewer)
        .await
        .unwrap();

    let (mut ws, _) = connect_async(ws_url(&addr, &session.id))
        .await
        .expect("failed to connect");
    ws.send(session_join_msg(&session.id, &guest_token))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), recv_json(&mut ws))
        .await
        .expect("timed out waiting for SessionState");

    match msg {
        Some(ServerMessage::SessionState { your_role, .. }) => {
            assert_eq!(
                your_role,
                Role::Viewer,
                "guest should join as their assigned role"
            );
        }
        other => panic!("expected SessionState, got: {other:?}"),
    }

    let _ = ws.close(None).await;
}

// --------------------------------------------------------------------------
// Test 6: WS disconnects after session is force-stopped via the hub
// --------------------------------------------------------------------------
#[tokio::test]
async fn ws_disconnects_after_session_stopped() {
    let (addr, state) = start_server().await;

    let (token, _user_id, session) = owned_session(&state, "owner").await;

    let (mut ws, _) = connect_async(ws_url(&addr, &session.id))
        .await
        .expect("failed to connect");

    ws.send(session_join_msg(&session.id, &token))
        .await
        .unwrap();

    // Receive SessionState
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), recv_json(&mut ws))
        .await
        .expect("timed out waiting for SessionState");
    assert!(matches!(msg, Some(ServerMessage::SessionState { .. })));

    // Stop the session via the hub
    state.hub.stop_session(&session.id).await;

    // WS should close within a reasonable time
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(Ok(msg)) = ws.next().await {
            if matches!(msg, Message::Close(_)) {
                return true;
            }
        }
        true // stream ended
    })
    .await;

    assert!(
        result.is_ok(),
        "WS connection should close after session stop"
    );
}

// Test 7: WS-phase launch failure closes the orphaned DB session row so
// the idle reaper (which only tracks hub entries) doesn't leave it
// lingering as `status=active` until the next server restart.
#[tokio::test]
async fn ws_closes_session_when_target_resolve_fails() {
    let (addr, state) = start_server().await;
    let token = state.create_test_user("solo").await;
    let user = state.auth.validate(&token).await.unwrap();

    // Bypass `POST /api/sessions` so we can plant a row whose
    // `target_name` the engine will refuse to resolve. In production
    // this happens when targets.yaml is hot-edited between session
    // creation and WS join; simulating it by hand is the same shape.
    let session = state
        .storage
        .create_session_with_owner(user.id, "ghost-target", InputMode::Serialized)
        .await
        .unwrap();

    // Mirror the production flow: `POST /api/sessions` always reserves
    // the target slot in the hub before returning 201. We bypassed the
    // HTTP layer for the row, so we have to plant the same reservation
    // by hand — otherwise the "release on failure" assertion below
    // would be a no-op (you can't release a reservation that was never
    // taken) and the regression we're guarding against would still
    // sneak past the test.
    state.hub.reserve_target(&session.id, "ghost-target").await;
    assert_eq!(
        state
            .hub
            .count_live_sessions_per_target()
            .await
            .get("ghost-target")
            .copied(),
        Some(1),
        "precondition: reserve_target must register the pending slot",
    );

    let (mut ws, _) = connect_async(ws_url(&addr, &session.id))
        .await
        .expect("failed to connect");

    ws.send(session_join_msg(&session.id, &token))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), recv_json(&mut ws))
        .await
        .expect("timed out waiting for error");

    match msg {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "TARGET_NOT_FOUND");
        }
        other => panic!("expected TARGET_NOT_FOUND error, got: {other:?}"),
    }

    // The whole point of the fix: the DB row must be `Closed`, not
    // lingering as `Active`. Drain any pending close frame first so the
    // server-side `cleanup_orphan_session` definitely committed.
    let _ = ws.close(None).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let fetched = state
        .storage
        .get_session(&session.id)
        .await
        .unwrap()
        .expect("session row must still exist");
    assert_eq!(
        fetched.status,
        telepair_core::session::SessionStatus::Closed,
        "failed WS launch must close the DB session row so it doesn't zombie",
    );
    assert!(
        fetched.closed_at.is_some(),
        "closed_at must be stamped by cleanup_orphan_session",
    );

    // Reservation must also be released so a subsequent
    // `/api/admin/targets/reload` doesn't see a phantom `Pending`
    // session pinning the now-deleted target. Before the fix this
    // assertion would still report the slot as in-use until the
    // pending TTL elapsed, blocking admin reload on a corpse.
    let counts = state.hub.count_live_sessions_per_target().await;
    assert!(
        !counts.contains_key("ghost-target"),
        "cleanup_orphan_session must release the hub reservation, \
         got lingering counts: {counts:?}",
    );
}

// --------------------------------------------------------------------------
// Joining a session id that does not exist on disk must surface as
// SESSION_NOT_FOUND. This pins the `Ok(None)` arm of the three-way
// match in `ws::handle_socket` so a future refactor can't lump it back
// together with the storage-error arm and turn either into the wrong
// close code (terminal vs transient).
// --------------------------------------------------------------------------
#[tokio::test]
async fn ws_unknown_session_id_returns_session_not_found() {
    let (addr, state) = start_server().await;
    // Real token, real user — only the session id is bogus.
    let token = state.create_test_user("tester").await;
    let bogus_session_id = Uuid::new_v4().to_string();

    let (mut ws, _) = connect_async(ws_url(&addr, &bogus_session_id))
        .await
        .expect("failed to connect");

    ws.send(session_join_msg(&bogus_session_id, &token))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(3), recv_json(&mut ws))
        .await
        .expect("timed out waiting for error");

    match msg {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(
                code, "SESSION_NOT_FOUND",
                "an unknown session id must surface as SESSION_NOT_FOUND, not STORAGE_ERROR",
            );
        }
        other => panic!("expected SESSION_NOT_FOUND error, got: {other:?}"),
    }
}
