English | [简体中文](architecture.zh-CN.md)

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
| `session.rs` | Domain types: `User`, `Session`, `Participant`, `InviteToken`, `InputMode`, `SessionStatus`, `CloseReason` |
| `permission.rs` | `Role` enum (Owner/Operator/Viewer) with capability methods (`can_input`, `can_resize`, `can_manage`) |
| `protocol.rs` | `ClientMessage`/`ServerMessage` enums (JSON, `#[serde(tag = "type")]`); PTY output is sent as raw binary WS frames |
| `storage.rs` | Async `Storage` trait — CRUD for users, sessions, participants, invite tokens, audit events |
| `storage/sqlite.rs` | `SqliteStorage` implementation using sqlx, with boot-time `run_migrations()` that handles idempotent column / table additions |
| `auth.rs` | `TokenAuthProvider` — SHA-256 hashed token validation (raw token returned once at creation, never persisted) |
| `target.rs` | `Target` and `TargetKind` definitions |
| `audit.rs` | `AuditEvent`, `AuditEventType`, and `AuditSink` trait — append-only event log backing `telepair admin audit` and the in-app session timeline |
| `error.rs` | `Error` enum — Auth (401), SessionNotFound/TargetNotFound (404), SessionClosed (410), PermissionDenied (403), InvalidInput (400), Conflict (409), RateLimited (429), ServiceUnavailable (503), Internal/Storage/Io (500). Each variant carries an `http_status()` method for consistent HTTP mapping. |

### telepair-agent

Manages PTY processes and virtual target resolution.

| Module | Purpose |
|--------|---------|
| `pty.rs` | `PtyManager` — spawns shell via portable-pty, handles read/write/resize |
| `virtual_target.rs` | `TargetEngine` — loads YAML config, resolves target names to commands, env var substitution |

### telepair-control

Business logic services that coordinate core abstractions. As of 0.1.1 this is the **only** layer that touches the `Storage` trait in production code — the gateway goes through services for every read and write. This keeps HTTP handlers and the WebSocket hub free of business rules and makes the services unit-testable without any HTTP plumbing.

| Module | Purpose |
|--------|---------|
| `session_service.rs` | `SessionService` — session lifecycle (`create_session`, `close_session(reason)`), participant queries (`list_participants`, `list_sessions_for_user`), authorization helpers (`require_owner`), and cross-layer aggregates like `active_session_counts_per_target`. Emits audit events for create/close and the startup sweep. |
| `invite_service.rs` | `InviteService` — invite lifecycle (`create`, `redeem`, `list_for_session`, `revoke`). Owns `MAX_INVITE_USES` / TTL validation, the cross-session scoped-guest check, and the guest mint-on-success path. Emits `invite.minted` / `invite.redeemed` / `invite.revoked` audit events. |
| `auth_service.rs` | `AuthService` — email-based registration with OTP verification, password login with Argon2 hashing, password change with atomic token rotation, and admin user management (`list_accounts`, `set_session_access`). Handles SMTP transport for OTP delivery (via lettre), login throttling (5-strike lockout with 15-minute window), server-side password-length validation, and enumeration-safe error collapsing. Emits `auth.register_rejected` / `auth.register_completed` / `auth.verify_failed` / `auth.login_failed` / `auth.password_changed` / `auth.user_enabled` / `auth.user_disabled` audit events. |
| `user_target_service.rs` | `UserTargetService` — CRUD for user-owned targets (`create`, `update`, `delete`, `get`, `list`, `resolve_by_id`). Enforces ownership on every mutation, blocks update/delete while an active session references the target (referential integrity via `Conflict` error), and deliberately skips `${VAR}` expansion on resolve to prevent process-env leakage through user-supplied command strings. |
| `target_service.rs` | `TargetService` — wraps `TargetEngine`, provides target listing and resolution. |

### telepair-gateway

The client-facing layer. Runs the HTTP server, WebSocket upgrade, and serves the frontend.

| Module | Purpose |
|--------|---------|
| `lib.rs` | Axum router setup, route definitions |
| `state.rs` | `AppState` — shared application state: storage, auth, `SessionService`, `InviteService`, `AuthService`, `UserTargetService`, `Arc<ArcSwap<TargetEngine>>` (for atomic target hot-reload), `Arc<dyn AuditSink>`, and the `SessionHub` |
| `http.rs` | REST handlers: health, targets, sessions, participant role change, invites (list / revoke), session history, session audit, whoami, change password, admin targets (list + reload), admin users, admin audit. All handlers go through services — no `.storage()` access in production code. |
| `ws.rs` | WebSocket handler — auth, role enforcement, message routing, PTY I/O bridge, `participant.joined` / `participant.left` audit emits |
| `session_hub.rs` | `SessionHub` — per-session state: PTY process, connected participants, broadcast channels. Holds `Arc<SessionService>` (not raw Storage) so the reaper closure emits `CloseReason::Reaper` through the same audit path as owner-initiated closes. |

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
                           │    ├── AuthService                │
                           │    ├── UserTargetService          │
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

### Email Registration & Login

1. Client calls `POST /api/auth/register` with email, password, and display name
2. `AuthService` hashes the password with Argon2, generates a 6-digit OTP, writes a `pending_registrations` row, and sends the OTP via SMTP
3. Client calls `POST /api/auth/verify` with email and OTP code
4. `AuthService` verifies the code against the pending row (with attempt limiting and TTL), materializes a `users` row, and returns a bearer token
5. For subsequent logins, client calls `POST /api/auth/login` with email and password
6. `AuthService` verifies the Argon2 hash, enforces the 5-strike lockout window, clears failure counters on success, and returns a fresh bearer token
7. Admin can disable/enable a user's session access via `PUT /api/admin/users/{id}/session-access` — login still works (for password reset, history viewing) but session creation and WS attach are blocked

### Session Lifecycle

1. Client calls `POST /api/sessions` with target name (or user target ID)
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
| `collab_tx` | 64 messages | `PeerJoined`, `PeerLeft`, `PeerChat`, `PeerCursor`, `PeerRoleChanged` |

Separation ensures high-frequency terminal output does not starve collaboration messages. Both use `tokio::broadcast` — slow receivers lose oldest messages.

## Storage Schema

```sql
users                 (id, name, token_sha256, is_admin, scoped_session_id,
                       email, password_hash, session_enabled,
                       login_failed_count, login_locked_until,
                       created_at, updated_at)
sessions              (id, owner_id, target_name, input_mode, status,
                       closed_reason, user_target_id, created_at, closed_at)
participants          (session_id, user_id, role, joined_at, left_at)
invite_tokens         (token_sha256, session_id, role, max_uses, used_count, expires_at)
audit_events          (id, ts, actor_id, actor_name, event_type, session_id, detail)
pending_registrations (email, display_name, password_hash, otp_code,
                       attempts_remaining, expires_at, created_at)
user_targets          (id, user_id, name, display, command, args, env, tags,
                       created_at, updated_at)
```

All IDs are UUIDs stored as TEXT. Timestamps are ISO 8601 TEXT. The `Storage` trait is async and implementation-agnostic — SQLite is the v1 backend.

**Schema evolution (0.1.x).** Migration state is kept in a single `migrations/001_initial.sql` file that is loaded on every boot. The loader (`run_migrations()` in `telepair-core/src/storage/sqlite.rs`) applies the full file, then performs column-existence checks (`pragma_table_info`) to idempotently add new columns on upgraded databases — e.g. `sessions.closed_reason`, `sessions.user_target_id`, and the `users` columns for email auth (`email`, `password_hash`, `session_enabled`, `login_failed_count`, `login_locked_until`). New tables (`audit_events`, `pending_registrations`, `user_targets`) use `CREATE TABLE IF NOT EXISTS` for the same reason. This keeps in-place upgrades working within the 0.1.x line without introducing a formal migration framework; the pre-1.0 "delete the DB" fallback still applies on genuine schema conflicts. A proper migration framework is planned for a later minor bump.

### Audit events

The `audit_events` table is append-only. Every row is a single immutable record of a security-meaningful state transition — logins, password changes, session lifecycle, participant joins / leaves / role changes, invite mint / redeem / revoke, target access denials, and target hot-reloads. High-frequency events (chat messages, cursor updates, PTY bytes) are **not** audited: the table would explode and none of them carry security-meaningful state that isn't already covered by the coarser events.

| Column | Purpose |
|--------|---------|
| `id` | Autoincrement i64 primary key — stable insertion order for paginated reads |
| `ts` | ISO 8601 UTC string, indexed for time-range queries |
| `actor_id` | User id of the initiator (nullable for system events and failed logins) |
| `actor_name` | Denormalized snapshot of the user name at emit time — a later user rename must not rewrite history |
| `event_type` | Tagged string like `session.created` or `invite.revoked` — serialized via `AuditEventType`'s `#[serde(rename = "...")]` |
| `session_id` | Indexed — supports the per-session timeline view and the `telepair admin audit --session <id>` filter |
| `detail` | JSON blob with event-specific fields: `reason`, `duration_s`, `role`, `max_uses`, `expires_at`, etc. |

Four indexes cover the four query shapes: time-range (`idx_audit_ts`), per-session timeline (`idx_audit_session`), per-actor history (`idx_audit_actor`), and type-filtered scans (`idx_audit_type`). The table is written to from `SessionService`, `InviteService`, the login path in `AuthService`, and the admin targets reload handler; reads happen from `GET /api/sessions/{id}/audit` and the `telepair admin audit` CLI.

## Security Model

- **Authentication**: Bearer token in `Authorization` header. Tokens are stored as their SHA-256 hex digest only — the raw value is returned to the caller exactly once at creation and never persisted. Email-based registration adds a second path: users register with email + password, verify via a 6-digit OTP sent over SMTP, and receive a bearer token on success. Passwords are hashed with Argon2 (salt per row); the OTP has a 15-minute TTL and a per-email 60-second rate limit.
- **Login throttling**: Password login enforces a 5-strike lockout — after 5 consecutive bad-password attempts the account is locked for 15 minutes. A single successful login clears the counter. All failure modes (unknown email, bad password, locked) return an identical error shape to prevent enumeration.
- **Pending registration**: The `pending_registrations` table is a staging area with no authority — it does not create a `users` row until the OTP is verified. Re-registration against an already-verified email silently succeeds (no information leak) and writes an audit row.
- **Admin approval gate**: New email-registered users start with `session_enabled = FALSE`. An admin must flip it to `TRUE` via `PUT /api/admin/users/{id}/session-access` before the user can create sessions or attach to WebSocket. Login itself remains permitted so the user can view history or change their password while waiting for approval.
- **Authorization**: Role-based per session. WS handler checks role on every input/resize action.
- **Invite tokens**: Single-use by default. Stored as SHA-256 digests; atomic `used_count < max_uses` increment prevents concurrent redemption race conditions.
- **CORS**: Configurable via `--allowed-origins` (comma-separated absolute URLs). When unset, the server defaults to **loopback dev origins only** (`http://localhost:5173`, `http://127.0.0.1:5173`) to match the Vite dev server. Malformed origins are fatal at startup so a typo can never silently widen the allowlist. `--allow-any-origin` opts into `Access-Control-Allow-Origin: *` and is only safe in dev or behind a reverse proxy that enforces CORS. For production direct-exposure (no reverse proxy), set `--allowed-origins` to the trusted frontend domain.
