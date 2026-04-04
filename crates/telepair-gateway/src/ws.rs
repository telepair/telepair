use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
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

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, session_id, state))
}

async fn send_error(
    ws_tx: &mut futures::stream::SplitSink<WebSocket, Message>,
    code: &str,
    message: String,
) {
    let err = ServerMessage::Error {
        code: code.into(),
        message,
    };
    let _ = ws_tx
        .send(Message::Text(serde_json::to_string(&err).unwrap().into()))
        .await;
}

async fn handle_socket(socket: WebSocket, session_id: String, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // 1. Auth: wait for SessionJoin message with 5-second timeout
    let user = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        ws_rx.next(),
    )
    .await
    {
        Ok(Some(Ok(Message::Text(text)))) => {
            match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::SessionJoin { token, .. }) => {
                    match state.auth.validate(&token).await {
                        Ok(user) => user,
                        Err(_) => {
                            send_error(&mut ws_tx, "AUTH_FAILED", "invalid token".into()).await;
                            return;
                        }
                    }
                }
                _ => return,
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

    // 2. Session lookup from DB
    let session = match state.sessions.storage().get_session(&session_id).await {
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

    // 3. Role lookup from DB participants
    let db_participants = state
        .sessions
        .storage()
        .list_participants(&session_id)
        .await
        .unwrap_or_default();
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
        .unwrap_or_else(|| {
            if is_owner {
                Role::Owner
            } else {
                Role::Viewer
            }
        });

    // 4. Start or join the live PTY session (atomic — no TOCTOU race)
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
    let (cmd_tx, mut output_rx, mut collab_rx, mut shutdown_rx) =
        match hub
            .start_or_join(&session_id, &cmd, &args, &env, 80, 24)
            .await
        {
            Ok(channels) => channels,
            Err(e) => {
                send_error(&mut ws_tx, "PTY_ERROR", e).await;
                return;
            }
        };

    // 5. Register participant in the hub
    let _color = hub
        .add_participant(&session_id, user.id, user.name.clone(), my_role)
        .await;

    // 6. Build SessionState with real participant list
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
    };
    let _ = ws_tx
        .send(Message::Text(
            serde_json::to_string(&state_msg).unwrap().into(),
        ))
        .await;

    // 7. Output forwarder: PTY output + collab messages -> WebSocket
    //    Use a oneshot channel to signal stop.
    //    Use a watch channel for reactive role updates.
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let (role_watch_tx, role_watch_rx) = tokio::sync::watch::channel(my_role);

    let my_user_id = user.id;
    let output_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = output_rx.recv() => {
                    match result {
                        Ok(data) => {
                            if ws_tx.send(Message::Binary(data.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
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
                            let json = serde_json::to_string(&collab_msg).unwrap();
                            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = &mut stop_rx => {
                    break;
                }
            }
        }
    });

    // 8. Input loop with permission enforcement (reactive role via watch channel)
    let user_id = user.id;
    let user_name = user.name.clone();
    let input_mode = session.input_mode;

    loop {
        let current_role = *role_watch_rx.borrow();
        tokio::select! {
            msg = ws_rx.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    Message::Text(text) => {
                        if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                            match client_msg {
                                ClientMessage::TermInput { data } => {
                                    if current_role.can_input() {
                                        // In serialized mode, only the owner can type
                                        if input_mode == InputMode::Serialized
                                            && current_role != Role::Owner
                                        {
                                            // Drop input from non-owners in serialized mode
                                        } else {
                                            let _ = cmd_tx.send(PtyCommand::Input(data)).await;
                                        }
                                    }
                                    // Silently drop if viewer
                                }
                                ClientMessage::TermResize { cols, rows } => {
                                    if current_role.can_resize() {
                                        let _ = cmd_tx.send(PtyCommand::Resize(cols, rows)).await;
                                    }
                                }
                                ClientMessage::ChatMessage { text } => {
                                    let chat_msg = ServerMessage::PeerChat {
                                        user_id,
                                        name: user_name.clone(),
                                        text,
                                        ts: Utc::now().to_rfc3339(),
                                    };
                                    hub.broadcast_collab(&session_id, chat_msg).await;
                                }
                                ClientMessage::CursorMove { x, y } => {
                                    let cursor_msg = ServerMessage::PeerCursor { user_id, x, y };
                                    hub.broadcast_collab(&session_id, cursor_msg).await;
                                }
                                ClientMessage::SessionJoin { .. } => {
                                    // Ignore duplicate join messages
                                }
                            }
                        }
                    }
                    Message::Binary(data) => {
                        // Binary frame: direct PTY input (only if allowed)
                        if current_role.can_input() {
                            // In serialized mode, only the owner can type
                            if input_mode == InputMode::Serialized && current_role != Role::Owner {
                                // Drop input from non-owners in serialized mode
                            } else {
                                let _ = cmd_tx.send(PtyCommand::Input(data.to_vec())).await;
                            }
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

    // 9. Cleanup
    let _ = stop_tx.send(());
    output_handle.abort();
    hub.remove_participant(&session_id, user_id).await;
    tracing::info!(user = %user_name, session = %session_id, "WebSocket disconnected");
}
