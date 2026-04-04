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

    // 1. Auth: wait for SessionJoin message with auth token
    let user = match ws_rx.next().await {
        Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMessage>(&text) {
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
        },
        _ => return,
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

    // 4. Start or join the live PTY session
    let hub = &state.hub;
    let (cmd_tx, mut output_rx, mut collab_rx) = if hub.is_live(&session_id).await {
        match hub.join_session(&session_id).await {
            Some(channels) => channels,
            None => return,
        }
    } else {
        // Resolve target and spawn PTY
        let (cmd, args) = match state.targets.resolve(&session.target_name) {
            Some(resolved) => resolved,
            None => return,
        };
        match hub.start_session(&session_id, &cmd, &args, 80, 24).await {
            Ok(channels) => channels,
            Err(_) => return,
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
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();

    let output_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = output_rx.recv() => {
                    match result {
                        Ok(data) => {
                            let msg = ServerMessage::TermOutput { data };
                            let json = serde_json::to_string(&msg).unwrap();
                            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                result = collab_rx.recv() => {
                    match result {
                        Ok(collab_msg) => {
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

    // 8. Input loop with permission enforcement
    let user_id = user.id;
    let user_name = user.name.clone();

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                    match client_msg {
                        ClientMessage::TermInput { data } => {
                            if my_role.can_input() {
                                let _ = cmd_tx.send(PtyCommand::Input(data)).await;
                            }
                            // Silently drop if viewer
                        }
                        ClientMessage::TermResize { cols, rows } => {
                            if my_role.can_resize() {
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
                if my_role.can_input() {
                    let _ = cmd_tx.send(PtyCommand::Input(data.to_vec())).await;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // 9. Cleanup
    let _ = stop_tx.send(());
    output_handle.abort();
    hub.remove_participant(&session_id, user_id).await;
    tracing::info!(user = %user_name, session = %session_id, "WebSocket disconnected");
}
