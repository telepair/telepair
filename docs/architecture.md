# Architecture

telepair is built as a Cargo workspace with five crates, each owning a distinct layer. A single binary (`telepair`) composes these layers via role flags.

## Crate Dependency Graph

```
telepair-cli
├── telepair-gateway ──┬── telepair-core
│                      └── telepair-agent ── telepair-core
├── telepair-control ──── telepair-core
└── telepair-core
```

## Crate Responsibilities

### telepair-core

The foundation crate. Contains no business logic — only shared abstractions.

| Module | Purpose |
|--------|---------|
| `session.rs` | Domain types: `User`, `Session`, `Participant`, `InviteToken`, `InputMode`, `SessionStatus` |
| `permission.rs` | `Role` enum (Owner/Operator/Viewer) with capability methods (`can_input`, `can_resize`, `can_manage`) |
| `protocol.rs` | `ClientMessage`/`ServerMessage` enums (JSON, `#[serde(tag = "type")]`) + `BinaryFrame` (terminal I/O) |
| `storage.rs` | Async `Storage` trait — CRUD for users, sessions, participants, invite tokens |
| `storage/sqlite.rs` | `SqliteStorage` implementation using sqlx |
| `auth.rs` | `TokenAuthProvider` — bcrypt-based token validation |
| `config.rs` | `AppConfig` YAML deserialization |
| `target.rs` | `Target` and `TargetKind` definitions |
| `error.rs` | `Error` enum (Auth, NotFound, Storage, Internal) |

### telepair-agent

Manages PTY processes and virtual target resolution.

| Module | Purpose |
|--------|---------|
| `pty.rs` | `PtyManager` — spawns shell via portable-pty, handles read/write/resize |
| `virtual_target.rs` | `TargetEngine` — loads YAML config, resolves target names to commands, env var substitution |

### telepair-control

Business logic services that coordinate core abstractions.

| Module | Purpose |
|--------|---------|
| `session_service.rs` | `SessionService` — create/close sessions, manage participants, delegate to Storage |
| `target_service.rs` | `TargetService` — wraps TargetEngine, provides target listing and resolution |

### telepair-gateway

The client-facing layer. Runs the HTTP server, WebSocket upgrade, and serves the frontend.

| Module | Purpose |
|--------|---------|
| `lib.rs` | Axum router setup, route definitions |
| `state.rs` | `AppState` — shared application state (storage, auth, services, session hubs) |
| `http.rs` | REST handlers: health, targets, sessions, invites |
| `ws.rs` | WebSocket handler — auth, role enforcement, message routing, PTY I/O bridge |
| `session_hub.rs` | `SessionHub` — per-session state: PTY process, connected participants, broadcast channels |

### telepair-cli

Minimal binary crate. Parses CLI args, initializes storage, sets up tracing, starts the server.

## Runtime Architecture

```
Browser                     telepair (single process)
┌──────────┐               ┌─────────────────────────────────┐
│ SolidJS  │──── REST ────▶│  Gateway (axum)                 │
│ xterm.js │──── WS ──────▶│    ├── HTTP handlers            │
│          │               │    └── WS handler                │
└──────────┘               │         ├── SessionHub           │
                           │         │   ├── PTY (agent)      │
                           │         │   ├── output_tx (broadcast)
                           │         │   └── collab_tx (broadcast)
                           │         └── Permission enforcement│
                           │                                   │
                           │  Control (services)               │
                           │    ├── SessionService             │
                           │    └── TargetService              │
                           │                                   │
                           │  Core (storage)                   │
                           │    └── SQLite (sqlx)              │
                           └─────────────────────────────────┘
```

## Data Flow

### Terminal I/O

1. User types in xterm.js
2. Frontend encodes keystrokes as `TermInput { data: Vec<u8> }` (JSON) or binary frame (type 0x02)
3. WS handler checks `role.can_input()` — rejects if viewer
4. Sends bytes to PTY via `SessionHub` command channel
5. PTY output is broadcast via `output_tx` to all connected participants
6. WS handler forwards as `TermOutput { data: Vec<u8> }` to each client
7. Frontend decodes and writes to xterm.js

### Collaboration Messages

1. Client sends `ChatMessage { text }` via WS
2. WS handler wraps as `PeerChat { user_id, name, text, ts }` with server timestamp
3. `SessionHub` broadcasts via `collab_tx` to all participants
4. Each WS handler forwards to its client

### Session Lifecycle

1. Client calls `POST /api/sessions` with target name
2. `SessionService` creates session in SQLite, adds owner as participant
3. Owner connects via `WS /ws/session/{id}`, sends `SessionJoin`
4. `SessionHub` spawns PTY, starts I/O loop
5. Owner creates invite via `POST /api/sessions/{id}/invite`
6. Collaborator redeems invite via `POST /api/invite/redeem`
7. Collaborator connects to same WS endpoint, `PeerJoined` broadcast to all

## Broadcast Channels

Each live session has two independent broadcast channels:

| Channel | Capacity | Content |
|---------|----------|---------|
| `output_tx` | 256 messages | `TermOutput` — PTY bytes |
| `collab_tx` | 64 messages | `PeerJoined`, `PeerLeft`, `PeerChat`, `PeerCursor`, `PermUpdate` |

Separation ensures high-frequency terminal output does not starve collaboration messages. Both use `tokio::broadcast` — slow receivers lose oldest messages.

## Storage Schema

```sql
users (id, name, token_hash, is_admin, created_at, updated_at)
sessions (id, owner_id, target_name, input_mode, status, created_at, closed_at)
participants (session_id, user_id, role, joined_at, left_at)
invite_tokens (token_hash, session_id, role, max_uses, used_count, expires_at)
```

All IDs are UUIDs stored as TEXT. Timestamps are ISO 8601 TEXT. The `Storage` trait is async and implementation-agnostic — SQLite is the v1 backend.

## Security Model

- **Authentication**: Bearer token in `Authorization` header. Tokens are bcrypt-hashed in storage, never stored in plaintext.
- **Authorization**: Role-based per session. WS handler checks role on every input/resize action.
- **Invite tokens**: Single-use by default. Hashed with bcrypt. Atomic consumption prevents concurrent redemption race conditions.
- **No CORS**: Frontend is served from the same origin. API proxy in dev mode.
