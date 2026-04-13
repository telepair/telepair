#![deny(unsafe_code)]
//! End-to-end tests: real Axum server + real PTY + multi-user WebSocket flows.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use telepair_core::permission::Role;
use telepair_core::protocol::ServerMessage;
use telepair_core::session::{CloseReason, InputMode, SessionStatus};
use telepair_core::storage::Storage;
use telepair_gateway::session_hub::ReaperConfig;
use telepair_gateway::state::AppState;

// ─── Helpers ─────────────────────────────────────────────

async fn start_server() -> (String, AppState) {
    let state = AppState::new_test().await;
    let router = telepair_gateway::build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
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

fn term_input_msg(data: &str) -> Message {
    // Terminal keystrokes flow as raw binary WebSocket frames — see
    // `crates/telepair-core/src/protocol.rs` and `ws.rs::handle_socket`.
    Message::Binary(data.as_bytes().to_vec().into())
}

fn term_resize_msg(cols: u16, rows: u16) -> Message {
    Message::Text(
        serde_json::json!({
            "type": "TermResize",
            "cols": cols,
            "rows": rows
        })
        .to_string()
        .into(),
    )
}

fn chat_msg(text: &str) -> Message {
    Message::Text(
        serde_json::json!({
            "type": "ChatMessage",
            "text": text
        })
        .to_string()
        .into(),
    )
}

fn cursor_msg(x: u16, y: u16) -> Message {
    Message::Text(
        serde_json::json!({
            "type": "CursorMove",
            "x": x,
            "y": y
        })
        .to_string()
        .into(),
    )
}

fn parse_server_msg(msg: &Message) -> Option<ServerMessage> {
    match msg {
        Message::Text(text) => serde_json::from_str::<ServerMessage>(text).ok(),
        _ => None,
    }
}

/// Read JSON frames (skipping binary) until predicate matches, or panic on timeout.
async fn expect_json<S, F>(rx: &mut S, secs: u64, pred: F) -> ServerMessage
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    F: Fn(&ServerMessage) -> bool,
{
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            match rx.next().await {
                Some(Ok(ref msg)) => {
                    if let Some(sm) = parse_server_msg(msg)
                        && pred(&sm)
                    {
                        return sm;
                    }
                    if matches!(msg, Message::Close(_)) {
                        panic!("connection closed while waiting for JSON message");
                    }
                }
                _ => panic!("stream ended while waiting for JSON message"),
            }
        }
    })
    .await
    .expect("timed out waiting for expected JSON message")
}

/// Accumulate binary frames (skipping text) until buffer contains needle.
async fn expect_binary_containing<S>(rx: &mut S, needle: &[u8], secs: u64) -> Vec<u8>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(secs), async {
        while let Some(Ok(msg)) = rx.next().await {
            if let Message::Binary(data) = msg {
                buf.extend_from_slice(&data);
                if buf.windows(needle.len()).any(|w| w == needle) {
                    return buf;
                }
            }
        }
        buf
    })
    .await
    .expect("timed out waiting for binary output")
}

/// Wait for WS to close (Close frame or stream end).
async fn wait_for_close<S>(rx: &mut S, secs: u64) -> bool
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(secs), async {
        while let Some(Ok(msg)) = rx.next().await {
            if matches!(msg, Message::Close(_)) {
                return true;
            }
        }
        true // stream ended = connection closed
    })
    .await
    .unwrap_or(false)
}

/// Common setup: create a user and a session they own. The owner
/// participant row is inserted atomically by the storage layer.
async fn create_owned_session(state: &AppState, username: &str) -> (String, String, uuid::Uuid) {
    let token = state.create_test_user(username).await;
    let user = state.auth.validate(&token).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    (session.id, token, user.id)
}

/// Add a user as participant with the given role. Returns (token, user_id).
async fn add_participant(
    state: &AppState,
    session_id: &str,
    username: &str,
    role: Role,
) -> (String, uuid::Uuid) {
    let token = state.create_test_user(username).await;
    let user = state.auth.validate(&token).await.unwrap();
    state
        .storage
        .upsert_participant(session_id, user.id, role)
        .await
        .unwrap();
    (token, user.id)
}

/// Connect to WS, send SessionJoin, wait for SessionState.
async fn join_session(
    addr: &str,
    session_id: &str,
    token: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (mut ws, _) = connect_async(ws_url(addr, session_id)).await.unwrap();
    ws.send(session_join_msg(session_id, token)).await.unwrap();
    expect_json(&mut ws, 5, |m| {
        matches!(m, ServerMessage::SessionState { .. })
    })
    .await;
    ws
}

// ─── Scenario 1: Full collaboration flow ─────────────────

#[tokio::test]
async fn e2e_full_collaboration_flow() {
    let (addr, state) = start_server().await;

    let (session_id, owner_token, _) = create_owned_session(&state, "alice").await;
    let (op_token, op_id) = add_participant(&state, &session_id, "bob", Role::Operator).await;

    // Owner connects and joins
    let mut ws_owner = join_session(&addr, &session_id, &owner_token).await;

    // Operator connects and joins
    let (mut ws_op, _) = connect_async(ws_url(&addr, &session_id)).await.unwrap();
    ws_op
        .send(session_join_msg(&session_id, &op_token))
        .await
        .unwrap();

    // Operator receives SessionState with both participants
    let op_state = expect_json(&mut ws_op, 5, |m| {
        matches!(m, ServerMessage::SessionState { .. })
    })
    .await;
    if let ServerMessage::SessionState {
        participants,
        your_role,
        your_user_id,
        ..
    } = op_state
    {
        assert_eq!(your_role, Role::Operator);
        assert_eq!(your_user_id, op_id);
        assert!(
            participants.len() >= 2,
            "expected both participants, got {}",
            participants.len()
        );
    }

    // Owner receives PeerJoined specifically for operator
    // (skip any PeerJoined for the owner themselves)
    let peer = expect_json(&mut ws_owner, 5, |m| match m {
        ServerMessage::PeerJoined { user_id, .. } => *user_id == op_id,
        _ => false,
    })
    .await;
    if let ServerMessage::PeerJoined { name, role, .. } = peer {
        assert_eq!(name, "bob");
        assert_eq!(role, Role::Operator);
    }

    let _ = ws_owner.close(None).await;
    let _ = ws_op.close(None).await;
}

// ─── Scenario 2: Chat broadcast ─────────────────────────

#[tokio::test]
async fn e2e_chat_broadcast() {
    let (addr, state) = start_server().await;

    let (session_id, token_a, user_a_id) = create_owned_session(&state, "alice").await;
    let (token_b, _) = add_participant(&state, &session_id, "bob", Role::Operator).await;

    let mut ws_a = join_session(&addr, &session_id, &token_a).await;
    let mut ws_b = join_session(&addr, &session_id, &token_b).await;

    // Let both connections stabilize
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Alice sends chat
    ws_a.send(chat_msg("hello from alice")).await.unwrap();

    // Bob receives PeerChat
    let chat = expect_json(&mut ws_b, 5, |m| {
        matches!(m, ServerMessage::PeerChat { .. })
    })
    .await;
    if let ServerMessage::PeerChat {
        user_id,
        name,
        text,
        ts,
    } = chat
    {
        assert_eq!(user_id, user_a_id);
        assert_eq!(name, "alice");
        assert_eq!(text, "hello from alice");
        assert!(!ts.is_empty(), "timestamp should be present");
    }

    let _ = ws_a.close(None).await;
    let _ = ws_b.close(None).await;
}

// ─── Scenario 3: PTY I/O roundtrip ──────────────────────

#[tokio::test]
async fn e2e_pty_io_roundtrip() {
    let (addr, state) = start_server().await;

    let (session_id, token, _) = create_owned_session(&state, "tester").await;
    let mut ws = join_session(&addr, &session_id, &token).await;

    // Wait for shell prompt
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Send a command with a unique marker
    ws.send(term_input_msg("echo E2E_PTY_MARKER_42\n"))
        .await
        .unwrap();

    // Read PTY output until marker appears
    let output = expect_binary_containing(&mut ws, b"E2E_PTY_MARKER_42", 10).await;
    assert!(
        output
            .windows(b"E2E_PTY_MARKER_42".len())
            .any(|w| w == b"E2E_PTY_MARKER_42"),
        "PTY output should contain the echoed marker"
    );

    let _ = ws.close(None).await;
}

// ─── Scenario 4: Permission enforcement ──────────────────

#[tokio::test]
async fn e2e_permission_enforcement() {
    let (addr, state) = start_server().await;

    let (session_id, owner_token, _) = create_owned_session(&state, "owner").await;
    let (op_token, _) = add_participant(&state, &session_id, "operator", Role::Operator).await;
    let (viewer_token, _) = add_participant(&state, &session_id, "viewer", Role::Viewer).await;

    let mut ws_owner = join_session(&addr, &session_id, &owner_token).await;
    let mut ws_op = join_session(&addr, &session_id, &op_token).await;
    let mut ws_viewer = join_session(&addr, &session_id, &viewer_token).await;

    // Wait for shell ready
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Viewer sends input → dropped, and the connection gets a one-shot
    // `InputDenied` notice with reason `VIEWER`.
    ws_viewer
        .send(term_input_msg("echo VIEWER_MARKER\n"))
        .await
        .unwrap();

    // Operator sends input in serialized mode → dropped, and the
    // connection gets a one-shot `InputDenied` notice with reason
    // `SERIALIZED_NOT_OWNER`.
    ws_op
        .send(term_input_msg("echo OPERATOR_MARKER\n"))
        .await
        .unwrap();

    // Assert the denial notices actually arrived before checking that
    // the keystrokes never reached the PTY.
    let viewer_notice = expect_json(&mut ws_viewer, 5, |m| {
        matches!(m, ServerMessage::InputDenied { .. })
    })
    .await;
    if let ServerMessage::InputDenied { reason } = viewer_notice {
        assert_eq!(reason, "VIEWER");
    }
    let op_notice = expect_json(&mut ws_op, 5, |m| {
        matches!(m, ServerMessage::InputDenied { .. })
    })
    .await;
    if let ServerMessage::InputDenied { reason } = op_notice {
        assert_eq!(reason, "SERIALIZED_NOT_OWNER");
    }

    // Owner sends input → should succeed
    ws_owner
        .send(term_input_msg("echo OWNER_MARKER_E2E\n"))
        .await
        .unwrap();

    // Read output from owner's stream until we see the owner's marker
    let output = expect_binary_containing(&mut ws_owner, b"OWNER_MARKER_E2E", 10).await;
    let output_str = String::from_utf8_lossy(&output);

    assert!(
        !output_str.contains("VIEWER_MARKER"),
        "viewer's input should have been dropped before reaching the PTY"
    );
    assert!(
        !output_str.contains("OPERATOR_MARKER"),
        "operator's input should be dropped in serialized mode"
    );

    let _ = ws_owner.close(None).await;
    let _ = ws_op.close(None).await;
    let _ = ws_viewer.close(None).await;
}

// ─── Scenario 4b: Multiplexed mode lets operators type ───

#[tokio::test]
async fn e2e_multiplexed_operator_can_type() {
    let (addr, state) = start_server().await;

    // Explicitly create a Multiplexed session: the new default for
    // sessions minted via the REST API, and the only mode in which
    // operator keystrokes should reach the PTY.
    let owner_token = state.create_test_user("alice").await;
    let owner = state.auth.validate(&owner_token).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(owner.id, "local-shell", InputMode::Multiplexed, None)
        .await
        .unwrap();
    let session_id = session.id.clone();

    let (op_token, _) = add_participant(&state, &session_id, "bob", Role::Operator).await;

    let mut ws_owner = join_session(&addr, &session_id, &owner_token).await;
    let mut ws_op = join_session(&addr, &session_id, &op_token).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    ws_op
        .send(term_input_msg("echo MUX_OPERATOR_MARKER\n"))
        .await
        .unwrap();

    // Operator input should reach the PTY and be echoed back to *any*
    // subscriber — read from the owner's stream to avoid intermixing
    // with the operator's own local echo.
    let output = expect_binary_containing(&mut ws_owner, b"MUX_OPERATOR_MARKER", 10).await;
    assert!(
        output
            .windows(b"MUX_OPERATOR_MARKER".len())
            .any(|w| w == b"MUX_OPERATOR_MARKER"),
        "operator input must flow through in multiplexed mode"
    );

    let _ = ws_owner.close(None).await;
    let _ = ws_op.close(None).await;
}

// ─── Scenario 4c: Scrollback replay for late joiners ────

#[tokio::test]
async fn e2e_late_joiner_receives_scrollback() {
    let (addr, state) = start_server().await;

    // Owner starts a Multiplexed session so the operator can type later,
    // though the operator only joins *after* the scrollback event lands.
    let owner_token = state.create_test_user("alice").await;
    let owner = state.auth.validate(&owner_token).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(owner.id, "local-shell", InputMode::Multiplexed, None)
        .await
        .unwrap();
    let session_id = session.id.clone();

    // Owner joins, types a unique marker, waits for the echo to reach
    // the PTY (and therefore the scrollback ring). We intentionally do
    // NOT assert anything on the owner's own stream here — the point of
    // this test is that a *second* connection, joining later, sees the
    // replayed bytes even though no new PTY output happens afterwards.
    let mut ws_owner = join_session(&addr, &session_id, &owner_token).await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    ws_owner
        .send(term_input_msg("echo SCROLLBACK_REPLAY_TEST\n"))
        .await
        .unwrap();
    // Drain until the marker is visible on the owner's own stream,
    // which proves it made it into the scrollback ring.
    let _ = expect_binary_containing(&mut ws_owner, b"SCROLLBACK_REPLAY_TEST", 10).await;

    // Now a late operator joins. They must see the marker in the
    // replay stream — no fresh PTY writes happen between the owner's
    // echo and the operator's join.
    let (op_token, _) = add_participant(&state, &session_id, "bob", Role::Operator).await;
    let mut ws_op = join_session(&addr, &session_id, &op_token).await;

    let replayed = expect_binary_containing(&mut ws_op, b"SCROLLBACK_REPLAY_TEST", 5).await;
    assert!(
        replayed
            .windows(b"SCROLLBACK_REPLAY_TEST".len())
            .any(|w| w == b"SCROLLBACK_REPLAY_TEST"),
        "late joiner must receive recent PTY output replayed from scrollback"
    );

    let _ = ws_owner.close(None).await;
    let _ = ws_op.close(None).await;
}

// ─── Scenario 5: Session close disconnects all ──────────

#[tokio::test]
async fn e2e_session_close_disconnects_all() {
    let (addr, state) = start_server().await;

    let (session_id, owner_token, _) = create_owned_session(&state, "alice").await;
    let (op_token, _) = add_participant(&state, &session_id, "bob", Role::Operator).await;

    let mut ws_owner = join_session(&addr, &session_id, &owner_token).await;
    let mut ws_op = join_session(&addr, &session_id, &op_token).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Close the session (same path as DELETE /api/sessions/{id})
    state
        .sessions
        .close_session(&session_id, CloseReason::Owner, None)
        .await
        .unwrap();
    state.hub.stop_session(&session_id).await;

    // Both connections should close
    assert!(
        wait_for_close(&mut ws_owner, 5).await,
        "owner WS should close after session stop"
    );
    assert!(
        wait_for_close(&mut ws_op, 5).await,
        "operator WS should close after session stop"
    );
}

// ─── Scenario 6: Oversized chat is dropped ──────────────

#[tokio::test]
async fn e2e_oversized_chat_dropped() {
    let (addr, state) = start_server().await;

    let (session_id, token_a, _) = create_owned_session(&state, "alice").await;
    let (token_b, _) = add_participant(&state, &session_id, "bob", Role::Operator).await;

    let mut ws_a = join_session(&addr, &session_id, &token_a).await;
    let mut ws_b = join_session(&addr, &session_id, &token_b).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Server cap is MAX_CHAT_BYTES = 4 KiB. 8 KiB should be rejected.
    let huge = "x".repeat(8 * 1024);
    ws_a.send(chat_msg(&huge)).await.unwrap();

    // Follow up with a small chat so we have a positive anchor to wait for.
    ws_a.send(chat_msg("short one")).await.unwrap();

    let received = expect_json(&mut ws_b, 5, |m| {
        matches!(m, ServerMessage::PeerChat { .. })
    })
    .await;
    if let ServerMessage::PeerChat { text, .. } = received {
        assert_eq!(
            text, "short one",
            "oversized chat should be dropped before the short one"
        );
    }

    let _ = ws_a.close(None).await;
    let _ = ws_b.close(None).await;
}

// ─── Scenario 7: Cursor move rate limited ───────────────

#[tokio::test]
async fn e2e_cursor_move_rate_limited() {
    let (addr, state) = start_server().await;

    let (session_id, token_a, _) = create_owned_session(&state, "alice").await;
    let (token_b, _) = add_participant(&state, &session_id, "bob", Role::Operator).await;

    let mut ws_a = join_session(&addr, &session_id, &token_a).await;
    let mut ws_b = join_session(&addr, &session_id, &token_b).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Blast 20 cursor moves with no delay. With a 33 ms throttle, at most 1
    // should be broadcast.
    for i in 0..20_u16 {
        ws_a.send(cursor_msg(i, i)).await.unwrap();
    }

    // Anchor: send a chat message AFTER the cursor flood so we have a
    // guaranteed-delivered signal to stop reading.
    tokio::time::sleep(Duration::from_millis(50)).await;
    ws_a.send(chat_msg("done")).await.unwrap();

    let mut cursor_count = 0_usize;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws_b.next().await {
                Some(Ok(ref msg)) => {
                    if let Some(sm) = parse_server_msg(msg) {
                        match sm {
                            ServerMessage::PeerCursor { .. } => cursor_count += 1,
                            ServerMessage::PeerChat { text, .. } if text == "done" => return,
                            _ => {}
                        }
                    }
                }
                _ => return,
            }
        }
    })
    .await
    .expect("timed out waiting for anchor chat");

    assert!(
        cursor_count <= 2,
        "expected at most 2 cursor broadcasts from a 20-move flood, got {cursor_count}"
    );

    let _ = ws_a.close(None).await;
    let _ = ws_b.close(None).await;
}

// ─── Scenario 8: Multi-tab participant refcount ─────────

#[tokio::test]
async fn e2e_multi_tab_keeps_participant_alive() {
    let (addr, state) = start_server().await;

    // Alice owns the session; Bob is an operator observer used to count
    // PeerJoined / PeerLeft events about Alice.
    let (session_id, alice_token, alice_id) = create_owned_session(&state, "alice").await;
    let (bob_token, _) = add_participant(&state, &session_id, "bob", Role::Operator).await;

    // Tab 1: alice joins first so Bob's SessionState already includes her.
    let mut alice_ws1 = join_session(&addr, &session_id, &alice_token).await;
    let mut bob_ws = join_session(&addr, &session_id, &bob_token).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Tab 2: second alice connection. Under the refcount fix this must NOT
    // broadcast a second PeerJoined for alice — the participant record and
    // color were set when Tab 1 joined.
    let mut alice_ws2 = join_session(&addr, &session_id, &alice_token).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Close Tab 1. Bob must NOT see a PeerLeft for alice: Tab 2 is still
    // open, so alice is still present from any observer's point of view.
    let _ = alice_ws1.close(None).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Anchor: send a chat from Tab 2 so Bob has a guaranteed event to wait
    // on. Receiving this proves alice's participant record is still wired
    // into the collab broadcast after Tab 1 closed.
    alice_ws2.send(chat_msg("still-here")).await.unwrap();

    // Scan everything Bob has seen until the anchor chat arrives, then
    // assert no PeerLeft / spurious PeerJoined for alice slipped through.
    let mut saw_peer_left_alice = false;
    let mut extra_peer_joined_alice = 0_usize;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match bob_ws.next().await {
                Some(Ok(ref msg)) => {
                    if let Some(sm) = parse_server_msg(msg) {
                        match sm {
                            ServerMessage::PeerLeft { user_id } if user_id == alice_id => {
                                saw_peer_left_alice = true;
                            }
                            ServerMessage::PeerJoined { user_id, .. } if user_id == alice_id => {
                                extra_peer_joined_alice += 1;
                            }
                            ServerMessage::PeerChat { text, .. } if text == "still-here" => {
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                _ => return,
            }
        }
    })
    .await
    .expect("timed out waiting for anchor chat from alice tab 2");

    assert!(
        !saw_peer_left_alice,
        "closing one of alice's tabs must not broadcast PeerLeft while another tab is open"
    );
    assert_eq!(
        extra_peer_joined_alice, 0,
        "alice's second tab must not broadcast a second PeerJoined"
    );

    // Now close Tab 2 — this is the final connection, so Bob should finally
    // see PeerLeft for alice.
    let _ = alice_ws2.close(None).await;

    let left = expect_json(
        &mut bob_ws,
        5,
        |m| matches!(m, ServerMessage::PeerLeft { user_id, .. } if *user_id == alice_id),
    )
    .await;
    assert!(matches!(left, ServerMessage::PeerLeft { .. }));

    let _ = bob_ws.close(None).await;
}

// ─── Scenario 9: Resize accepted ────────────────────────

#[tokio::test]
async fn e2e_resize_accepted() {
    let (addr, state) = start_server().await;

    let (session_id, token, _) = create_owned_session(&state, "tester").await;
    let mut ws = join_session(&addr, &session_id, &token).await;

    // Wait for shell ready
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Resize the terminal
    ws.send(term_resize_msg(120, 40)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify PTY still works after resize
    ws.send(term_input_msg("echo RESIZE_OK_E2E\n"))
        .await
        .unwrap();

    let output = expect_binary_containing(&mut ws, b"RESIZE_OK_E2E", 10).await;
    assert!(
        output
            .windows(b"RESIZE_OK_E2E".len())
            .any(|w| w == b"RESIZE_OK_E2E"),
        "should still receive PTY output after resize"
    );

    let _ = ws.close(None).await;
}

// ─── Scenario 10: Idle session reaper ────────────────────

/// When every WebSocket client disconnects, the hub's cmd_tx clone keeps the
/// PTY I/O loop alive forever — there's nothing to push `cmd_rx.recv()` to
/// `None`. The reaper fixes that: it sweeps sessions whose `idle_since` has
/// elapsed beyond the configured timeout, drops them from the map (which
/// drops the last cmd_tx, which lets the PTY loop exit cleanly), and marks
/// the row closed in the DB.
///
/// This test runs the reaper with a 200 ms idle grace and a 100 ms check
/// interval. A real-world deployment uses 120 s / 30 s.
#[tokio::test]
async fn e2e_reaper_kills_idle_session() {
    let (addr, state) = start_server().await;

    // `new_test` intentionally skips the reaper so most tests don't race
    // against it. Spawn our own with a fast config — we want to prove the
    // reaper kicks in within a test-scale timeout.
    let _reaper = state.hub.spawn_reaper(ReaperConfig {
        idle_timeout: Duration::from_millis(200),
        check_interval: Duration::from_millis(100),
    });

    let (session_id, token, _) = create_owned_session(&state, "alice").await;

    // Connect, let the handshake settle, then immediately close.
    let mut ws = join_session(&addr, &session_id, &token).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = ws.close(None).await;

    // After close, handle_socket needs to flush output, call
    // `remove_participant` (which sets idle_since), then the reaper needs
    // to tick twice past the 200 ms grace. 800 ms gives comfortable slack.
    tokio::time::sleep(Duration::from_millis(800)).await;

    // The row should now be marked `closed` in the DB.
    let reloaded = state
        .storage
        .get_session(&session_id)
        .await
        .expect("storage get_session failed")
        .expect("session row should still exist after reaping (just closed)");
    assert_eq!(
        reloaded.status,
        SessionStatus::Closed,
        "reaper should have closed the idle session in the DB"
    );

    // Reconnecting must fail with SESSION_CLOSED because the row is closed
    // and the live entry is gone from the hub map.
    let (mut ws_again, _) = connect_async(ws_url(&addr, &session_id)).await.unwrap();
    ws_again
        .send(session_join_msg(&session_id, &token))
        .await
        .unwrap();
    let err = expect_json(&mut ws_again, 5, |m| {
        matches!(m, ServerMessage::Error { .. })
    })
    .await;
    if let ServerMessage::Error { code, .. } = err {
        assert_eq!(
            code, "SESSION_CLOSED",
            "reconnect after reap should report SESSION_CLOSED"
        );
    }
}

/// Regression for the invitee-reconnect bug: a non-owner participant
/// whose WebSocket drops (e.g. a wifi hiccup) must be able to reconnect
/// and be re-admitted, as long as the session itself is still alive.
///
/// Before the fix, `ws.rs::handle_socket` eagerly called
/// `storage.remove_participant` on socket close, stamping `left_at`
/// immediately. The session_hub's 2-minute reaper grace kept the
/// in-memory `LiveSession` alive (so client-side auto-retry kicked
/// in), but the DB participant row was already marked gone, so the
/// next handshake hit the `NOT_PARTICIPANT` branch and close-4001'd.
/// Invitees could only ever join once per socket lifetime.
///
/// The fix moves `left_at` writes into `close_session` / the reaper,
/// which runs only when the session actually dies. Socket close now
/// only touches in-memory refcounts.
///
/// The owner path was already immune because `ws.rs` short-circuits
/// on `is_owner` before checking participant rows, which is why the
/// existing `e2e_reaper_skips_reconnected_session` test didn't catch
/// this — it used an owner token. This test specifically exercises
/// an invitee.
#[tokio::test]
async fn e2e_invitee_reconnects_after_transient_disconnect() {
    let (addr, state) = start_server().await;

    let (session_id, owner_token, _) = create_owned_session(&state, "alice").await;
    let (invitee_token, invitee_id) =
        add_participant(&state, &session_id, "bob", Role::Operator).await;

    // Keep an owner connection open throughout so the session stays
    // live and we don't accidentally test reaper behaviour instead.
    let mut _ws_owner = join_session(&addr, &session_id, &owner_token).await;

    // First join by the invitee (this is the first "tab" in the
    // browser, mirroring the normal invite redeem → session flow).
    let mut ws_invitee_1 = join_session(&addr, &session_id, &invitee_token).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Simulate a transient disconnect — exactly what the frontend
    // auto-retry logic sees when a user's wifi blips.
    let _ = ws_invitee_1.close(None).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // DB state check: the invitee's row must still be active. If the
    // old eager-remove logic ever sneaks back in, this fires.
    let participants_before_rejoin = state.storage.list_participants(&session_id).await.unwrap();
    assert!(
        participants_before_rejoin
            .iter()
            .any(|p| p.user_id == invitee_id),
        "invitee participant row must stay active across a socket close (list_participants filters on left_at IS NULL)"
    );

    // Reconnect — this is the moment the bug used to surface. With
    // the fix, we complete the handshake and get SessionState. We
    // can't use join_session() here because it swallows the
    // SessionState frame and we need to inspect `your_role`.
    let (mut ws_invitee_2, _) = connect_async(ws_url(&addr, &session_id)).await.unwrap();
    ws_invitee_2
        .send(session_join_msg(&session_id, &invitee_token))
        .await
        .unwrap();

    // And we must be able to act with our original role, not get
    // downgraded to Viewer by the fallback in ws.rs:156.
    let state_msg = expect_json(&mut ws_invitee_2, 5, |m| {
        matches!(m, ServerMessage::SessionState { .. })
    })
    .await;
    if let ServerMessage::SessionState { your_role, .. } = state_msg {
        assert_eq!(
            your_role,
            Role::Operator,
            "invitee should keep their Operator role on reconnect"
        );
    }

    let _ = ws_invitee_2.close(None).await;
}

/// Companion test: once the session is actually closed (by the owner
/// or the reaper), all participants' `left_at` columns must be
/// settled. This is the other half of the fix — we moved the cleanup
/// write from the WS handler into `close_session`, and we need to
/// prove that path actually runs.
#[tokio::test]
async fn e2e_close_session_settles_all_participants() {
    let (_addr, state) = start_server().await;

    let (session_id, _owner_token, owner_id) = create_owned_session(&state, "alice").await;
    let (_bob_token, bob_id) = add_participant(&state, &session_id, "bob", Role::Operator).await;
    let (_carol_token, carol_id) =
        add_participant(&state, &session_id, "carol", Role::Viewer).await;

    // Sanity: all three are active before close.
    let before = state.storage.list_participants(&session_id).await.unwrap();
    assert_eq!(before.len(), 3);

    // Close the session via the same storage path DELETE /api/sessions
    // and the reaper both use.
    state
        .storage
        .close_session(&session_id, CloseReason::Owner)
        .await
        .unwrap();

    // After close, list_participants (which filters on left_at IS NULL)
    // should return zero rows for this session. left_at was written
    // atomically with the sessions status update.
    let after = state.storage.list_participants(&session_id).await.unwrap();
    assert!(
        after.is_empty(),
        "close_session should settle left_at for every active participant; still active: {:?}",
        after
            .iter()
            .map(|p| (p.user_id, p.role))
            .collect::<Vec<_>>()
    );

    // Silence unused warnings on the user ids — keeping them in the
    // test signature makes the intent clear even though we don't
    // inspect them individually.
    let _ = (owner_id, bob_id, carol_id);
}

/// Inverse of the reaper test: a reconnect inside the grace window must
/// cancel the pending reap. This verifies that `add_participant` correctly
/// clears `idle_since` and that the write-lock re-check in the reaper
/// honours the fresh value instead of blindly trusting the read-lock scan.
#[tokio::test]
async fn e2e_reaper_skips_reconnected_session() {
    let (addr, state) = start_server().await;

    let _reaper = state.hub.spawn_reaper(ReaperConfig {
        idle_timeout: Duration::from_millis(300),
        check_interval: Duration::from_millis(100),
    });

    let (session_id, token, _) = create_owned_session(&state, "alice").await;

    // First connection, then close — starts the idle clock.
    let mut ws1 = join_session(&addr, &session_id, &token).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = ws1.close(None).await;

    // Reconnect well inside the grace window. This should clear idle_since
    // and prevent the reaper from collecting the session.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut ws2 = join_session(&addr, &session_id, &token).await;

    // Wait long enough that the original grace window would have expired,
    // plus a couple of reaper ticks. If the reconnect didn't clear the
    // clock, the session would be closed by now.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let reloaded = state
        .storage
        .get_session(&session_id)
        .await
        .unwrap()
        .expect("session row should still exist");
    assert_eq!(
        reloaded.status,
        SessionStatus::Active,
        "a reconnect inside the grace window must cancel the pending reap"
    );

    // And the live session is still usable — send a resize to prove the
    // PTY command channel is alive.
    ws2.send(term_resize_msg(100, 30)).await.unwrap();
    let _ = ws2.close(None).await;
}
