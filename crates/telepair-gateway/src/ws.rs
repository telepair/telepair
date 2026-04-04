use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};

use telepair_core::permission::Role;
use telepair_core::protocol::{ClientMessage, ServerMessage};
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

async fn handle_socket(socket: WebSocket, session_id: String, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Wait for SessionJoin message with auth token
    let user = match ws_rx.next().await {
        Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMessage>(&text) {
            Ok(ClientMessage::SessionJoin { token, .. }) => {
                match state.auth.validate(&token).await {
                    Ok(user) => user,
                    Err(_) => {
                        let err = ServerMessage::Error {
                            code: "AUTH_FAILED".into(),
                            message: "invalid token".into(),
                        };
                        let _ = ws_tx
                            .send(Message::Text(serde_json::to_string(&err).unwrap().into()))
                            .await;
                        return;
                    }
                }
            }
            _ => return,
        },
        _ => return,
    };

    // Check if session exists in DB
    let session = match state.sessions.storage().get_session(&session_id).await {
        Ok(Some(s)) => s,
        _ => {
            let err = ServerMessage::Error {
                code: "SESSION_NOT_FOUND".into(),
                message: format!("session {session_id} not found"),
            };
            let _ = ws_tx
                .send(Message::Text(serde_json::to_string(&err).unwrap().into()))
                .await;
            return;
        }
    };

    // Start or join the live PTY session
    let hub = &state.hub;
    let (cmd_tx, mut output_rx) = if hub.is_live(&session_id).await {
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

    // Send session state
    let state_msg = ServerMessage::SessionState {
        session: session.clone(),
        participants: vec![],
        your_role: Role::Owner,
    };
    let _ = ws_tx
        .send(Message::Text(
            serde_json::to_string(&state_msg).unwrap().into(),
        ))
        .await;

    // Spawn output forwarder: PTY output -> WebSocket
    let output_handle = tokio::spawn(async move {
        while let Ok(data) = output_rx.recv().await {
            let msg = ServerMessage::TermOutput { data };
            let json = serde_json::to_string(&msg).unwrap();
            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Input loop: WebSocket -> PTY
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                    match client_msg {
                        ClientMessage::TermInput { data } => {
                            let _ = cmd_tx.send(PtyCommand::Input(data)).await;
                        }
                        ClientMessage::TermResize { cols, rows } => {
                            let _ = cmd_tx.send(PtyCommand::Resize(cols, rows)).await;
                        }
                        _ => {}
                    }
                }
            }
            Message::Binary(data) => {
                // Binary frame: direct PTY input
                let _ = cmd_tx.send(PtyCommand::Input(data.to_vec())).await;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    output_handle.abort();
    tracing::info!(user = %user.name, session = %session_id, "WebSocket disconnected");
}
