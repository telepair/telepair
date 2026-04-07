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
| `protocol.rs` | `ClientMessage`/`ServerMessage` enums (JSON, `#[serde(tag = "type")]`); PTY output is sent as raw binary WS frames |
| `storage.rs` | Async `Storage` trait — CRUD for users, sessions, participants, invite tokens |
| `storage/sqlite.rs` | `SqliteStorage` implementation using sqlx |
| `auth.rs` | `TokenAuthProvider` — SHA-256 hashed token validation (raw token returned once at creation, never persisted) |
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
2. Frontend sends the raw UTF-8 bytes as a **binary WebSocket frame** (no JSON wrapper)
3. WS handler checks `role.can_input()` — silently drops if viewer
4. Sends bytes to PTY via `SessionHub` command channel
5. PTY output is broadcast via `output_tx` to all connected participants
6. WS handler forwards each chunk as a raw binary WS frame
7. Frontend writes the bytes directly to xterm.js

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
| `output_tx` | 256 messages | PTY bytes (forwarded as raw binary WS frames) |
| `collab_tx` | 64 messages | `PeerJoined`, `PeerLeft`, `PeerChat`, `PeerCursor` |

Separation ensures high-frequency terminal output does not starve collaboration messages. Both use `tokio::broadcast` — slow receivers lose oldest messages.

## Storage Schema

```sql
users (id, name, token_sha256, is_admin, created_at, updated_at)
sessions (id, owner_id, target_name, input_mode, status, created_at, closed_at)
participants (session_id, user_id, role, joined_at, left_at)
invite_tokens (token_sha256, session_id, role, max_uses, used_count, expires_at)
```

All IDs are UUIDs stored as TEXT. Timestamps are ISO 8601 TEXT. The `Storage` trait is async and implementation-agnostic — SQLite is the v1 backend.

## Security Model

- **Authentication**: Bearer token in `Authorization` header. Tokens are stored as their SHA-256 hex digest only — the raw value is returned to the caller exactly once at creation and never persisted.
- **Authorization**: Role-based per session. WS handler checks role on every input/resize action.
- **Invite tokens**: Single-use by default. Stored as SHA-256 digests; atomic `used_count < max_uses` increment prevents concurrent redemption race conditions.
- **CORS**: Configurable via `--allowed-origins` (comma-separated absolute URLs). When unset, the server defaults to **loopback dev origins only** (`http://localhost:5173`, `http://127.0.0.1:5173`) to match the Vite dev server. Malformed origins are fatal at startup so a typo can never silently widen the allowlist. `--allow-any-origin` opts into `Access-Control-Allow-Origin: *` and is only safe in dev or behind a reverse proxy that enforces CORS. For production direct-exposure (no reverse proxy), set `--allowed-origins` to the trusted frontend domain.
