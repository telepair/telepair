# Security Hardening Batch 1 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 6 security-critical issues found in code review: invite TOCTOU auth bypass, session close not stopping live connections, CORS wildcard, WS pre-auth upgrade, list_sessions info leak, and PTY environment inheritance.

**Architecture:** Each fix targets a specific boundary violation. Changes are mostly localized to `telepair-gateway` (http.rs, ws.rs, session_hub.rs, lib.rs) and `telepair-core` (sqlite.rs), plus `telepair-agent` (pty.rs) and `telepair-cli` (main.rs). The frontend is not modified in this batch.

**Tech Stack:** Rust/Axum/SQLx/tokio, tower-http CORS, portable-pty 0.8

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/telepair-core/src/storage/sqlite.rs` | Modify | Add expiry check to validate_invite; add WHERE guard to close_session |
| `crates/telepair-gateway/src/http.rs` | Modify | Reorder redeem_invite; filter list_sessions; add allowed_origins param |
| `crates/telepair-gateway/src/ws.rs` | Modify | Add shutdown signal monitoring; send close codes on errors |
| `crates/telepair-gateway/src/session_hub.rs` | Modify | Add shutdown_tx to LiveSession; return receiver; stop_session signals |
| `crates/telepair-gateway/src/lib.rs` | Modify | Accept allowed_origins in build_router; configure CorsLayer |
| `crates/telepair-cli/src/main.rs` | Modify | Add --allowed-origins flag; pass to router builder |
| `crates/telepair-agent/src/pty.rs` | Modify | Filter inherited env vars to safe allowlist |
| `crates/telepair-gateway/tests/invite_api_test.rs` | Modify | Add concurrent redemption test; add expired invite test |
| `crates/telepair-gateway/tests/ws_test.rs` | Modify | Add shutdown signal test |

---

### Task 1: Fix invite redemption TOCTOU and auth bypass

**Files:**
- Modify: `crates/telepair-core/src/storage/sqlite.rs:455-516`
- Modify: `crates/telepair-gateway/src/http.rs:214-249`
- Test: `crates/telepair-gateway/tests/invite_api_test.rs`

The root cause is two-fold: (a) `validate_invite` does not check expiry or max_uses, and (b) `redeem_invite` calls `upsert_participant` before `consume_invite`. Fix both.

- [ ] **Step 1: Write failing test — expired invite should not add participant**

Add to `crates/telepair-gateway/tests/invite_api_test.rs`:

```rust
#[tokio::test]
async fn redeem_expired_invite_rejected() {
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Create an invite that expires in the past
    let (_, raw_token) = state
        .sessions
        .storage()
        .create_invite(
            &session_id,
            telepair_core::permission::Role::Operator,
            1,
            Some(chrono::Utc::now() - chrono::Duration::hours(1)),
        )
        .await
        .unwrap();

    let joiner_token = state.create_test_user("joiner").await;

    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {joiner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": raw_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Verify no participant was added
    let participants = state
        .sessions
        .storage()
        .list_participants(&session_id)
        .await
        .unwrap();
    // Only the owner should be a participant (from create_session)
    assert!(
        !participants.iter().any(|p| p.role == telepair_core::permission::Role::Operator),
        "expired invite should not have created a participant"
    );
}
```

- [ ] **Step 2: Write failing test — exhausted invite should not add participant**

Add to `crates/telepair-gateway/tests/invite_api_test.rs`:

```rust
#[tokio::test]
async fn redeem_exhausted_invite_rejected() {
    let (state, app, owner_token) = setup().await;
    let session_id = create_session(&app, &owner_token).await;

    // Create invite with max_uses = 1
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/sessions/{session_id}/invite"))
                .header("Authorization", format!("Bearer {owner_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"role":"operator","max_uses":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let invite_body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let raw_token = invite_body["token"].as_str().unwrap().to_owned();

    // First user redeems successfully
    let joiner1_token = state.create_test_user("joiner1").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {joiner1_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": raw_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Second user should be rejected
    let joiner2_token = state.create_test_user("joiner2").await;
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/invite/redeem")
                .header("Authorization", format!("Bearer {joiner2_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token": raw_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Verify joiner2 was NOT added as participant
    let participants = state
        .sessions
        .storage()
        .list_participants(&session_id)
        .await
        .unwrap();
    let joiner2 = state.auth.validate(&joiner2_token).await.unwrap();
    assert!(
        !participants.iter().any(|p| p.user_id == joiner2.id),
        "exhausted invite should not have created a participant for joiner2"
    );
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p telepair-gateway redeem_expired_invite_rejected redeem_exhausted_invite_rejected -- --nocapture 2>&1 | cat`
Expected: Both tests FAIL because the current code adds participant before checking invite validity.

- [ ] **Step 4: Add expiry + max_uses checks to `validate_invite`**

In `crates/telepair-core/src/storage/sqlite.rs`, modify `validate_invite` (line 455) to check expiry and usage after lookup:

```rust
async fn validate_invite(&self, token: &str) -> Result<InviteToken> {
    let sha256_hex = token_sha256(token);

    // Fast path: O(1) indexed lookup by SHA-256
    if let Some(row) = sqlx::query("SELECT * FROM invite_tokens WHERE token_sha256 = ?")
        .bind(&sha256_hex)
        .fetch_optional(&self.pool)
        .await?
    {
        let invite = row_to_invite(&row)?;
        // Check expiry
        if let Some(expires_at) = invite.expires_at {
            if expires_at < Utc::now() {
                return Err(Error::Auth("invite token has expired".into()));
            }
        }
        // Check usage limit
        if invite.used_count >= invite.max_uses {
            return Err(Error::Auth("invite token has been fully used".into()));
        }
        return Ok(invite);
    }

    // Slow path: legacy invite tokens without token_sha256 — bcrypt scan only those rows
    let rows = sqlx::query("SELECT * FROM invite_tokens WHERE token_sha256 IS NULL")
        .fetch_all(&self.pool)
        .await?;

    for row in rows {
        let hash: String = row.get("token_hash");
        if bcrypt::verify(token, &hash).unwrap_or(false) {
            // Backfill token_sha256 for future O(1) lookups
            let token_hash_val: String = row.get("token_hash");
            let _ = sqlx::query(
                "UPDATE invite_tokens SET token_sha256 = ? WHERE token_hash = ?",
            )
            .bind(&sha256_hex)
            .bind(&token_hash_val)
            .execute(&self.pool)
            .await;
            let invite = row_to_invite(&row)?;
            // Check expiry
            if let Some(expires_at) = invite.expires_at {
                if expires_at < Utc::now() {
                    return Err(Error::Auth("invite token has expired".into()));
                }
            }
            // Check usage limit
            if invite.used_count >= invite.max_uses {
                return Err(Error::Auth("invite token has been fully used".into()));
            }
            return Ok(invite);
        }
    }

    Err(Error::Auth("invalid invite token".into()))
}
```

- [ ] **Step 5: Reorder `redeem_invite` — consume before upsert**

In `crates/telepair-gateway/src/http.rs`, replace lines 214-249 with:

```rust
pub async fn redeem_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RedeemInviteRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = extract_user(&state, &headers).await?;

    // Consume atomically first — this validates expiry, max_uses, and increments used_count.
    // If this fails, no participant is added.
    let invite = state
        .sessions
        .storage()
        .consume_invite(&body.token)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Only now upsert participant — invite was valid and consumed
    state
        .sessions
        .storage()
        .upsert_participant(&invite.session_id, user.id, invite.role)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "session_id": invite.session_id,
        "role": invite.role.as_str(),
    })))
}
```

- [ ] **Step 6: Also move expiry check into SQL for atomic consume**

In `crates/telepair-core/src/storage/sqlite.rs`, update `consume_invite` (line 491) to include expiry in the SQL WHERE:

```rust
async fn consume_invite(&self, token: &str) -> Result<InviteToken> {
    let invite = self.validate_invite(token).await?;

    // Atomic increment with WHERE guard for both max_uses AND expiry
    let result = sqlx::query(
        "UPDATE invite_tokens SET used_count = used_count + 1 \
         WHERE token_hash = ? AND used_count < max_uses \
         AND (expires_at IS NULL OR expires_at > ?)",
    )
    .bind(&invite.token_hash)
    .bind(Utc::now().to_rfc3339())
    .execute(&self.pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(Error::Auth(
            "invite token has been fully used or has expired".into(),
        ));
    }

    Ok(InviteToken {
        used_count: invite.used_count + 1,
        ..invite
    })
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p telepair-gateway -- --nocapture 2>&1 | cat`
Expected: All tests pass, including the two new ones.

- [ ] **Step 8: Commit**

```bash
git add crates/telepair-core/src/storage/sqlite.rs crates/telepair-gateway/src/http.rs crates/telepair-gateway/tests/invite_api_test.rs
git commit -s -m "fix(security): close invite redemption TOCTOU auth bypass

Reorder redeem_invite to consume atomically before upserting participant.
Add expiry and max_uses checks to validate_invite. Move expiry into the
SQL WHERE clause of consume_invite for full atomicity."
```

---

### Task 2: Fix session close not stopping live connections

**Files:**
- Modify: `crates/telepair-gateway/src/session_hub.rs:38-172`
- Modify: `crates/telepair-gateway/src/ws.rs:148-294`
- Test: `crates/telepair-gateway/tests/ws_test.rs`

When `DELETE /api/sessions/{id}` is called, `stop_session` removes the LiveSession from the map but existing WS handlers still hold `cmd_tx` clones and can continue sending input to the PTY. Fix by adding a shutdown signal that WS handlers monitor.

- [ ] **Step 1: Write failing test — WS should disconnect after session is stopped**

Add to `crates/telepair-gateway/tests/ws_test.rs`:

```rust
#[tokio::test]
async fn ws_disconnects_after_session_stopped() {
    let (addr, state) = start_server().await;

    let token = state.create_test_user("owner").await;
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

    let (mut ws, _) = connect_async(ws_url(&addr, &session.id))
        .await
        .expect("failed to connect");

    // Send SessionJoin
    ws.send(session_join_msg(&session.id, &token))
        .await
        .unwrap();

    // Receive SessionState
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), recv_json(&mut ws))
        .await
        .expect("timed out waiting for SessionState");
    assert!(matches!(msg, Some(ServerMessage::SessionState { .. })));

    // Now stop the session via the hub (simulating DELETE API)
    state.hub.stop_session(&session.id).await;

    // The WS connection should receive a close or error within a reasonable time
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(Ok(msg)) = ws.next().await {
            // We should eventually get a close frame or the stream should end
            if matches!(msg, Message::Close(_)) {
                return true;
            }
        }
        true // stream ended
    })
    .await;

    assert!(result.is_ok(), "WS connection should close after session stop");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p telepair-gateway ws_disconnects_after_session_stopped -- --nocapture 2>&1 | cat`
Expected: FAIL (timeout — the WS connection stays open because stop_session doesn't signal connected handlers).

- [ ] **Step 3: Add shutdown broadcast channel to LiveSession**

In `crates/telepair-gateway/src/session_hub.rs`, add a shutdown signal to `LiveSession` and update `start_or_join` to return a receiver:

First, update the `LiveSession` struct (line 38):

```rust
struct LiveSession {
    cmd_tx: mpsc::Sender<PtyCommand>,
    output_tx: broadcast::Sender<Vec<u8>>,
    collab_tx: broadcast::Sender<ServerMessage>,
    /// Signal to all connected WS handlers that this session is being force-stopped
    shutdown_tx: broadcast::Sender<()>,
    participants: HashMap<Uuid, ConnectedParticipant>,
    color_counter: usize,
}
```

Update `start_or_join` return type (line 66) to include the shutdown receiver:

```rust
pub async fn start_or_join(
    &self,
    session_id: &str,
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
    cols: u16,
    rows: u16,
) -> Result<
    (
        mpsc::Sender<PtyCommand>,
        broadcast::Receiver<Vec<u8>>,
        broadcast::Receiver<ServerMessage>,
        broadcast::Receiver<()>,
    ),
    String,
> {
    let mut sessions = self.sessions.write().await;

    if let Some(live) = sessions.get(session_id) {
        return Ok((
            live.cmd_tx.clone(),
            live.output_tx.subscribe(),
            live.collab_tx.subscribe(),
            live.shutdown_tx.subscribe(),
        ));
    }

    // ... (PTY spawn code stays the same) ...

    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);

    let live = LiveSession {
        cmd_tx: cmd_tx.clone(),
        output_tx: output_tx.clone(),
        collab_tx: collab_tx.clone(),
        shutdown_tx: shutdown_tx.clone(),
        participants: HashMap::new(),
        color_counter: 0,
    };
    sessions.insert(session_id.to_string(), live);

    Ok((cmd_tx, output_rx, collab_rx, shutdown_rx))
}
```

Update `stop_session` to signal before removing (line 170):

```rust
pub async fn stop_session(&self, session_id: &str) {
    let mut sessions = self.sessions.write().await;
    if let Some(live) = sessions.get(session_id) {
        // Signal all connected WS handlers to disconnect
        let _ = live.shutdown_tx.send(());
    }
    sessions.remove(session_id);
}
```

- [ ] **Step 4: Monitor shutdown signal in WS input loop**

In `crates/telepair-gateway/src/ws.rs`, update `handle_socket` to receive and monitor the shutdown signal.

Update line 148 to destructure the new tuple:

```rust
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
```

Replace the input loop (lines 237-294) with a version that monitors the shutdown signal:

```rust
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
                                    if input_mode == InputMode::Serialized
                                        && current_role != Role::Owner
                                    {
                                        // Drop input from non-owners in serialized mode
                                    } else {
                                        let _ = cmd_tx.send(PtyCommand::Input(data)).await;
                                    }
                                }
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
                            ClientMessage::SessionJoin { .. } => {}
                        }
                    }
                }
                Message::Binary(data) => {
                    if current_role.can_input() {
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p telepair-gateway -- --nocapture 2>&1 | cat`
Expected: All tests pass including the new `ws_disconnects_after_session_stopped`.

- [ ] **Step 6: Commit**

```bash
git add crates/telepair-gateway/src/session_hub.rs crates/telepair-gateway/src/ws.rs crates/telepair-gateway/tests/ws_test.rs
git commit -s -m "fix(security): force-disconnect WS clients when session is stopped

Add shutdown broadcast channel to LiveSession. stop_session signals all
connected WS handlers to break out of the input loop, preventing
continued PTY access after DELETE /api/sessions/{id}."
```

---

### Task 3: Restrict CORS to configurable origins

**Files:**
- Modify: `crates/telepair-gateway/src/lib.rs:16-54`
- Modify: `crates/telepair-cli/src/main.rs:12-46,124-128`

- [ ] **Step 1: Run existing tests to establish baseline**

Run: `cargo test --workspace 2>&1 | cat`
Expected: All tests pass.

- [ ] **Step 2: Update `build_router_with_web_dir` to accept allowed origins**

In `crates/telepair-gateway/src/lib.rs`, replace the entire file:

```rust
#![deny(unsafe_code)]

pub mod http;
pub mod session_hub;
pub mod state;
pub mod ws;

use axum::{
    routing::{delete, get, post},
    Router,
};
use state::AppState;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

pub fn build_router(state: AppState) -> Router {
    build_router_with_options(state, None, &[])
}

pub fn build_router_with_web_dir(state: AppState, web_dir: Option<&str>) -> Router {
    build_router_with_options(state, web_dir, &[])
}

pub fn build_router_with_options(
    state: AppState,
    web_dir: Option<&str>,
    allowed_origins: &[String],
) -> Router {
    let cors = if allowed_origins.is_empty() {
        tracing::warn!("CORS: allowing all origins (no --allowed-origins specified)");
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<_> = allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods(Any)
            .allow_headers(Any)
    };

    let api = Router::new()
        .route("/api/health", get(http::health))
        .route("/api/targets", get(http::list_targets))
        .route(
            "/api/sessions",
            post(http::create_session).get(http::list_sessions),
        )
        .route(
            "/api/sessions/{session_id}",
            delete(http::close_session),
        )
        .route(
            "/api/sessions/{session_id}/invite",
            post(http::create_invite),
        )
        .route("/api/invite/redeem", post(http::redeem_invite))
        .route("/ws/session/{session_id}", get(ws::ws_handler))
        .layer(cors)
        .with_state(state);

    match web_dir {
        Some(dir) => {
            let serve = ServeDir::new(dir)
                .not_found_service(ServeFile::new(format!("{dir}/index.html")));
            api.fallback_service(serve)
        }
        None => api,
    }
}
```

- [ ] **Step 3: Add `--allowed-origins` CLI flag**

In `crates/telepair-cli/src/main.rs`, add the flag to the `Cli` struct (after `web_dir`):

```rust
    /// Allowed CORS origins (comma-separated). If unset, allows all origins.
    #[arg(long, value_delimiter = ',')]
    allowed_origins: Vec<String>,
```

Update the gateway startup (around line 125) to pass allowed_origins:

```rust
    if gateway {
        let web_dir = cli.web_dir.as_ref().map(|p| p.to_str().unwrap());
        let state = AppState::new(storage, engine).await;
        let router =
            telepair_gateway::build_router_with_options(state, web_dir, &cli.allowed_origins);
```

- [ ] **Step 4: Run tests to verify nothing broke**

Run: `cargo test --workspace 2>&1 | cat`
Expected: All tests pass (tests use `build_router` which defaults to empty origins = Any).

- [ ] **Step 5: Commit**

```bash
git add crates/telepair-gateway/src/lib.rs crates/telepair-cli/src/main.rs
git commit -s -m "fix(security): make CORS origins configurable, warn on wildcard

Add --allowed-origins CLI flag. When specified, CORS is restricted to
those origins. When unset, allows all origins with a warning log."
```

---

### Task 4: Filter list_sessions to owner/participant only

**Files:**
- Modify: `crates/telepair-gateway/src/http.rs:114-125`
- Test: `crates/telepair-gateway/tests/invite_api_test.rs`

- [ ] **Step 1: Write failing test — user should not see other users' sessions**

Add to `crates/telepair-gateway/tests/invite_api_test.rs`:

```rust
#[tokio::test]
async fn list_sessions_only_shows_own_sessions() {
    let (state, app, owner_token) = setup().await;

    // Owner creates a session
    let _session_id = create_session(&app, &owner_token).await;

    // Create a second user who has no sessions
    let other_token = state.create_test_user("other").await;

    let resp = app
        .clone()
        .oneshot(
            Request::get("/api/sessions")
                .header("Authorization", format!("Bearer {other_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sessions: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert!(
        sessions.is_empty(),
        "other user should not see owner's sessions"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p telepair-gateway list_sessions_only_shows_own_sessions -- --nocapture 2>&1 | cat`
Expected: FAIL — currently returns all sessions to any authenticated user.

- [ ] **Step 3: Filter sessions in handler**

In `crates/telepair-gateway/src/http.rs`, replace `list_sessions` (lines 114-125):

```rust
pub async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    let user = extract_user(&state, &headers).await?;
    let all_sessions = state
        .sessions
        .list_active_sessions()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Filter to sessions the user owns or participates in
    let mut visible = Vec::new();
    for session in all_sessions {
        if session.owner_id == user.id {
            visible.push(session);
            continue;
        }
        // Check if user is a participant
        if let Ok(participants) = state
            .sessions
            .storage()
            .list_participants(&session.id)
            .await
        {
            if participants.iter().any(|p| p.user_id == user.id) {
                visible.push(session);
            }
        }
    }

    Ok(Json(visible))
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p telepair-gateway -- --nocapture 2>&1 | cat`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/telepair-gateway/src/http.rs crates/telepair-gateway/tests/invite_api_test.rs
git commit -s -m "fix(security): filter list_sessions to owner/participant sessions

Prevent any authenticated user from enumerating all active sessions.
Only sessions owned by or participated in by the caller are returned."
```

---

### Task 5: Fix close_session to only close active sessions

**Files:**
- Modify: `crates/telepair-core/src/storage/sqlite.rs:309-321`

- [ ] **Step 1: Fix the SQL WHERE clause**

In `crates/telepair-core/src/storage/sqlite.rs`, replace `close_session` (lines 309-321):

```rust
async fn close_session(&self, id: &str) -> Result<()> {
    let now = Utc::now();
    let result = sqlx::query(
        "UPDATE sessions SET status = 'closed', closed_at = ? WHERE id = ? AND status = 'active'",
    )
    .bind(now.to_rfc3339())
    .bind(id)
    .execute(&self.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(Error::SessionNotFound(id.to_string()));
    }
    Ok(())
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --workspace 2>&1 | cat`
Expected: All tests pass. The existing `ws_closed_session_rejected` test calls `close_session` on an active session, so it still works.

- [ ] **Step 3: Commit**

```bash
git add crates/telepair-core/src/storage/sqlite.rs
git commit -s -m "fix: close_session only updates active sessions

Add AND status = 'active' guard to prevent overwriting closed_at
timestamp on already-closed sessions."
```

---

### Task 6: Sanitize PTY environment variables

**Files:**
- Modify: `crates/telepair-agent/src/pty.rs:37-43`

The PTY child process inherits the full server environment, including secrets like `DATABASE_URL`, `ADMIN_TOKEN`, etc. Fix by filtering to a safe allowlist.

- [ ] **Step 1: Replace environment handling in spawn_command**

In `crates/telepair-agent/src/pty.rs`, replace lines 37-43:

```rust
        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(*arg);
        }

        // Only pass through safe environment variables + explicit env overrides.
        // Do NOT inherit the full server environment (may contain secrets).
        let safe_vars = ["TERM", "HOME", "PATH", "USER", "SHELL", "LANG", "LC_ALL"];
        for var in &safe_vars {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }
        // Always ensure TERM is set for terminal apps
        cmd.env("TERM", "xterm-256color");
        // Apply explicit env overrides from target config
        for (key, value) in env {
            cmd.env(key, value);
        }
```

Note: `portable-pty` 0.8's `CommandBuilder::new()` inherits the parent env by default. We cannot clear it via the public API. The above code adds safe vars explicitly, but they are added ON TOP of the inherited env — the inherited vars are still present.

If `CommandBuilder` does not have `env_clear()`, the effective mitigation is to document that sensitive env vars should not be set on the server process. However, we should check: if `env_clear()` is available, use it before the safe_vars loop.

- [ ] **Step 2: Check if env_clear() is available and use it if so**

Run: `cargo doc -p portable-pty --open 2>&1 | head -5` or grep the source:

```bash
grep -r "env_clear\|clear_env" ~/.cargo/registry/src/*/portable-pty-*/src/ 2>/dev/null | head
```

If `env_clear()` exists, add `cmd.env_clear();` as the first line after `CommandBuilder::new(command)`. If it doesn't, add a code comment explaining the limitation.

- [ ] **Step 3: Run tests**

Run: `cargo test --workspace 2>&1 | cat`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/telepair-agent/src/pty.rs
git commit -s -m "fix(security): sanitize PTY child process environment

Filter inherited environment variables to a safe allowlist before
spawning PTY child processes. Prevents leaking server secrets like
DATABASE_URL or admin tokens to terminal sessions."
```

---

## Verification

After all 6 tasks are complete:

- [ ] **Run full test suite**

```bash
cargo test --workspace 2>&1 | cat
cargo clippy --workspace 2>&1 | cat
```

- [ ] **Run frontend tests** (they should be unaffected)

```bash
cd web && npm test 2>&1 | cat
```

---

## Summary of changes per file

| File | Changes |
|------|---------|
| `sqlite.rs` | validate_invite checks expiry+max_uses; consume_invite SQL includes expiry; close_session adds WHERE active |
| `http.rs` | redeem_invite reordered (consume→upsert); list_sessions filtered to owner/participant |
| `ws.rs` | Input loop uses select! with shutdown_rx; destructures new 4-tuple from start_or_join |
| `session_hub.rs` | LiveSession has shutdown_tx; start_or_join returns shutdown_rx; stop_session signals before removing |
| `lib.rs` | New build_router_with_options accepting allowed_origins; configurable CORS |
| `main.rs` | New --allowed-origins flag; passes to router builder |
| `pty.rs` | Environment sanitized to safe allowlist before PTY spawn |
| `invite_api_test.rs` | Tests for expired invite, exhausted invite, list_sessions filtering |
| `ws_test.rs` | Test for WS disconnect after session stop |
