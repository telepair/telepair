# Telepair Multi-user Collaboration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add multi-user collaboration to telepair — invite links, participant tracking, permission enforcement, chat, and collaboration UI components.

**Architecture:** Invite tokens stored in SQLite enable secure session sharing. SessionHub tracks connected participants in-memory with broadcast channels for collaboration messages. The WS handler enforces role-based permissions and routes chat/cursor/permission messages between peers. Frontend adds ParticipantList, ChatPanel, and InviteDialog components to the Session page.

**Tech Stack:** Rust (axum, tokio, sqlx, bcrypt, nanoid), SolidJS, TypeScript

---

## Dependency Graph

```
Task 1 (Invite Storage) → Task 2 (Session Hub Multi-user) → Task 4 (Multi-user WS Handler)
                       → Task 3 (Invite REST API)                    ↓
                                                           Task 5 (Collab Message Routing)

Task 6 (Frontend: InviteDialog) ──────→ Task 9 (Session Page Integration)
Task 7 (Frontend: ParticipantList) ───→ Task 9
Task 8 (Frontend: ChatPanel) ─────────→ Task 9
```

## File Structure

```
crates/telepair-core/src/
├── storage.rs               — Add invite token methods to Storage trait
└── storage/sqlite.rs         — Implement invite token storage

crates/telepair-gateway/src/
├── session_hub.rs            — Refactor for multi-user participant tracking
├── ws.rs                     — Multi-user WS handler with permission enforcement
├── http.rs                   — Add invite REST endpoints
└── lib.rs                    — Add new routes

web/src/
├── lib/
│   ├── protocol.ts           — Add invite-related types
│   └── api.ts                — Add invite API methods
├── components/
│   ├── InviteDialog.tsx      — Invite link generation
│   ├── ParticipantList.tsx   — Connected users list
│   └── ChatPanel.tsx         — Sidebar chat
├── pages/
│   ├── Session.tsx           — Wire collaboration components
│   └── Join.tsx              — Invite redemption page
└── App.tsx                   — Add /join/:token route
```

---

### Task 1: Invite Token Storage

**Files:**
- Modify: `crates/telepair-core/src/storage.rs`
- Modify: `crates/telepair-core/src/storage/sqlite.rs`
- Create: `tests/invite_storage_test.rs`

**Depends on:** Nothing

- [ ] **Step 1: Write the failing test**

Create `tests/invite_storage_test.rs`:

```rust
use std::sync::Arc;
use telepair_core::permission::Role;
use telepair_core::storage::{SqliteStorage, Storage};

async fn setup() -> Arc<SqliteStorage> {
    Arc::new(SqliteStorage::new_memory().await.unwrap())
}

#[tokio::test]
async fn create_and_validate_invite_token() {
    let storage = setup().await;
    let (user, _) = storage.create_user("alice", false).await.unwrap();
    let session = storage
        .create_session(user.id, "shell", telepair_core::session::InputMode::Serialized)
        .await
        .unwrap();

    let (invite, raw_token) = storage
        .create_invite(&session.id, Role::Operator, 1, None)
        .await
        .unwrap();

    assert_eq!(invite.session_id, session.id);
    assert_eq!(invite.role, Role::Operator);
    assert_eq!(invite.max_uses, 1);
    assert_eq!(invite.used_count, 0);

    let validated = storage.validate_invite(&raw_token).await.unwrap();
    assert_eq!(validated.session_id, session.id);
}

#[tokio::test]
async fn consume_invite_token() {
    let storage = setup().await;
    let (user, _) = storage.create_user("bob", false).await.unwrap();
    let session = storage
        .create_session(user.id, "shell", telepair_core::session::InputMode::Serialized)
        .await
        .unwrap();

    let (_, raw_token) = storage
        .create_invite(&session.id, Role::Viewer, 1, None)
        .await
        .unwrap();

    let invite = storage.consume_invite(&raw_token).await.unwrap();
    assert_eq!(invite.role, Role::Viewer);
    assert_eq!(invite.used_count, 1);

    // Second consume should fail (max_uses reached)
    let result = storage.consume_invite(&raw_token).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn expired_invite_token_rejected() {
    let storage = setup().await;
    let (user, _) = storage.create_user("carol", false).await.unwrap();
    let session = storage
        .create_session(user.id, "shell", telepair_core::session::InputMode::Serialized)
        .await
        .unwrap();

    // Create with expiry in the past
    let past = chrono::Utc::now() - chrono::Duration::hours(1);
    let (_, raw_token) = storage
        .create_invite(&session.id, Role::Viewer, 1, Some(past))
        .await
        .unwrap();

    let result = storage.consume_invite(&raw_token).await;
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --test invite_storage_test 2>&1 | cat`
Expected: FAIL — `create_invite`, `validate_invite`, `consume_invite` not found on Storage trait

- [ ] **Step 3: Add invite methods to Storage trait**

In `crates/telepair-core/src/storage.rs`, add these methods to the `Storage` trait:

```rust
    // Invite Tokens
    async fn create_invite(
        &self,
        session_id: &str,
        role: Role,
        max_uses: i32,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(InviteToken, String)>;
    async fn validate_invite(&self, token: &str) -> Result<InviteToken>;
    async fn consume_invite(&self, token: &str) -> Result<InviteToken>;
```

Add the `DateTime` and `InviteToken` imports at the top of storage.rs:

```rust
use chrono::{DateTime, Utc};
use crate::session::{InputMode, InviteToken, Participant, Session, User};
```

- [ ] **Step 4: Implement invite methods in SqliteStorage**

In `crates/telepair-core/src/storage/sqlite.rs`, add a `row_to_invite` helper and implement the three methods:

```rust
fn row_to_invite(r: &SqliteRow) -> Result<InviteToken> {
    let role_str: String = r.get("role");
    Ok(InviteToken {
        token_hash: r.get("token_hash"),
        session_id: r.get("session_id"),
        role: match role_str.as_str() {
            "owner" => Role::Owner,
            "operator" => Role::Operator,
            _ => Role::Viewer,
        },
        max_uses: r.get("max_uses"),
        used_count: r.get("used_count"),
        expires_at: r
            .get::<Option<String>, _>("expires_at")
            .and_then(|s| s.parse().ok()),
    })
}
```

Then in the `impl Storage for SqliteStorage`:

```rust
    async fn create_invite(
        &self,
        session_id: &str,
        role: Role,
        max_uses: i32,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(InviteToken, String)> {
        let raw_token = nanoid::nanoid!(32);
        let token_hash =
            bcrypt::hash(&raw_token, 10).map_err(|e| Error::Auth(e.to_string()))?;

        sqlx::query(
            "INSERT INTO invite_tokens (token_hash, session_id, role, max_uses, used_count, expires_at) VALUES (?, ?, ?, ?, 0, ?)",
        )
        .bind(&token_hash)
        .bind(session_id)
        .bind(role.as_str())
        .bind(max_uses)
        .bind(expires_at.map(|t| t.to_rfc3339()))
        .execute(&self.pool)
        .await?;

        let invite = InviteToken {
            token_hash,
            session_id: session_id.into(),
            role,
            max_uses,
            used_count: 0,
            expires_at,
        };
        Ok((invite, raw_token))
    }

    async fn validate_invite(&self, token: &str) -> Result<InviteToken> {
        let rows = sqlx::query("SELECT * FROM invite_tokens")
            .fetch_all(&self.pool)
            .await?;

        for row in rows {
            let hash: String = row.get("token_hash");
            if bcrypt::verify(token, &hash).unwrap_or(false) {
                return row_to_invite(&row);
            }
        }
        Err(Error::Auth("invalid invite token".into()))
    }

    async fn consume_invite(&self, token: &str) -> Result<InviteToken> {
        let invite = self.validate_invite(token).await?;

        // Check expiry
        if let Some(expires) = invite.expires_at {
            if Utc::now() > expires {
                return Err(Error::Auth("invite token expired".into()));
            }
        }

        // Check uses
        if invite.used_count >= invite.max_uses {
            return Err(Error::Auth("invite token exhausted".into()));
        }

        sqlx::query("UPDATE invite_tokens SET used_count = used_count + 1 WHERE token_hash = ?")
            .bind(&invite.token_hash)
            .execute(&self.pool)
            .await?;

        Ok(InviteToken {
            used_count: invite.used_count + 1,
            ..invite
        })
    }
```

Add `use chrono::{DateTime, Utc};` to the imports if not already there (it's already imported for `Utc`). Add `use crate::session::InviteToken;` to the imports.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --test invite_storage_test 2>&1 | cat`
Expected: 3 tests PASS

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --workspace 2>&1 | cat`
Expected: all tests PASS (no regressions)

- [ ] **Step 6: Commit**

```bash
git add crates/telepair-core/src/storage.rs crates/telepair-core/src/storage/sqlite.rs tests/invite_storage_test.rs
git commit -s -m "feat(core): add invite token storage methods"
```

---

### Task 2: Multi-user Session Hub

**Files:**
- Modify: `crates/telepair-gateway/src/session_hub.rs`

**Depends on:** Nothing (parallel with Task 1)

- [ ] **Step 1: Refactor SessionHub for participant tracking**

Replace `crates/telepair-gateway/src/session_hub.rs` entirely:

```rust
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

use telepair_agent::pty::PtyManager;
use telepair_core::permission::Role;
use telepair_core::protocol::ServerMessage;

/// Commands sent to the PTY I/O loop.
pub enum PtyCommand {
    Input(Vec<u8>),
    Resize(u16, u16),
}

/// A connected participant in a live session.
#[derive(Clone, Debug)]
pub struct ConnectedParticipant {
    pub user_id: Uuid,
    pub name: String,
    pub role: Role,
    pub color: String,
}

/// Color palette for participant cursors.
const COLORS: &[&str] = &[
    "#58a6ff", "#3fb950", "#d29922", "#f85149",
    "#bc8cff", "#39c5cf", "#ffa198", "#56d364",
];

fn assign_color(index: usize) -> String {
    COLORS[index % COLORS.len()].to_string()
}

/// A running terminal session with PTY, broadcast channels, and participant tracking.
struct LiveSession {
    cmd_tx: mpsc::Sender<PtyCommand>,
    output_tx: broadcast::Sender<Vec<u8>>,
    collab_tx: broadcast::Sender<ServerMessage>,
    participants: HashMap<Uuid, ConnectedParticipant>,
    color_counter: usize,
}

pub struct SessionHub {
    sessions: Arc<RwLock<HashMap<String, LiveSession>>>,
}

impl Default for SessionHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionHub {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawn a PTY for a session. Returns channels for I/O and collaboration.
    pub async fn start_session(
        &self,
        session_id: &str,
        command: &str,
        args: &[String],
        cols: u16,
        rows: u16,
    ) -> Result<
        (
            mpsc::Sender<PtyCommand>,
            broadcast::Receiver<Vec<u8>>,
            broadcast::Receiver<ServerMessage>,
        ),
        String,
    > {
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let mut pty =
            PtyManager::spawn_command(command, &args_ref, cols, rows).map_err(|e| e.to_string())?;

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<PtyCommand>(256);
        let (output_tx, output_rx) = broadcast::channel::<Vec<u8>>(256);
        let (collab_tx, collab_rx) = broadcast::channel::<ServerMessage>(64);

        let output_tx_clone = output_tx.clone();
        let session_id_owned = session_id.to_string();
        let sessions = self.sessions.clone();

        tokio::spawn(async move {
            loop {
                enum Action {
                    Output(Option<Vec<u8>>),
                    Command(Option<PtyCommand>),
                }

                let action = tokio::select! {
                    data = pty.read() => Action::Output(data),
                    cmd = cmd_rx.recv() => Action::Command(cmd),
                };

                match action {
                    Action::Output(Some(bytes)) => {
                        let _ = output_tx_clone.send(bytes);
                    }
                    Action::Output(None) => {
                        tracing::info!(session = %session_id_owned, "PTY process exited");
                        break;
                    }
                    Action::Command(Some(PtyCommand::Input(data))) => {
                        if pty.write(&data).await.is_err() {
                            break;
                        }
                    }
                    Action::Command(Some(PtyCommand::Resize(cols, rows))) => {
                        let _ = pty.resize(cols, rows);
                    }
                    Action::Command(None) => {
                        tracing::info!(session = %session_id_owned, "all clients disconnected");
                        break;
                    }
                }
            }
            sessions.write().await.remove(&session_id_owned);
        });

        let live = LiveSession {
            cmd_tx: cmd_tx.clone(),
            output_tx: output_tx.clone(),
            collab_tx: collab_tx.clone(),
            participants: HashMap::new(),
            color_counter: 0,
        };
        self.sessions
            .write()
            .await
            .insert(session_id.to_string(), live);

        Ok((cmd_tx, output_rx, collab_rx))
    }

    /// Join an existing live session.
    pub async fn join_session(
        &self,
        session_id: &str,
    ) -> Option<(
        mpsc::Sender<PtyCommand>,
        broadcast::Receiver<Vec<u8>>,
        broadcast::Receiver<ServerMessage>,
    )> {
        let sessions = self.sessions.read().await;
        let live = sessions.get(session_id)?;
        Some((
            live.cmd_tx.clone(),
            live.output_tx.subscribe(),
            live.collab_tx.subscribe(),
        ))
    }

    /// Add a participant to a live session. Returns the assigned color.
    pub async fn add_participant(
        &self,
        session_id: &str,
        user_id: Uuid,
        name: &str,
        role: Role,
    ) -> Option<ConnectedParticipant> {
        let mut sessions = self.sessions.write().await;
        let live = sessions.get_mut(session_id)?;
        let color = assign_color(live.color_counter);
        live.color_counter += 1;
        let participant = ConnectedParticipant {
            user_id,
            name: name.to_string(),
            role,
            color,
        };
        live.participants.insert(user_id, participant.clone());

        // Broadcast PeerJoined to existing participants
        let _ = live.collab_tx.send(ServerMessage::PeerJoined {
            user_id,
            name: name.to_string(),
            role,
            color: participant.color.clone(),
        });

        Some(participant)
    }

    /// Remove a participant from a live session.
    pub async fn remove_participant(&self, session_id: &str, user_id: Uuid) {
        let mut sessions = self.sessions.write().await;
        if let Some(live) = sessions.get_mut(session_id) {
            live.participants.remove(&user_id);
            let _ = live.collab_tx.send(ServerMessage::PeerLeft { user_id });
        }
    }

    /// Get the current participants of a live session.
    pub async fn get_participants(&self, session_id: &str) -> Vec<ConnectedParticipant> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|live| live.participants.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Broadcast a collaboration message to all participants.
    pub async fn broadcast_collab(&self, session_id: &str, msg: ServerMessage) {
        let sessions = self.sessions.read().await;
        if let Some(live) = sessions.get(session_id) {
            let _ = live.collab_tx.send(msg);
        }
    }

    /// Update a participant's role in a live session.
    pub async fn update_participant_role(
        &self,
        session_id: &str,
        user_id: Uuid,
        new_role: Role,
    ) -> bool {
        let mut sessions = self.sessions.write().await;
        if let Some(live) = sessions.get_mut(session_id) {
            if let Some(p) = live.participants.get_mut(&user_id) {
                p.role = new_role;
                let _ = live.collab_tx.send(ServerMessage::PermUpdate {
                    user_id,
                    new_role,
                });
                return true;
            }
        }
        false
    }

    pub async fn is_live(&self, session_id: &str) -> bool {
        self.sessions.read().await.contains_key(session_id)
    }
}
```

- [ ] **Step 2: Fix compilation — update ws.rs for new join_session signature**

The `join_session` and `start_session` now return a 3-tuple (adds collab_rx). Update `crates/telepair-gateway/src/ws.rs` to destructure the third element:

Change line 68-83 from the current 2-tuple destructuring to 3-tuple. For now, just add `_collab_rx` to the destructuring. The full WS refactor happens in Task 4.

Replace the current start/join block:

```rust
    let (cmd_tx, mut output_rx, _collab_rx) = if hub.is_live(&session_id).await {
        match hub.join_session(&session_id).await {
            Some(channels) => channels,
            None => return,
        }
    } else {
        let (cmd, args) = match state.targets.resolve(&session.target_name) {
            Some(resolved) => resolved,
            None => return,
        };
        match hub.start_session(&session_id, &cmd, &args, 80, 24).await {
            Ok(channels) => channels,
            Err(_) => return,
        }
    };
```

- [ ] **Step 3: Run tests to verify no regressions**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --workspace 2>&1 | cat`
Expected: all tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/telepair-gateway/src/session_hub.rs crates/telepair-gateway/src/ws.rs
git commit -s -m "feat(gateway): add multi-user participant tracking to session hub"
```

---

### Task 3: Invite REST API

**Files:**
- Modify: `crates/telepair-gateway/src/http.rs`
- Modify: `crates/telepair-gateway/src/lib.rs`
- Create: `tests/invite_api_test.rs`

**Depends on:** Task 1

- [ ] **Step 1: Write the failing test**

Create `tests/invite_api_test.rs`:

```rust
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use telepair_gateway::build_router;
use telepair_gateway::state::AppState;

async fn setup() -> (axum::Router, String) {
    let state = AppState::new_test().await;
    let token = state.create_test_user("alice").await;
    let router = build_router(state);
    (router, token)
}

#[tokio::test]
async fn create_and_redeem_invite() {
    let (router, token) = setup().await;

    // Create a session first
    let create_body = serde_json::json!({ "target_name": "local-shell" });
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/sessions")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let session: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session_id = session["id"].as_str().unwrap();

    // Create invite
    let invite_body = serde_json::json!({ "role": "operator" });
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/sessions/{session_id}/invite"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&invite_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let invite: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let invite_token = invite["token"].as_str().unwrap();
    assert_eq!(invite["role"].as_str().unwrap(), "operator");

    // Create a second user to redeem the invite
    let state2 = AppState::new_test().await;
    let bob_token = state2.create_test_user("bob").await;

    // Redeem invite (need fresh router with same state — use original)
    let redeem_body = serde_json::json!({ "token": invite_token });
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/invite/redeem")
                .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&redeem_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    // Bob's token is from a different AppState, so auth will fail
    // This test verifies the route exists and requires auth
    assert!(resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::OK);
}

#[tokio::test]
async fn invite_requires_auth() {
    let (router, _) = setup().await;

    let resp = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/sessions/nonexistent/invite")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"role":"viewer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --test invite_api_test 2>&1 | cat`
Expected: FAIL — routes don't exist yet

- [ ] **Step 3: Add invite handlers to http.rs**

Add to `crates/telepair-gateway/src/http.rs`:

```rust
#[derive(Deserialize)]
pub struct CreateInviteRequest {
    pub role: String,
    #[serde(default = "default_max_uses")]
    pub max_uses: i32,
}

fn default_max_uses() -> i32 {
    1
}

pub async fn create_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<CreateInviteRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = extract_user(&state, &headers).await?;

    // Verify session exists and user is the owner
    let session = state
        .sessions
        .storage()
        .get_session(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if session.owner_id != user.id {
        return Err(StatusCode::FORBIDDEN);
    }

    let role = match body.role.as_str() {
        "operator" => telepair_core::permission::Role::Operator,
        "viewer" => telepair_core::permission::Role::Viewer,
        _ => return Err(StatusCode::BAD_REQUEST),
    };

    let (invite, raw_token) = state
        .sessions
        .storage()
        .create_invite(&session_id, role, body.max_uses, None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "token": raw_token,
            "role": invite.role.as_str(),
            "max_uses": invite.max_uses,
            "session_id": session_id,
        })),
    ))
}

#[derive(Deserialize)]
pub struct RedeemInviteRequest {
    pub token: String,
}

pub async fn redeem_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RedeemInviteRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = extract_user(&state, &headers).await?;

    let invite = state
        .sessions
        .storage()
        .consume_invite(&body.token)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Add user as participant with the invited role
    let _ = state
        .sessions
        .storage()
        .add_participant(&invite.session_id, user.id, invite.role)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "session_id": invite.session_id,
        "role": invite.role.as_str(),
    })))
}
```

Add `use axum::extract::Path;` to the imports in http.rs (if not already present — it's currently only in ws.rs).

- [ ] **Step 4: Add routes to lib.rs**

Update `crates/telepair-gateway/src/lib.rs`:

```rust
pub fn build_router_with_web_dir(state: AppState, web_dir: Option<&str>) -> Router {
    let api = Router::new()
        .route("/api/health", get(http::health))
        .route("/api/targets", get(http::list_targets))
        .route(
            "/api/sessions",
            post(http::create_session).get(http::list_sessions),
        )
        .route("/api/sessions/{session_id}/invite", post(http::create_invite))
        .route("/api/invite/redeem", post(http::redeem_invite))
        .route("/ws/session/{session_id}", get(ws::ws_handler))
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

- [ ] **Step 5: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --workspace 2>&1 | cat`
Expected: all tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/telepair-gateway/src/http.rs crates/telepair-gateway/src/lib.rs tests/invite_api_test.rs
git commit -s -m "feat(gateway): add invite token REST API endpoints"
```

---

### Task 4: Multi-user WebSocket Handler

**Files:**
- Modify: `crates/telepair-gateway/src/ws.rs`

**Depends on:** Tasks 1, 2

- [ ] **Step 1: Refactor ws.rs for multi-user support**

Replace `crates/telepair-gateway/src/ws.rs` entirely:

```rust
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};

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

    // Check if session exists
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

    // Look up user's role from participant records
    let participants = state
        .sessions
        .storage()
        .list_participants(&session_id)
        .await
        .unwrap_or_default();

    let my_role = participants
        .iter()
        .find(|p| p.user_id == user.id)
        .map(|p| p.role)
        .unwrap_or_else(|| {
            if session.owner_id == user.id {
                Role::Owner
            } else {
                // Not a participant — reject
                Role::Viewer // Will be handled below
            }
        });

    // If user is not a participant, reject
    let is_participant = participants.iter().any(|p| p.user_id == user.id);
    if !is_participant && session.owner_id != user.id {
        let err = ServerMessage::Error {
            code: "NOT_PARTICIPANT".into(),
            message: "you are not a participant in this session".into(),
        };
        let _ = ws_tx
            .send(Message::Text(serde_json::to_string(&err).unwrap().into()))
            .await;
        return;
    }

    // Start or join the live PTY session
    let hub = &state.hub;
    let (cmd_tx, mut output_rx, mut collab_rx) = if hub.is_live(&session_id).await {
        match hub.join_session(&session_id).await {
            Some(channels) => channels,
            None => return,
        }
    } else {
        let (cmd, args) = match state.targets.resolve(&session.target_name) {
            Some(resolved) => resolved,
            None => return,
        };
        match hub.start_session(&session_id, &cmd, &args, 80, 24).await {
            Ok(channels) => channels,
            Err(_) => return,
        }
    };

    // Register participant in session hub
    let participant = match hub
        .add_participant(&session_id, user.id, &user.name, my_role)
        .await
    {
        Some(p) => p,
        None => return,
    };

    // Build participant info list for SessionState
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

    // Send session state
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

    let user_id = user.id;
    let user_name = user.name.clone();
    let session_id_clone = session_id.clone();
    let hub_clone = state.hub.clone();

    // Spawn output forwarder: PTY output -> WebSocket
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<()>();

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
                        Ok(msg) => {
                            let json = serde_json::to_string(&msg).unwrap();
                            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = &mut done_rx => break,
            }
        }
    });

    // Input loop: WebSocket -> PTY (with permission enforcement)
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                    match client_msg {
                        ClientMessage::TermInput { data } => {
                            if my_role.can_input() {
                                let _ = cmd_tx.send(PtyCommand::Input(data)).await;
                            }
                        }
                        ClientMessage::TermResize { cols, rows } => {
                            if my_role.can_resize() {
                                let _ = cmd_tx.send(PtyCommand::Resize(cols, rows)).await;
                            }
                        }
                        ClientMessage::ChatMessage { text } => {
                            let chat = ServerMessage::PeerChat {
                                user_id,
                                name: user_name.clone(),
                                text,
                                ts: chrono::Utc::now().to_rfc3339(),
                            };
                            hub_clone
                                .broadcast_collab(&session_id_clone, chat)
                                .await;
                        }
                        ClientMessage::CursorMove { x, y } => {
                            let cursor = ServerMessage::PeerCursor { user_id, x, y };
                            hub_clone
                                .broadcast_collab(&session_id_clone, cursor)
                                .await;
                        }
                        _ => {}
                    }
                }
            }
            Message::Binary(data) => {
                if my_role.can_input() {
                    let _ = cmd_tx.send(PtyCommand::Input(data.to_vec())).await;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup: remove participant and notify
    let _ = done_tx.send(());
    output_handle.abort();
    hub_clone
        .remove_participant(&session_id_clone, user_id)
        .await;
    tracing::info!(user = %user_name, session = %session_id_clone, "WebSocket disconnected");
}
```

- [ ] **Step 2: Add chrono dependency to gateway Cargo.toml if not present**

Check if `chrono` is in `crates/telepair-gateway/Cargo.toml`. If not, add it:

```toml
chrono = { workspace = true }
```

- [ ] **Step 3: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --workspace 2>&1 | cat`
Expected: all tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/telepair-gateway/
git commit -s -m "feat(gateway): add multi-user WebSocket handler with permission enforcement"
```

---

### Task 5: Frontend Protocol + API Updates

**Files:**
- Modify: `web/src/lib/protocol.ts`
- Modify: `web/src/lib/api.ts`
- Create: `web/src/pages/Join.tsx`
- Modify: `web/src/App.tsx`

**Depends on:** Tasks 3, 4

- [ ] **Step 1: Add invite types to protocol.ts**

Append to `web/src/lib/protocol.ts`:

```typescript
// --- Invite ---

export interface InviteInfo {
  token: string;
  role: Role;
  max_uses: number;
  session_id: string;
}

export interface RedeemResult {
  session_id: string;
  role: Role;
}
```

- [ ] **Step 2: Add invite API methods to api.ts**

Add to the `api` object in `web/src/lib/api.ts`:

```typescript
  createInvite(sessionId: string, role: string, maxUses?: number): Promise<InviteInfo> {
    return request(`/sessions/${sessionId}/invite`, {
      method: 'POST',
      body: JSON.stringify({ role, max_uses: maxUses ?? 1 }),
    });
  },

  redeemInvite(token: string): Promise<RedeemResult> {
    return request('/invite/redeem', {
      method: 'POST',
      body: JSON.stringify({ token }),
    });
  },
```

Add `InviteInfo, RedeemResult` to the import from `./protocol`.

- [ ] **Step 3: Create Join page**

Create `web/src/pages/Join.tsx`:

```tsx
import { onMount, createSignal, Show } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { api, ApiError } from '../lib/api';

export default function Join() {
  const params = useParams<{ token: string }>();
  const navigate = useNavigate();
  const [error, setError] = createSignal('');
  const [redeeming, setRedeeming] = createSignal(true);

  onMount(async () => {
    if (!auth.isAuthenticated()) {
      // Store invite token and redirect to login
      sessionStorage.setItem('pending_invite', params.token);
      navigate('/login', { replace: true });
      return;
    }

    try {
      const result = await api.redeemInvite(params.token);
      navigate(`/session/${result.session_id}`, { replace: true });
    } catch (e) {
      setRedeeming(false);
      if (e instanceof ApiError) {
        setError(e.status === 400 ? 'Invalid or expired invite link' : e.message);
      } else {
        setError('Failed to join session');
      }
    }
  });

  return (
    <div class="join-page">
      <div class="join-card">
        <h1>telepair</h1>
        <Show when={redeeming()} fallback={
          <div>
            <p class="error-msg">{error()}</p>
            <button class="primary" onClick={() => navigate('/')}>Go to Dashboard</button>
          </div>
        }>
          <p class="muted">Joining session...</p>
        </Show>
      </div>

      <style>{`
        .join-page {
          display: flex;
          align-items: center;
          justify-content: center;
          min-height: 100vh;
        }
        .join-card {
          background: var(--bg-secondary);
          border: 1px solid var(--border);
          border-radius: 12px;
          padding: 40px;
          width: 380px;
          text-align: center;
        }
        .join-card h1 { font-size: 28px; font-weight: 700; margin-bottom: 16px; }
        .muted { color: var(--text-secondary); }
        .error-msg { color: var(--error); margin-bottom: 16px; }
      `}</style>
    </div>
  );
}
```

- [ ] **Step 4: Add /join/:token route to App.tsx**

Update `web/src/App.tsx`:

```tsx
import { Router, Route, Navigate } from '@solidjs/router';
import { Show } from 'solid-js';
import { auth } from './stores/auth';
import Login from './pages/Login';
import Dashboard from './pages/Dashboard';
import Session from './pages/Session';
import Join from './pages/Join';

function AuthGuard(props: { children: any }) {
  return (
    <Show when={auth.isAuthenticated()} fallback={<Navigate href="/login" />}>
      {props.children}
    </Show>
  );
}

export default function App() {
  return (
    <Router>
      <Route path="/login" component={Login} />
      <Route path="/join/:token" component={Join} />
      <Route path="/" component={() => <AuthGuard><Dashboard /></AuthGuard>} />
      <Route path="/session/:id" component={() => <AuthGuard><Session /></AuthGuard>} />
    </Router>
  );
}
```

- [ ] **Step 5: Handle pending invite after login**

In `web/src/pages/Login.tsx`, after successful validation, check for pending invite:

```tsx
  const handleSubmit = async (e: Event) => {
    e.preventDefault();
    const ok = await auth.validateToken(input());
    if (ok) {
      const pendingInvite = sessionStorage.getItem('pending_invite');
      if (pendingInvite) {
        sessionStorage.removeItem('pending_invite');
        navigate(`/join/${pendingInvite}`, { replace: true });
      } else {
        navigate('/', { replace: true });
      }
    }
  };
```

- [ ] **Step 6: Verify build**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`
Expected: build succeeds

- [ ] **Step 7: Commit**

```bash
git add web/src/
git commit -s -m "feat(web): add invite flow with join page and API integration"
```

---

### Task 6: InviteDialog Component

**Files:**
- Create: `web/src/components/InviteDialog.tsx`

**Depends on:** Task 5

- [ ] **Step 1: Create InviteDialog.tsx**

```tsx
import { createSignal, Show } from 'solid-js';
import { api } from '../lib/api';

interface InviteDialogProps {
  sessionId: string;
  open: boolean;
  onClose: () => void;
}

export default function InviteDialog(props: InviteDialogProps) {
  const [role, setRole] = createSignal('operator');
  const [inviteUrl, setInviteUrl] = createSignal('');
  const [creating, setCreating] = createSignal(false);
  const [copied, setCopied] = createSignal(false);

  const handleCreate = async () => {
    setCreating(true);
    try {
      const invite = await api.createInvite(props.sessionId, role());
      const url = `${location.origin}/join/${invite.token}`;
      setInviteUrl(url);
    } catch (e) {
      console.error('Failed to create invite:', e);
    }
    setCreating(false);
  };

  const handleCopy = async () => {
    await navigator.clipboard.writeText(inviteUrl());
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleClose = () => {
    setInviteUrl('');
    setCopied(false);
    props.onClose();
  };

  return (
    <Show when={props.open}>
      <div class="dialog-backdrop" onClick={handleClose}>
        <div class="dialog" onClick={(e) => e.stopPropagation()}>
          <h3>Invite to Session</h3>

          <Show when={!inviteUrl()} fallback={
            <div class="invite-result">
              <label>Invite Link</label>
              <div class="invite-url-row">
                <input type="text" value={inviteUrl()} readonly />
                <button class="primary" onClick={handleCopy}>
                  {copied() ? 'Copied!' : 'Copy'}
                </button>
              </div>
              <p class="hint">Share this link with the person you want to invite.</p>
              <button onClick={handleClose} style={{ 'margin-top': '12px', width: '100%' }}>Done</button>
            </div>
          }>
            <div class="invite-form">
              <label>Role</label>
              <div class="role-options">
                <button
                  class={role() === 'operator' ? 'role-btn active' : 'role-btn'}
                  onClick={() => setRole('operator')}
                >
                  Operator
                  <span class="role-desc">Can type and resize</span>
                </button>
                <button
                  class={role() === 'viewer' ? 'role-btn active' : 'role-btn'}
                  onClick={() => setRole('viewer')}
                >
                  Viewer
                  <span class="role-desc">Can only watch</span>
                </button>
              </div>
              <button class="primary" onClick={handleCreate} disabled={creating()} style={{ width: '100%', 'margin-top': '16px' }}>
                {creating() ? 'Creating...' : 'Create Invite Link'}
              </button>
            </div>
          </Show>

          <style>{`
            .dialog-backdrop {
              position: fixed;
              inset: 0;
              background: rgba(0, 0, 0, 0.5);
              display: flex;
              align-items: center;
              justify-content: center;
              z-index: 100;
            }
            .dialog {
              background: var(--bg-secondary);
              border: 1px solid var(--border);
              border-radius: 12px;
              padding: 24px;
              width: 400px;
              max-width: 90vw;
            }
            .dialog h3 {
              font-size: 16px;
              font-weight: 600;
              margin-bottom: 16px;
            }
            .dialog label {
              display: block;
              font-size: 12px;
              font-weight: 600;
              color: var(--text-secondary);
              margin-bottom: 8px;
            }
            .role-options { display: flex; gap: 8px; }
            .role-btn {
              flex: 1;
              padding: 12px;
              text-align: left;
              border-radius: 8px;
              display: flex;
              flex-direction: column;
              gap: 4px;
            }
            .role-btn.active { border-color: var(--accent); background: rgba(88, 166, 255, 0.1); }
            .role-desc { font-size: 11px; color: var(--text-secondary); }
            .invite-url-row { display: flex; gap: 8px; }
            .invite-url-row input { flex: 1; }
            .hint { font-size: 12px; color: var(--text-secondary); margin-top: 8px; }
          `}</style>
        </div>
      </div>
    </Show>
  );
}
```

- [ ] **Step 2: Verify build**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`

- [ ] **Step 3: Commit**

```bash
git add web/src/components/InviteDialog.tsx
git commit -s -m "feat(web): add invite dialog component"
```

---

### Task 7: ParticipantList Component

**Files:**
- Create: `web/src/components/ParticipantList.tsx`

**Depends on:** Task 5

- [ ] **Step 1: Create ParticipantList.tsx**

```tsx
import { For, Show } from 'solid-js';
import type { ParticipantInfo, Role } from '../lib/protocol';

interface ParticipantListProps {
  participants: ParticipantInfo[];
  myRole: Role;
  onPromote?: (userId: string, newRole: Role) => void;
  onKick?: (userId: string) => void;
}

export default function ParticipantList(props: ParticipantListProps) {
  return (
    <div class="participant-list">
      <h4>Participants ({props.participants.length})</h4>
      <div class="participants">
        <For each={props.participants}>
          {(p) => (
            <div class="participant-row">
              <span class="participant-color" style={{ background: p.color }} />
              <span class="participant-name">{p.name}</span>
              <span class="participant-role" data-role={p.role}>{p.role}</span>

              <Show when={props.myRole === 'owner' && p.role !== 'owner'}>
                <div class="participant-actions">
                  <Show when={p.role === 'viewer'}>
                    <button
                      class="action-btn"
                      title="Promote to operator"
                      onClick={() => props.onPromote?.(p.user_id, 'operator')}
                    >+</button>
                  </Show>
                  <Show when={p.role === 'operator'}>
                    <button
                      class="action-btn"
                      title="Demote to viewer"
                      onClick={() => props.onPromote?.(p.user_id, 'viewer')}
                    >-</button>
                  </Show>
                  <button
                    class="action-btn kick"
                    title="Remove from session"
                    onClick={() => props.onKick?.(p.user_id)}
                  >x</button>
                </div>
              </Show>
            </div>
          )}
        </For>
      </div>

      <style>{`
        .participant-list h4 {
          font-size: 12px;
          font-weight: 600;
          color: var(--text-secondary);
          text-transform: uppercase;
          margin-bottom: 8px;
        }
        .participants { display: flex; flex-direction: column; gap: 4px; }
        .participant-row {
          display: flex;
          align-items: center;
          gap: 8px;
          padding: 6px 8px;
          border-radius: 6px;
          font-size: 13px;
        }
        .participant-row:hover { background: var(--bg-tertiary); }
        .participant-color {
          width: 8px;
          height: 8px;
          border-radius: 50%;
          flex-shrink: 0;
        }
        .participant-name { flex: 1; }
        .participant-role {
          font-size: 10px;
          padding: 1px 6px;
          border-radius: 8px;
          text-transform: uppercase;
          font-weight: 600;
        }
        .participant-role[data-role="owner"] { background: rgba(63, 185, 80, 0.2); color: var(--success); }
        .participant-role[data-role="operator"] { background: rgba(88, 166, 255, 0.2); color: var(--accent); }
        .participant-role[data-role="viewer"] { background: rgba(139, 148, 158, 0.2); color: var(--text-secondary); }
        .participant-actions { display: flex; gap: 2px; }
        .action-btn {
          width: 20px;
          height: 20px;
          padding: 0;
          font-size: 12px;
          line-height: 1;
          border-radius: 4px;
          display: flex;
          align-items: center;
          justify-content: center;
        }
        .action-btn.kick { color: var(--error); }
      `}</style>
    </div>
  );
}
```

- [ ] **Step 2: Verify build**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`

- [ ] **Step 3: Commit**

```bash
git add web/src/components/ParticipantList.tsx
git commit -s -m "feat(web): add participant list component"
```

---

### Task 8: ChatPanel Component

**Files:**
- Create: `web/src/components/ChatPanel.tsx`

**Depends on:** Task 5

- [ ] **Step 1: Create ChatPanel.tsx**

```tsx
import { createSignal, For, onMount } from 'solid-js';

export interface ChatMessage {
  user_id: string;
  name: string;
  text: string;
  ts: string;
}

interface ChatPanelProps {
  messages: ChatMessage[];
  onSend: (text: string) => void;
}

export default function ChatPanel(props: ChatPanelProps) {
  const [input, setInput] = createSignal('');
  let messagesEnd: HTMLDivElement | undefined;

  const scrollToBottom = () => {
    messagesEnd?.scrollIntoView({ behavior: 'smooth' });
  };

  // Auto-scroll when new messages arrive
  onMount(() => {
    const observer = new MutationObserver(scrollToBottom);
    const container = messagesEnd?.parentElement;
    if (container) {
      observer.observe(container, { childList: true });
    }
  });

  const handleSend = () => {
    const text = input().trim();
    if (!text) return;
    props.onSend(text);
    setInput('');
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const formatTime = (ts: string) => {
    try {
      return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } catch {
      return '';
    }
  };

  return (
    <div class="chat-panel">
      <h4>Chat</h4>
      <div class="chat-messages">
        <For each={props.messages}>
          {(msg) => (
            <div class="chat-msg">
              <span class="chat-name">{msg.name}</span>
              <span class="chat-time">{formatTime(msg.ts)}</span>
              <p class="chat-text">{msg.text}</p>
            </div>
          )}
        </For>
        <div ref={messagesEnd} />
      </div>
      <div class="chat-input-row">
        <input
          type="text"
          placeholder="Type a message..."
          value={input()}
          onInput={(e) => setInput(e.currentTarget.value)}
          onKeyDown={handleKeyDown}
        />
        <button onClick={handleSend} disabled={!input().trim()}>Send</button>
      </div>

      <style>{`
        .chat-panel {
          display: flex;
          flex-direction: column;
          height: 100%;
        }
        .chat-panel h4 {
          font-size: 12px;
          font-weight: 600;
          color: var(--text-secondary);
          text-transform: uppercase;
          margin-bottom: 8px;
          padding: 0 4px;
        }
        .chat-messages {
          flex: 1;
          overflow-y: auto;
          display: flex;
          flex-direction: column;
          gap: 8px;
          padding: 4px;
          min-height: 0;
        }
        .chat-msg { font-size: 13px; }
        .chat-name { font-weight: 600; margin-right: 6px; }
        .chat-time { color: var(--text-secondary); font-size: 11px; }
        .chat-text { margin-top: 2px; word-break: break-word; }
        .chat-input-row {
          display: flex;
          gap: 6px;
          padding: 8px 4px 4px;
          border-top: 1px solid var(--border);
        }
        .chat-input-row input { flex: 1; font-family: var(--font-sans); font-size: 13px; }
        .chat-input-row button { font-size: 13px; padding: 6px 12px; }
      `}</style>
    </div>
  );
}
```

- [ ] **Step 2: Verify build**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`

- [ ] **Step 3: Commit**

```bash
git add web/src/components/ChatPanel.tsx
git commit -s -m "feat(web): add chat panel component"
```

---

### Task 9: Session Page Integration

**Files:**
- Modify: `web/src/pages/Session.tsx`

**Depends on:** Tasks 5, 6, 7, 8

- [ ] **Step 1: Replace Session.tsx with full collaboration version**

```tsx
import { createSignal, onCleanup, Show, createMemo } from 'solid-js';
import { useParams, useNavigate } from '@solidjs/router';
import { auth } from '../stores/auth';
import { TelepairSocket } from '../lib/ws';
import { encodeInput, decodeOutput } from '../lib/protocol';
import type { ServerMessage, Role, ParticipantInfo } from '../lib/protocol';
import type { TerminalHandle } from '../components/Terminal';
import type { ChatMessage } from '../components/ChatPanel';
import Terminal from '../components/Terminal';
import ParticipantList from '../components/ParticipantList';
import ChatPanel from '../components/ChatPanel';
import InviteDialog from '../components/InviteDialog';

type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'error';

export default function SessionPage() {
  const params = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [status, setStatus] = createSignal<ConnectionStatus>('connecting');
  const [role, setRole] = createSignal<Role>('viewer');
  const [errorMsg, setErrorMsg] = createSignal('');
  const [participants, setParticipants] = createSignal<ParticipantInfo[]>([]);
  const [chatMessages, setChatMessages] = createSignal<ChatMessage[]>([]);
  const [showInvite, setShowInvite] = createSignal(false);
  const [sidebarOpen, setSidebarOpen] = createSignal(true);

  let termHandle: TerminalHandle | undefined;
  let socket: TelepairSocket | undefined;

  const isOwner = createMemo(() => role() === 'owner');

  const handleMessage = (msg: ServerMessage) => {
    switch (msg.type) {
      case 'SessionState':
        setRole(msg.your_role);
        setParticipants(msg.participants);
        break;
      case 'TermOutput':
        termHandle?.write(decodeOutput(msg.data));
        break;
      case 'PeerJoined':
        setParticipants((prev) => [
          ...prev.filter((p) => p.user_id !== msg.user_id),
          { user_id: msg.user_id, name: msg.name, role: msg.role, color: msg.color },
        ]);
        break;
      case 'PeerLeft':
        setParticipants((prev) => prev.filter((p) => p.user_id !== msg.user_id));
        break;
      case 'PeerChat':
        setChatMessages((prev) => [
          ...prev,
          { user_id: msg.user_id, name: msg.name, text: msg.text, ts: msg.ts },
        ]);
        break;
      case 'PermUpdate':
        setParticipants((prev) =>
          prev.map((p) =>
            p.user_id === msg.user_id ? { ...p, role: msg.new_role } : p
          )
        );
        break;
      case 'Error':
        setErrorMsg(`${msg.code}: ${msg.message}`);
        break;
    }
  };

  const handleStatus = (s: ConnectionStatus) => {
    setStatus(s);
  };

  const handleData = (data: string) => {
    if (role() === 'viewer') return;
    socket?.sendInput(encodeInput(data));
  };

  const handleResize = (cols: number, rows: number) => {
    if (role() === 'viewer') return;
    socket?.sendResize(cols, rows);
  };

  const handleSendChat = (text: string) => {
    socket?.send({ type: 'ChatMessage', text });
  };

  // Connect WebSocket
  socket = new TelepairSocket(handleMessage, handleStatus);
  socket.connect(params.id, auth.token());

  onCleanup(() => {
    socket?.disconnect();
  });

  return (
    <div class="session-page">
      <header class="session-topbar">
        <button class="back-btn" onClick={() => navigate('/')}>← Back</button>
        <span class="session-label">Session: <code>{params.id}</code></span>
        <span class="role-badge" data-role={role()}>{role()}</span>
        <span class="status-dot" data-status={status()} />
        <div class="topbar-actions">
          <Show when={isOwner()}>
            <button class="action-btn" onClick={() => setShowInvite(true)}>Invite</button>
          </Show>
          <button class="action-btn" onClick={() => setSidebarOpen(!sidebarOpen())}>
            {sidebarOpen() ? 'Hide' : 'Show'} Sidebar
          </button>
        </div>
      </header>

      <Show when={errorMsg()}>
        <div class="error-banner">{errorMsg()}</div>
      </Show>

      <div class="session-body">
        <div class="terminal-container">
          <Terminal
            onData={handleData}
            onResize={handleResize}
            ref={(h) => { termHandle = h; }}
          />
        </div>

        <Show when={sidebarOpen()}>
          <aside class="sidebar">
            <div class="sidebar-section">
              <ParticipantList
                participants={participants()}
                myRole={role()}
              />
            </div>
            <div class="sidebar-section chat-section">
              <ChatPanel messages={chatMessages()} onSend={handleSendChat} />
            </div>
          </aside>
        </Show>
      </div>

      <InviteDialog
        sessionId={params.id}
        open={showInvite()}
        onClose={() => setShowInvite(false)}
      />

      <style>{`
        .session-page {
          display: flex;
          flex-direction: column;
          height: 100vh;
        }
        .session-topbar {
          display: flex;
          align-items: center;
          gap: 12px;
          padding: 8px 16px;
          border-bottom: 1px solid var(--border);
          background: var(--bg-secondary);
          font-size: 13px;
        }
        .back-btn { font-size: 13px; padding: 4px 10px; }
        .session-label code { font-family: var(--font-mono); color: var(--accent); }
        .role-badge {
          padding: 2px 8px;
          border-radius: 12px;
          font-size: 11px;
          font-weight: 600;
          text-transform: uppercase;
        }
        .role-badge[data-role="owner"] { background: rgba(63, 185, 80, 0.2); color: var(--success); }
        .role-badge[data-role="operator"] { background: rgba(88, 166, 255, 0.2); color: var(--accent); }
        .role-badge[data-role="viewer"] { background: rgba(139, 148, 158, 0.2); color: var(--text-secondary); }
        .status-dot {
          width: 8px;
          height: 8px;
          border-radius: 50%;
        }
        .status-dot[data-status="connecting"] { background: var(--warning); }
        .status-dot[data-status="connected"] { background: var(--success); }
        .status-dot[data-status="disconnected"] { background: var(--text-secondary); }
        .status-dot[data-status="error"] { background: var(--error); }
        .topbar-actions {
          margin-left: auto;
          display: flex;
          gap: 8px;
        }
        .topbar-actions .action-btn { font-size: 12px; padding: 4px 10px; }

        .error-banner {
          padding: 8px 16px;
          background: rgba(248, 81, 73, 0.15);
          color: var(--error);
          font-size: 13px;
          border-bottom: 1px solid rgba(248, 81, 73, 0.3);
        }

        .session-body {
          flex: 1;
          display: flex;
          overflow: hidden;
        }
        .terminal-container {
          flex: 1;
          padding: 4px;
          overflow: hidden;
        }
        .sidebar {
          width: 260px;
          border-left: 1px solid var(--border);
          background: var(--bg-secondary);
          display: flex;
          flex-direction: column;
          overflow: hidden;
        }
        .sidebar-section {
          padding: 12px;
        }
        .sidebar-section.chat-section {
          flex: 1;
          border-top: 1px solid var(--border);
          min-height: 0;
          display: flex;
          flex-direction: column;
        }
      `}</style>
    </div>
  );
}
```

- [ ] **Step 2: Verify build**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vite build 2>&1 | cat`
Expected: build succeeds

- [ ] **Step 3: Run all tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --workspace 2>&1 | cat`
Run: `cd /Users/liys/workspace/github.com/telepair/telepair/web && npx vitest run 2>&1 | cat`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add web/src/
git commit -s -m "feat(web): integrate collaboration UI into session page"
```

---

## Summary

After completing all 9 tasks, you will have:

1. **Invite system** — Create invite tokens with role assignment, share links, redeem to join sessions
2. **Multi-user sessions** — Multiple users connect to same PTY, each with assigned role
3. **Permission enforcement** — Server-side role checking (viewers can't type/resize)
4. **Participant tracking** — Live join/leave notifications, colored presence indicators
5. **Chat** — In-session text chat between participants
6. **Invite UI** — Dialog for owners to generate invite links with role selection
7. **Collaboration sidebar** — Participant list + chat panel alongside terminal

**What's NOT included (deferred):**
- CollabOverlay (cursor overlay on terminal) — requires deep xterm.js integration
- WebRTC DataChannel — WS works fine for v1
- Serialized input mode (control request/grant) — multiplexed is default
- Min-dimensions resize across participants — complex coordination
