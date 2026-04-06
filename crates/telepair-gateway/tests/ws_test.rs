#![deny(unsafe_code)]

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use uuid::Uuid;

use telepair_core::permission::Role;
use telepair_core::protocol::ServerMessage;
use telepair_core::session::{InputMode, Session};
use telepair_core::storage::Storage;
use telepair_gateway::state::AppState;

/// Create a user, a session they own, and the owner participant row.
/// Returns `(token, user_id, session)` so tests can skip 4 lines of setup.
async fn owned_session(state: &AppState, username: &str) -> (String, Uuid, Session) {
    let token = state.create_test_user(username).await;
    let user = state.auth.validate(&token).await.unwrap();
    let session = state
        .sessions
        .storage()
        .create_session(user.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();
    state
        .sessions
        .storage()
        .upsert_participant(&session.id, user.id, Role::Owner)
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
        .sessions
        .storage()
        .close_session(&session.id)
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
