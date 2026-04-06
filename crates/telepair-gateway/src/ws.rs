use std::time::{Duration, Instant};

use axum::{
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    response::IntoResponse,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use tokio::sync::oneshot;

use telepair_core::permission::Role;
use telepair_core::protocol::{ClientMessage, ParticipantInfo, ServerMessage};
use telepair_core::session::InputMode;
use telepair_core::storage::Storage;

use crate::session_hub::PtyCommand;
use crate::state::AppState;

/// Hard cap on any single WebSocket frame/message. Terminal keystrokes are
/// tiny; this limit only matters for paste buffers and guards against a
/// malicious client allocating tens of MB per frame on the server.
const MAX_WS_FRAME_BYTES: usize = 256 * 1024;

/// Maximum UTF-8 byte length of a single chat message. Anything longer is
/// dropped server-side with a warn log. Prevents a client from broadcasting
/// multi-MB strings and fanning out to every participant.
const MAX_CHAT_BYTES: usize = 4 * 1024;

/// Minimum interval between `CursorMove` broadcasts from a single connection.
/// ~30 Hz is smooth enough for collaborative cursors and throttles flood
/// attempts against the collab broadcast channel.
const CURSOR_MIN_INTERVAL: Duration = Duration::from_millis(33);

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.max_frame_size(MAX_WS_FRAME_BYTES)
        .max_message_size(MAX_WS_FRAME_BYTES)
        .on_upgrade(move |socket| handle_socket(socket, session_id, state))
}

async fn send_error(
    ws_tx: &mut futures::stream::SplitSink<WebSocket, Message>,
    code: &str,
    message: String,
) {
    let err = ServerMessage::Error {
        code: code.into(),
        message: message.clone(),
    };
    if let Ok(json) = serde_json::to_string(&err) {
        let _ = ws_tx.send(Message::Text(json.into())).await;
    }
    // Send proper close frame so frontend can distinguish permanent errors
    let _ = ws_tx
        .send(Message::Close(Some(CloseFrame {
            code: 4001,
            reason: message.into(),
        })))
        .await;
}

async fn handle_socket(socket: WebSocket, session_id: String, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let (user, initial_cols, initial_rows) =
        match tokio::time::timeout(std::time::Duration::from_secs(5), ws_rx.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::SessionJoin {
                        token, cols, rows, ..
                    }) => match state.auth.validate(&token).await {
                        Ok(user) => (user, cols, rows),
                        Err(_) => {
                            send_error(&mut ws_tx, "AUTH_FAILED", "invalid token".into()).await;
                            return;
                        }
                    },
                    _ => {
                        send_error(
                            &mut ws_tx,
                            "EXPECTED_JOIN",
                            "first message must be SessionJoin".into(),
                        )
                        .await;
                        return;
                    }
                }
            }
            _ => {
                send_error(
                    &mut ws_tx,
                    "AUTH_TIMEOUT",
                    "expected SessionJoin within 5 seconds".into(),
                )
                .await;
                return;
            }
        };

    // Run session lookup and participant listing concurrently — both depend
    // only on session_id and account for ~half the handshake DB time.
    let storage = state.sessions.storage();
    let (session_res, participants_res) = tokio::join!(
        storage.get_session(&session_id),
        storage.list_participants(&session_id),
    );

    let session = match session_res {
        Ok(Some(s)) => s,
        _ => {
            send_error(
                &mut ws_tx,
                "SESSION_NOT_FOUND",
                format!("session {session_id} not found"),
            )
            .await;
            return;
        }
    };

    if session.status == telepair_core::session::SessionStatus::Closed {
        send_error(
            &mut ws_tx,
            "SESSION_CLOSED",
            "this session has been closed".into(),
        )
        .await;
        return;
    }

    let db_participants = participants_res.unwrap_or_default();
    let is_owner = session.owner_id == user.id;
    let is_participant = db_participants.iter().any(|p| p.user_id == user.id);

    // Reject users who are neither the owner nor an existing participant
    if !is_owner && !is_participant {
        send_error(
            &mut ws_tx,
            "NOT_PARTICIPANT",
            "you are not a participant of this session".into(),
        )
        .await;
        return;
    }

    let my_role = db_participants
        .iter()
        .find(|p| p.user_id == user.id)
        .map(|p| p.role)
        .unwrap_or_else(|| if is_owner { Role::Owner } else { Role::Viewer });

    let hub = &state.hub;
    let (cmd, args, env) = match state.targets.resolve(&session.target_name) {
        Some(resolved) => resolved,
        None => {
            send_error(
                &mut ws_tx,
                "TARGET_NOT_FOUND",
                format!("target {} not found", session.target_name),
            )
            .await;
            return;
        }
    };
    let (cmd_tx, mut output_rx, mut collab_rx, mut shutdown_rx) = match hub
        .start_or_join(&session_id, &cmd, &args, &env, initial_cols, initial_rows)
        .await
    {
        Ok(channels) => channels,
        Err(e) => {
            send_error(&mut ws_tx, "PTY_ERROR", e).await;
            return;
        }
    };

    hub.add_participant(&session_id, user.id, user.name.clone(), my_role)
        .await;

    let connected = hub.get_participants(&session_id).await;
    let participant_infos: Vec<ParticipantInfo> = connected
        .iter()
        .map(|p| ParticipantInfo {
            user_id: p.user_id,
            name: p.name.clone(),
            role: p.role,
            color: p.color.clone(),
        })
        .collect();

    let state_msg = ServerMessage::SessionState {
        session: session.clone(),
        participants: participant_infos,
        your_role: my_role,
        your_user_id: user.id,
    };
    match serde_json::to_string(&state_msg) {
        Ok(json) => {
            let _ = ws_tx.send(Message::Text(json.into())).await;
        }
        Err(e) => {
            tracing::error!("failed to serialize SessionState: {e}");
            return;
        }
    }

    // Spawn a forwarder that pumps PTY output + collab messages to the WS sink.
    // `stop_tx` (oneshot) lets the main loop tell the forwarder to exit;
    // `role_watch_tx` (watch) lets it react to PermUpdate without blocking the input loop.
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let (role_watch_tx, role_watch_rx) = tokio::sync::watch::channel(my_role);

    let my_user_id = user.id;
    let session_id_for_output = session_id.clone();
    let output_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = output_rx.recv() => {
                    match result {
                        Ok(data) => {
                            // `data` is already a refcounted `Bytes`; axum's
                            // `Message::Binary` takes `Bytes` directly so this
                            // forwards without an extra allocation or copy.
                            if ws_tx.send(Message::Binary(data)).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(session = %session_id_for_output, "output receiver lagged, dropped {n} messages");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                result = collab_rx.recv() => {
                    match result {
                        Ok(collab_msg) => {
                            // Detect PermUpdate targeting current user and update role watch
                            if let ServerMessage::PermUpdate { user_id, new_role } = &collab_msg {
                                if *user_id == my_user_id {
                                    let _ = role_watch_tx.send(*new_role);
                                }
                            }
                            let Ok(json) = serde_json::to_string(&collab_msg) else {
                                tracing::error!("failed to serialize collab message");
                                continue;
                            };
                            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(session = %session_id_for_output, "collab receiver lagged, dropped {n} messages");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = &mut stop_rx => {
                    break;
                }
            }
        }
    });

    let user_id = user.id;
    let user_name = user.name.clone();
    let input_mode = session.input_mode;

    let can_forward_input = |role: Role| -> bool {
        role.can_input() && !(input_mode == InputMode::Serialized && role != Role::Owner)
    };

    // Track the last accepted CursorMove timestamp per connection so we can
    // drop floods without spinning up a timer task.
    let mut last_cursor_at: Option<Instant> = None;

    loop {
        let current_role = *role_watch_rx.borrow();
        tokio::select! {
            msg = ws_rx.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    Message::Text(text) => {
                        if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                            match client_msg {
                                ClientMessage::TermResize { cols, rows } => {
                                    if current_role.can_resize() {
                                        let _ = cmd_tx.send(PtyCommand::Resize(cols, rows)).await;
                                    }
                                }
                                ClientMessage::ChatMessage { text } => {
                                    if text.len() > MAX_CHAT_BYTES {
                                        tracing::warn!(
                                            session = %session_id,
                                            user = %user_name,
                                            len = text.len(),
                                            max = MAX_CHAT_BYTES,
                                            "dropped oversized chat message"
                                        );
                                    } else {
                                        let chat_msg = ServerMessage::PeerChat {
                                            user_id,
                                            name: user_name.clone(),
                                            text,
                                            ts: Utc::now().to_rfc3339(),
                                        };
                                        hub.broadcast_collab(&session_id, chat_msg).await;
                                    }
                                }
                                ClientMessage::CursorMove { x, y } => {
                                    let now = Instant::now();
                                    let ok = last_cursor_at
                                        .map(|prev| now.duration_since(prev) >= CURSOR_MIN_INTERVAL)
                                        .unwrap_or(true);
                                    if ok {
                                        last_cursor_at = Some(now);
                                        let cursor_msg = ServerMessage::PeerCursor { user_id, x, y };
                                        hub.broadcast_collab(&session_id, cursor_msg).await;
                                    }
                                }
                                ClientMessage::SessionJoin { .. } => {
                                    // Ignore duplicate join messages
                                }
                            }
                        }
                    }
                    Message::Binary(data) => {
                        if can_forward_input(current_role) {
                            let _ = cmd_tx.send(PtyCommand::Input(data.to_vec())).await;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!(user = %user_name, session = %session_id, "session force-stopped");
                break;
            }
        }
    }

    let _ = stop_tx.send(());
    // Give the output handler time to flush pending sends before force-aborting
    if tokio::time::timeout(std::time::Duration::from_secs(2), output_handle)
        .await
        .is_err()
    {
        tracing::warn!(session = %session_id, "output handler did not stop within 2s");
    }
    // Only update DB `left_at` when this was the user's final connection;
    // otherwise other tabs stay authoritative and the participant row must
    // remain active. `hub.remove_participant` handles the refcount.
    let was_last = hub.remove_participant(&session_id, user_id).await;
    if was_last {
        if let Err(e) = state
            .sessions
            .storage()
            .remove_participant(&session_id, user_id)
            .await
        {
            tracing::warn!(session = %session_id, user = %user_name, "failed to update DB left_at: {e}");
        }
    }
    tracing::info!(user = %user_name, session = %session_id, "WebSocket disconnected");
}
