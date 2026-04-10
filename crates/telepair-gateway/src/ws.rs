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
use tokio::sync::{mpsc, oneshot};

use telepair_core::permission::Role;
use telepair_core::protocol::{
    ClientMessage, ServerMessage, close_code_for, error_codes, input_denied,
};
use telepair_core::session::{CloseReason, InputMode};

use crate::session_hub::{PtyCommand, SessionAttachment};
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

/// Close a session row whose WS-phase launch failed so it doesn't
/// linger as "active" on the owner's dashboard with no hub entry for
/// the idle reaper to find. The history chip reads "Error" because
/// this path is exclusively about "launch broke mid-handshake" —
/// nothing the owner did.
async fn cleanup_orphan_session(state: &AppState, session_id: &str) {
    // No actor: the close is server-initiated after a mid-handshake
    // launch failure, not something the owner asked for. Audit will
    // render it as "reason=error, actor=none".
    if let Err(err) = state
        .sessions
        .close_session(session_id, CloseReason::Error, None)
        .await
    {
        tracing::error!(
            session = %session_id,
            error = %err,
            "failed to close orphan session after launch failure"
        );
    }
}

/// Build the two WebSocket frames that `send_error` will write for a
/// given protocol error: a JSON `ServerMessage::Error` text frame the
/// client's `onmessage` handler can surface, followed by a `Close`
/// frame whose code decides "retry or give up" on the client side.
///
/// Extracted from `send_error` so the close-code decision is
/// unit-testable without having to stand up a real WebSocket sink.
/// The close code comes from `close_code_for` — the protocol layer is
/// the single source of truth for terminal vs transient classification.
fn build_error_frames(code: &str, message: String) -> (Message, Message) {
    let err = ServerMessage::Error {
        code: code.into(),
        message: message.clone(),
    };
    // `serde_json::to_string` on a struct of owned `String`s never
    // fails in practice, but we fall back to an empty JSON object
    // instead of unwrapping so a hypothetical future variant with a
    // non-serializable field can't crash the gateway.
    let json = serde_json::to_string(&err).unwrap_or_else(|_| "{}".into());
    let text = Message::Text(json.into());
    let close = Message::Close(Some(CloseFrame {
        code: close_code_for(code),
        reason: message.into(),
    }));
    (text, close)
}

async fn send_error(
    ws_tx: &mut futures::stream::SplitSink<WebSocket, Message>,
    code: &str,
    message: String,
) {
    let (text, close) = build_error_frames(code, message);
    let _ = ws_tx.send(text).await;
    let _ = ws_tx.send(close).await;
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
                            send_error(
                                &mut ws_tx,
                                error_codes::AUTH_FAILED,
                                "invalid token".into(),
                            )
                            .await;
                            return;
                        }
                    },
                    _ => {
                        send_error(
                            &mut ws_tx,
                            error_codes::EXPECTED_JOIN,
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
                    error_codes::AUTH_TIMEOUT,
                    "expected SessionJoin within 5 seconds".into(),
                )
                .await;
                return;
            }
        };

    // Scoped-guest session pinning: a guest's bearer token is only
    // valid for the session it was minted for. The DB's participants
    // row + the `NOT_PARTICIPANT` check below *also* catches most
    // cross-session attempts, but this explicit comparison is the
    // guarantee — it trips before we even touch the participants
    // table and gives the client a clear refusal instead of the
    // generic "not a participant" message. Real accounts skip this
    // entirely (their `scoped_session_id` is None).
    if let Some(ref scope) = user.scoped_session_id
        && *scope != session_id
    {
        send_error(
            &mut ws_tx,
            error_codes::NOT_PARTICIPANT,
            "guest token is not valid for this session".into(),
        )
        .await;
        return;
    }

    // Run session lookup and participant listing concurrently — both depend
    // only on session_id and account for ~half the handshake DB time.
    let (session_res, participants_res) = tokio::join!(
        state.sessions.get_session(&session_id),
        state.sessions.list_participants(&session_id),
    );

    let session = match session_res {
        Ok(Some(s)) => s,
        _ => {
            send_error(
                &mut ws_tx,
                error_codes::SESSION_NOT_FOUND,
                format!("session {session_id} not found"),
            )
            .await;
            return;
        }
    };

    if session.status == telepair_core::session::SessionStatus::Closed {
        send_error(
            &mut ws_tx,
            error_codes::SESSION_CLOSED,
            "this session has been closed".into(),
        )
        .await;
        return;
    }

    // A DB failure here must not collapse into the `NOT_PARTICIPANT`
    // branch below — that would misdiagnose a storage outage as a
    // permission problem and push users into rebuilding invites.
    let db_participants = match participants_res {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(session = %session_id, error = %e, "failed to list participants");
            send_error(
                &mut ws_tx,
                error_codes::STORAGE_ERROR,
                "temporary storage failure — please retry".into(),
            )
            .await;
            return;
        }
    };
    let is_owner = session.owner_id == user.id;
    // Single scan serves both "are they allowed in?" and "what's their
    // role?" — the old code walked `db_participants` twice for the same
    // predicate.
    let me = db_participants.iter().find(|p| p.user_id == user.id);

    // Reject users who are neither the owner nor an existing participant
    if !is_owner && me.is_none() {
        send_error(
            &mut ws_tx,
            error_codes::NOT_PARTICIPANT,
            "you are not a participant of this session".into(),
        )
        .await;
        return;
    }

    let my_role = me
        .map(|p| p.role)
        .unwrap_or_else(|| if is_owner { Role::Owner } else { Role::Viewer });

    let hub = &state.hub;
    // `load()` is wait-free; `resolve` returns owned strings so the
    // guard drops before we hand the tuple to `start_or_join`.
    let (cmd, args, env) = match state.targets.load().resolve(&session.target_name) {
        Some(resolved) => resolved,
        None => {
            cleanup_orphan_session(&state, &session_id).await;
            send_error(
                &mut ws_tx,
                error_codes::TARGET_NOT_FOUND,
                format!("target {} not found", session.target_name),
            )
            .await;
            return;
        }
    };
    let SessionAttachment {
        cmd_tx,
        mut output_rx,
        mut collab_rx,
        mut shutdown_rx,
        scrollback,
    } = match hub
        .start_or_join(&session_id, &cmd, &args, &env, initial_cols, initial_rows)
        .await
    {
        Ok(attachment) => attachment,
        Err(e) => {
            cleanup_orphan_session(&state, &session_id).await;
            send_error(&mut ws_tx, error_codes::PTY_ERROR, e.to_string()).await;
            return;
        }
    };

    // `None` here means the session vanished from the hub between
    // `start_or_join` and this call — force-stop, reaper sweep during
    // the handshake gap, that kind of thing. The `shutdown_rx` we
    // already hold will deliver the tear-down signal on the next loop
    // iteration, so falling through with an empty list is safe; the
    // warn log just makes the rare case debuggable.
    let participants = hub
        .add_participant_and_snapshot(&session_id, user.id, user.name.clone(), my_role)
        .await
        .unwrap_or_else(|| {
            tracing::warn!(
                session = %session_id,
                "session vanished between start_or_join and add_participant"
            );
            Vec::new()
        });

    let state_msg = ServerMessage::SessionState {
        session: session.clone(),
        participants,
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

    // Replay scrollback BEFORE handing `ws_tx` off to the forwarder. Doing
    // this here (instead of pushing the scrollback through the forwarder's
    // personal channel) keeps the ordering airtight: the subscriber's
    // `output_rx` was constructed atomically with the snapshot, so live
    // broadcast chunks will arrive strictly after these replay frames.
    // Without scrollback, a late joiner saw a completely blank screen
    // until the owner typed something — useless for any session mid-
    // compile, mid-debugger, or mid-log-tail.
    for chunk in scrollback {
        if ws_tx.send(Message::Binary(chunk)).await.is_err() {
            // Client already bailed — nothing else to do; the main loop
            // below will exit on the next poll.
            return;
        }
    }

    // Spawn a forwarder that pumps PTY output + collab messages to the WS sink.
    // `stop_tx` (oneshot) lets the main loop tell the forwarder to exit.
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    // `personal_tx` is the main loop's private path back to the WS sink:
    // it lets us push per-connection control messages (e.g. `InputDenied`)
    // without broadcasting to every other participant. Buffer of 4 is
    // enough for notice-once latches; a full buffer is a drop signal.
    let (personal_tx, mut personal_rx) = mpsc::channel::<ServerMessage>(4);

    let session_id_for_output = session_id.clone();
    let mut output_handle = tokio::spawn(async move {
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
                // Personal control channel — used by the main loop to send
                // per-connection notices (e.g. `InputDenied`) without
                // touching the broadcast channel.
                personal = personal_rx.recv() => {
                    let Some(msg) = personal else { continue };
                    let Ok(json) = serde_json::to_string(&msg) else {
                        tracing::error!("failed to serialize personal message");
                        continue;
                    };
                    if ws_tx.send(Message::Text(json.into())).await.is_err() {
                        break;
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

    // Role is captured at join time and constant for the lifetime of this
    // WS connection. If a future role-mutation API lands, reconnect is the
    // right place to re-check (keeps auth consistent across tabs), not a
    // side-channel that races the input loop.
    let current_role = my_role;
    let can_forward_input = current_role.can_input()
        && !(input_mode == InputMode::Serialized && current_role != Role::Owner);
    // Precompute the `InputDenied` reason so the hot path never has to
    // branch twice for the same answer. `None` means "input is allowed";
    // `Some(reason)` means every binary frame gets dropped and the first
    // one triggers a single-shot user notice.
    let input_denied_reason = if can_forward_input {
        None
    } else if current_role == Role::Viewer {
        Some(input_denied::VIEWER)
    } else {
        Some(input_denied::SERIALIZED_NOT_OWNER)
    };
    // Latch so we only send the denial notice once per connection —
    // otherwise every keystroke would spam the client's toast bus.
    let mut denial_notice_sent = false;

    // Track the last accepted CursorMove timestamp per connection so we can
    // drop floods without spinning up a timer task.
    let mut last_cursor_at: Option<Instant> = None;

    loop {
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
                        match input_denied_reason {
                            None => {
                                // `data` is already a refcounted `Bytes` from
                                // axum 0.8's WS codec — forward without copy.
                                let _ = cmd_tx.send(PtyCommand::Input(data)).await;
                            }
                            Some(reason) => {
                                if !denial_notice_sent {
                                    denial_notice_sent = true;
                                    // One-shot denial notice. `try_send`
                                    // so a momentarily-full personal
                                    // buffer never blocks the input
                                    // loop; worst case the notice is
                                    // dropped and the client stays quiet
                                    // — far better than stalling input
                                    // processing on a toast.
                                    let _ = personal_tx.try_send(
                                        ServerMessage::InputDenied {
                                            reason: reason.to_string(),
                                        },
                                    );
                                }
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

    let _ = stop_tx.send(());
    // Give the forwarder a bounded grace window to flush pending sends.
    // Passing `&mut output_handle` keeps ownership locally so we can
    // call `.abort()` if it doesn't shut down — the previous code moved
    // the handle into `timeout`, which on elapse just dropped the
    // handle and silently leaked the task (it kept running, blocked on
    // a backpressured TCP send, until the runtime tore it down).
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut output_handle)
        .await
        .is_err()
    {
        tracing::warn!(session = %session_id, "output handler did not stop within 2s, aborting");
        output_handle.abort();
    }
    // Drop the in-memory connection record so the reaper's idle clock
    // can start counting, but **do not** touch the DB `left_at` here.
    // Writing `left_at` on socket close used to race with the reaper's
    // 2-minute grace window: clients that dropped and reconnected a
    // second later were rejected as NOT_PARTICIPANT even though the
    // hub was still holding their session open. `close_session` /
    // `close_stale_sessions` now own the participant-cleanup write,
    // which keeps the DB consistent with the session lifecycle.
    hub.remove_participant(&session_id, user_id).await;
    tracing::info!(user = %user_name, session = %session_id, "WebSocket disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use telepair_core::protocol::{CLOSE_CODE_TERMINAL, CLOSE_CODE_TRANSIENT};

    /// Pull the `CloseFrame` out of a `Message::Close`, or panic with a
    /// descriptive error so test failures point at the right thing.
    fn expect_close_frame(msg: &Message) -> &CloseFrame {
        match msg {
            Message::Close(Some(frame)) => frame,
            other => panic!("expected Message::Close(Some(..)), got {other:?}"),
        }
    }

    // The whole fix hinges on this: `STORAGE_ERROR` must ride out on a
    // close code the frontend recognizes as transient, not on 4001
    // which `web/src/lib/ws.ts` treats as terminal. If this test
    // regresses, a SQLite hiccup again strands users on a dead page.
    #[test]
    fn build_error_frames_uses_transient_close_for_storage_error() {
        let (_text, close) = build_error_frames(error_codes::STORAGE_ERROR, "boom".into());
        let frame = expect_close_frame(&close);
        assert_eq!(frame.code, CLOSE_CODE_TRANSIENT);
        assert_ne!(frame.code, CLOSE_CODE_TERMINAL);
    }

    // Counterpart: revoked tokens / missing sessions MUST stay terminal.
    // A regression that made these transient would turn every bad
    // credential into a reconnect storm against the gateway.
    #[test]
    fn build_error_frames_uses_terminal_close_for_permanent_errors() {
        for code in [
            error_codes::AUTH_FAILED,
            error_codes::AUTH_TIMEOUT,
            error_codes::NOT_PARTICIPANT,
            error_codes::SESSION_NOT_FOUND,
            error_codes::SESSION_CLOSED,
            error_codes::TARGET_NOT_FOUND,
            error_codes::PTY_ERROR,
        ] {
            let (_text, close) = build_error_frames(code, "nope".into());
            let frame = expect_close_frame(&close);
            assert_eq!(
                frame.code, CLOSE_CODE_TERMINAL,
                "{code} must stay terminal to avoid reconnect storms"
            );
        }
    }

    // The text frame is what the client's `onmessage` handler parses to
    // render a localized error — ensure the protocol `code` survives
    // the round-trip through `build_error_frames`.
    #[test]
    fn build_error_frames_text_frame_carries_error_code() {
        let (text, _close) = build_error_frames(error_codes::STORAGE_ERROR, "temporary".into());
        let Message::Text(json) = text else {
            panic!("expected Message::Text, got {text:?}");
        };
        let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerMessage::Error { code, message } => {
                assert_eq!(code, error_codes::STORAGE_ERROR);
                assert_eq!(message, "temporary");
            }
            other => panic!("expected Error variant, got {other:?}"),
        }
    }
}
