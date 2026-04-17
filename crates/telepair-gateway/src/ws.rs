use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    ChatEntry, ClientMessage, RecordingStatusInfo, ServerMessage, close_code_for, error_codes,
    input_denied,
};
use telepair_core::session::{CloseReason, InputMode};
use telepair_core::storage::Storage;

use crate::session_hub::{PtyCommand, PtyLaunch, SessionAttachment};
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
    // Drop the hub reservation minted by `create_session` before this
    // attach attempt failed. Without this, a `Pending` entry sits in
    // the hub for up to `pending_attach_ttl` and `count_live_sessions
    // _per_target` keeps reporting the target as in-use, blocking
    // `/api/admin/targets/reload` on a phantom session that nobody is
    // ever going to attach to. `release_reservation` is a no-op on
    // Live entries (paranoid against a slow error path racing a real
    // attach), so it's safe to call unconditionally here.
    state.hub.release_reservation(session_id).await;
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

/// Resolve the `(command, args, env)` tuple for a live WS attach. The
/// session row's `user_target_id` is the only authority on which
/// namespace to consult — `Some(_)` means the session was launched from
/// a user-owned target and must resolve via the storage-backed
/// `UserTargetService`; `None` means the launch came from a global
/// target and must resolve via the in-memory `TargetEngine`. There is
/// no fallback in either direction.
///
/// Why no fallback: namespaces overlap. A user named their VPS `vps`
/// and so did the admin in `targets.yaml`. Without this strict split,
/// a global `vps` added between session create and WS attach would
/// shadow the user's target and launch the admin's command (with
/// admin-supplied env) on the user's session — see
/// `resolve_session_pty_does_not_fall_back_when_user_target_missing`.
#[derive(Debug)]
enum ResolveError {
    /// The target genuinely does not exist — safe to clean up the session.
    NotFound(String),
    /// A transient storage failure — the target may still exist, so the
    /// session must NOT be closed.
    Storage(String),
}

async fn resolve_session_pty(
    user_target_id: Option<&str>,
    target_name: &str,
    state: &AppState,
) -> Result<
    (
        String,
        Vec<String>,
        std::collections::HashMap<String, String>,
    ),
    ResolveError,
> {
    if let Some(id) = user_target_id {
        return match state.user_targets.resolve_by_id(id).await {
            Ok(Some(t)) => Ok(t),
            Ok(None) => Err(ResolveError::NotFound(format!(
                "user target {id} not found"
            ))),
            Err(e) => Err(ResolveError::Storage(format!(
                "failed to resolve user target {id}: {e}"
            ))),
        };
    }
    state
        .targets
        .load()
        .resolve(target_name)
        .ok_or_else(|| ResolveError::NotFound(format!("target {target_name} not found")))
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

/// Compute the `InputDenied` reason for a given role and input mode.
/// Returns `None` when input is allowed, `Some(reason)` when it should
/// be blocked. Extracted so both the initial setup and the runtime
/// `PeerRoleChanged` handler use the same logic.
fn compute_input_denied(role: Role, input_mode: InputMode) -> Option<&'static str> {
    let can_forward =
        role.can_input() && !(input_mode == InputMode::Serialized && role != Role::Owner);
    if can_forward {
        None
    } else if role == Role::Viewer {
        Some(input_denied::VIEWER)
    } else {
        Some(input_denied::SERIALIZED_NOT_OWNER)
    }
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

    // `session_enabled = FALSE` gate: a self-served email signup
    // that has not been approved by an admin cannot attach to any
    // session — not even one they somehow became a participant of
    // before being disabled. Scoped guests are seeded with the bit
    // ON at mint time and the scope pin above has already been
    // enforced, so this check only ever fires on disabled real
    // accounts. Admins bypass it so the bootstrap path cannot lock
    // itself out. The rejection is audited as
    // `auth.session_access_denied` to mirror the HTTP-side gate and
    // give operators a single place to correlate disabled-account
    // probes across both attach surfaces.
    if !user.session_enabled && !user.is_admin {
        crate::http::audit_session_access_denied(
            &state,
            &user,
            "WS /ws/session/{id}",
            Some(&session_id),
        )
        .await;
        send_error(
            &mut ws_tx,
            error_codes::SESSION_DISABLED,
            "account is pending admin approval".into(),
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

    // Three-way split is load-bearing: lumping `Err(_)` in with
    // `Ok(None)` would surface a transient SQLite hiccup as
    // `SESSION_NOT_FOUND`, which `web/src/lib/ws.ts` treats as a
    // terminal close — the user sees "session does not exist" and
    // the page never reconnects, even though the row is still on
    // disk. Storage errors must take the same `STORAGE_ERROR` /
    // transient close path as the `participants_res` branch below.
    let session = match session_res {
        Ok(Some(s)) => s,
        Ok(None) => {
            send_error(
                &mut ws_tx,
                error_codes::SESSION_NOT_FOUND,
                format!("session {session_id} not found"),
            )
            .await;
            return;
        }
        Err(e) => {
            tracing::error!(session = %session_id, error = %e, "failed to load session");
            send_error(
                &mut ws_tx,
                error_codes::STORAGE_ERROR,
                "temporary storage failure — please retry".into(),
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
    // Resolve the target via the namespace recorded on the session row at
    // create time. If `user_target_id` is `Some`, this session was launched
    // from a user-owned target — we MUST resolve by id and never consult
    // the global engine, otherwise a global target with a colliding name
    // added between create and attach would silently launch the wrong PTY
    // (with arbitrary admin-supplied env). The reverse is also true: a
    // session created against a global target does not get to fall back to
    // a user-owned target with the same name. See the `resolve_session_pty`
    // tests below for the regression coverage.
    let resolved = resolve_session_pty(
        session.user_target_id.as_deref(),
        &session.target_name,
        &state,
    )
    .await;
    let (cmd, args, env) = match resolved {
        Ok(tuple) => tuple,
        Err(ResolveError::NotFound(msg)) => {
            tracing::warn!(
                session = %session_id,
                target = %session.target_name,
                user_target_id = ?session.user_target_id,
                "target not found: {msg}",
            );
            cleanup_orphan_session(&state, &session_id).await;
            send_error(&mut ws_tx, error_codes::TARGET_NOT_FOUND, msg).await;
            return;
        }
        Err(ResolveError::Storage(msg)) => {
            tracing::error!(
                session = %session_id,
                target = %session.target_name,
                user_target_id = ?session.user_target_id,
                "transient storage error resolving target: {msg}",
            );
            send_error(&mut ws_tx, error_codes::STORAGE_ERROR, msg).await;
            return;
        }
    };
    let SessionAttachment {
        cmd_tx,
        mut output_rx,
        mut collab_rx,
        mut shutdown_rx,
        chat_history,
        scrollback,
    } = match hub
        .start_or_join(
            &session_id,
            &session.target_name,
            PtyLaunch {
                command: &cmd,
                args: &args,
                env: &env,
                cols: initial_cols,
                rows: initial_rows,
            },
        )
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

    let recording_status = match state.storage.find_active_recording(&session_id).await {
        Ok(Some(rec)) => Some(RecordingStatusInfo {
            recording_id: rec.id,
            started_at: rec.started_at,
        }),
        _ => None,
    };

    let state_msg = ServerMessage::SessionState {
        session: session.clone(),
        participants,
        your_role: my_role,
        your_user_id: user.id,
        chat_history,
        recording: recording_status,
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

    // Second collab subscriber for the main loop — used to intercept
    // `PeerRoleChanged` targeting this connection so input permissions
    // update without a reconnect.
    let mut role_rx = collab_rx.resubscribe();

    // Spawn a forwarder that pumps PTY output + collab messages to the WS sink.
    // `stop_tx` (oneshot) lets the main loop tell the forwarder to exit.
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    // `personal_tx` is the main loop's private path back to the WS sink:
    // it lets us push per-connection control messages (e.g. `InputDenied`)
    // without broadcasting to every other participant. Buffer of 4 is
    // enough for notice-once latches; a full buffer is a drop signal.
    let (personal_tx, mut personal_rx) = mpsc::channel::<ServerMessage>(4);

    let session_id_for_output = session_id.clone();
    // Captured here (rather than re-read inside the task) so the
    // forwarder can detect self-eviction without having to touch
    // `user` after ownership moves into the async block. Uuid is
    // Copy, so this is just a field read.
    let forwarder_user_id = user.id;
    // Shared "was the socket force-evicted?" flag. The forwarder sets
    // it the instant it sees a `PeerEvicted` frame aimed at this user;
    // the main loop reads it just before deciding whether to call
    // `hub.remove_participant` on exit. Without this, an evicted WS
    // task would still decrement the hub's refcount for its user_id
    // after `SessionHub::evict_user` already cleared the entry, and if
    // the user re-attached quickly (admin re-enabled the account), the
    // stale decrement would drop the fresh participant from the live
    // map and broadcast a spurious `PeerLeft`.
    let was_evicted = Arc::new(AtomicBool::new(false));
    let was_evicted_forwarder = was_evicted.clone();
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
                            // Detect self-eviction BEFORE the send so we can
                            // follow the frame with an explicit Close in one
                            // serialized sequence. Doing this in the forwarder
                            // (instead of in the main loop + a stop signal)
                            // sidesteps the race where tokio::select! picks
                            // `stop_rx` over the next `collab_rx.recv()` and
                            // drops the notice frame on the floor — the only
                            // task that owns `ws_tx` is the one that both
                            // forwards and closes, so ordering is atomic.
                            let self_evict_reason = if let ServerMessage::PeerEvicted {
                                user_id,
                                reason,
                            } = collab_msg
                                && user_id == forwarder_user_id
                            {
                                Some(reason)
                            } else {
                                None
                            };
                            let Ok(json) = serde_json::to_string(&collab_msg) else {
                                tracing::error!("failed to serialize collab message");
                                continue;
                            };
                            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                            if let Some(reason) = self_evict_reason {
                                // Signal the main loop to skip its own
                                // `remove_participant` bookkeeping on exit:
                                // `SessionHub::evict_user` has already torn
                                // down the participant and the connection
                                // refcount, and a second decrement here
                                // would race a reconnect from the same user
                                // (admin re-enable) and spuriously drop the
                                // fresh participant. Release ordering so the
                                // main loop's Acquire load observes this
                                // write after the eviction-triggered Close
                                // traverses the WS stream.
                                was_evicted_forwarder.store(true, Ordering::Release);
                                // Terminal close — the client should route
                                // the user per `reason`. The close-frame
                                // reason string mirrors the PeerEvicted
                                // enum so a client that only inspects the
                                // Close frame (bypassing the JSON path,
                                // e.g. raw tooling) can still tell the
                                // cases apart. `CLOSE_CODE_TERMINAL` is
                                // the same signal the HTTP-side
                                // SESSION_DISABLED rejection uses; the
                                // frontend's JSON handler already routes
                                // on the enum and doesn't rely on the
                                // close-reason text.
                                let close_reason = match reason {
                                    telepair_core::protocol::EvictReason::AccountDisabled => {
                                        "account disabled"
                                    }
                                    telepair_core::protocol::EvictReason::TokenRotated => {
                                        "token rotated"
                                    }
                                };
                                let _ = ws_tx
                                    .send(Message::Close(Some(CloseFrame {
                                        code: telepair_core::protocol::CLOSE_CODE_TERMINAL,
                                        reason: close_reason.into(),
                                    })))
                                    .await;
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

    // Role starts at the value captured at join time. It can be
    // mutated at runtime when the owner changes this participant's
    // role via the REST API — the `role_rx` arm below picks up the
    // `PeerRoleChanged` broadcast and recalculates input permissions
    // in place, so the connection doesn't need to reconnect.
    let mut current_role = my_role;
    let mut input_denied_reason = compute_input_denied(current_role, input_mode);
    // Latch so we only send the denial notice once per connection —
    // otherwise every keystroke would spam the client's toast bus.
    // Reset when the role changes (a newly-denied user should get
    // one fresh notice).
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
                                        // `record_chat` pushes into the
                                        // bounded history AND broadcasts
                                        // under the same mutex so late
                                        // joiners receive this message
                                        // either via `SessionState.chat_history`
                                        // or via their live `collab_rx`,
                                        // never both, never neither.
                                        let entry = ChatEntry {
                                            user_id,
                                            name: user_name.clone(),
                                            text,
                                            ts: Utc::now().to_rfc3339(),
                                        };
                                        hub.record_chat(&session_id, entry).await;
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
            // Listen for PeerRoleChanged targeting this user so input
            // permissions update without a reconnect. The output forwarder
            // already forwards the message to the WS client; this arm only
            // updates the server-side gate variables.
            result = role_rx.recv() => {
                match result {
                    Ok(ServerMessage::PeerRoleChanged { user_id: uid, new_role })
                        if uid == user_id =>
                    {
                        current_role = new_role;
                        input_denied_reason = compute_input_denied(current_role, input_mode);
                        denial_notice_sent = false;
                        tracing::info!(
                            user = %user_name,
                            session = %session_id,
                            role = %new_role,
                            "role changed at runtime"
                        );
                    }
                    // Self-eviction is handled by the forwarder: it
                    // is the only task that owns `ws_tx`, so the
                    // "send PeerEvicted then Close atomically"
                    // sequence has to live there to avoid racing the
                    // stop signal. The main loop winds down naturally
                    // once the client acknowledges the close frame
                    // and `ws_rx.next()` returns None. We still pin
                    // this branch to document the decision so a
                    // future reader doesn't re-introduce a main-loop
                    // break and reinstate the race.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Messages were dropped — re-fetch the authoritative
                        // role from the hub so a missed demotion cannot leave
                        // stale permissions in effect.
                        tracing::warn!(
                            user = %user_name,
                            session = %session_id,
                            skipped = n,
                            "collab broadcast lagged, re-syncing role from hub"
                        );
                        if let Some(role) = hub.get_participant_role(&session_id, user_id).await
                            && role != current_role
                        {
                            current_role = role;
                            input_denied_reason = compute_input_denied(current_role, input_mode);
                            denial_notice_sent = false;
                            tracing::info!(
                                user = %user_name,
                                session = %session_id,
                                role = %role,
                                "role re-synced after lag"
                            );
                        }
                    }
                    _ => {} // non-role messages for other users
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
    //
    // Skip the decrement entirely if the socket was force-evicted:
    // `SessionHub::evict_user` already removed the participant, and
    // calling `remove_participant` here would race a user's reconnect
    // (admin re-enabled the account before this task exited) and drop
    // the fresh participant from the live map.
    if !was_evicted.load(Ordering::Acquire) {
        hub.remove_participant(&session_id, user_id).await;
    }
    tracing::info!(user = %user_name, session = %session_id, "WebSocket disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use telepair_agent::virtual_target::TargetEngine;
    use telepair_core::protocol::{CLOSE_CODE_TERMINAL, CLOSE_CODE_TRANSIENT};
    use telepair_core::session::CreateUserTargetParams;
    use telepair_core::storage::Storage;

    /// Build a `TargetEngine` whose only virtual target is `vps`,
    /// staging a global namespace collision against any user-owned
    /// `vps` we create in storage. Goes through `from_yaml` so the
    /// `local-shell` auto-injection runs (production never sees an
    /// engine without it).
    fn engine_with_global_vps_echoing(echo: &str) -> TargetEngine {
        let yaml = format!(
            "targets:\n  - name: vps\n    display: Global VPS\n    type: virtual\n    command: /bin/echo\n    args: [\"{echo}\"]\n",
        );
        TargetEngine::from_yaml(&yaml).expect("yaml fixture must parse")
    }

    /// The whole reason `resolve_session_pty` exists: a session whose
    /// row carries `user_target_id = Some(_)` MUST resolve via the
    /// user-target storage table even when a global target with the
    /// same `target_name` exists in the engine. The pre-fix code
    /// resolved global-first and would launch the admin's `/bin/echo
    /// global` on the user's session, complete with admin-controlled
    /// args and env — a privilege boundary blast.
    #[tokio::test]
    async fn resolve_session_pty_uses_user_target_when_id_is_set() {
        let state = AppState::new_test().await;
        state
            .targets
            .store(Arc::new(engine_with_global_vps_echoing("global")));
        let (alice, _) = state.storage.create_user("alice", false).await.unwrap();
        let user_target = state
            .storage
            .create_user_target(CreateUserTargetParams {
                user_id: alice.id,
                name: "vps".into(),
                display: "Alice VPS".into(),
                command: "/bin/echo".into(),
                args: vec!["user".into()],
                env: Default::default(),
                tags: vec![],
            })
            .await
            .unwrap();

        let (cmd, args, _env) = resolve_session_pty(Some(&user_target.id), "vps", &state)
            .await
            .expect("user target must resolve");

        assert_eq!(cmd, "/bin/echo");
        assert_eq!(
            args,
            vec!["user".to_string()],
            "must launch the user-owned command, not the colliding global one"
        );
    }

    /// Counterpart: a global-launched session (`user_target_id` is
    /// `None`) must resolve via the engine even when a user owns a
    /// target with the same name. No silent reverse fallback.
    #[tokio::test]
    async fn resolve_session_pty_uses_global_when_id_is_none() {
        let state = AppState::new_test().await;
        state
            .targets
            .store(Arc::new(engine_with_global_vps_echoing("global")));
        let (alice, _) = state.storage.create_user("alice", false).await.unwrap();
        // Stage a colliding user target so the test would catch a
        // reverse fallback that started reading user_targets by name.
        state
            .storage
            .create_user_target(CreateUserTargetParams {
                user_id: alice.id,
                name: "vps".into(),
                display: "Alice VPS".into(),
                command: "/bin/echo".into(),
                args: vec!["user".into()],
                env: Default::default(),
                tags: vec![],
            })
            .await
            .unwrap();

        let (cmd, args, _env) = resolve_session_pty(None, "vps", &state)
            .await
            .expect("global target must resolve");

        assert_eq!(cmd, "/bin/echo");
        assert_eq!(
            args,
            vec!["global".to_string()],
            "must launch the global command, not the colliding user-owned one"
        );
    }

    /// If the recorded `user_target_id` no longer exists (e.g. the
    /// owner deleted it after the session row was written and the
    /// referential guard was somehow bypassed), the resolver MUST NOT
    /// fall back to whatever global target happens to share the name.
    /// Returning an error is the only safe answer — the WS handler
    /// surfaces it as `TARGET_NOT_FOUND` and tears the session down.
    #[tokio::test]
    async fn resolve_session_pty_does_not_fall_back_when_user_target_missing() {
        let state = AppState::new_test().await;
        state
            .targets
            .store(Arc::new(engine_with_global_vps_echoing("global")));

        // Pass a nanoid that does not exist in the user_targets table.
        let result = resolve_session_pty(Some("nonexistent-nanoid"), "vps", &state).await;
        assert!(
            matches!(result, Err(ResolveError::NotFound(_))),
            "missing user target must be NotFound, not Storage"
        );
    }

    /// And the symmetric case for global launches: a session created
    /// against a global target whose row was later removed from
    /// `targets.yaml` must error rather than picking up some
    /// arbitrary user-owned target with the same name.
    #[tokio::test]
    async fn resolve_session_pty_global_miss_does_not_fall_back_to_user() {
        let state = AppState::new_test().await;
        // Empty engine = only `local-shell`. The `vps` global is gone.
        let (alice, _) = state.storage.create_user("alice", false).await.unwrap();
        state
            .storage
            .create_user_target(CreateUserTargetParams {
                user_id: alice.id,
                name: "vps".into(),
                display: "Alice VPS".into(),
                command: "/bin/echo".into(),
                args: vec!["user".into()],
                env: Default::default(),
                tags: vec![],
            })
            .await
            .unwrap();

        let result = resolve_session_pty(None, "vps", &state).await;
        assert!(
            matches!(result, Err(ResolveError::NotFound(_))),
            "missing global target must be NotFound, not Storage"
        );
    }

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
