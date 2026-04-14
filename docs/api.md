English | [简体中文](api.zh-CN.md)

# REST API Reference

Base URL: `http://localhost:7700/api`

All endpoints except `/api/health`, the auth endpoints (`POST /api/auth/register`, `POST /api/auth/verify`, `POST /api/auth/login`), and `POST /api/invite/redeem` require authentication via Bearer token:

```
Authorization: Bearer <token>
```

`POST /api/invite/redeem` accepts both authenticated and anonymous callers — anonymous redemptions mint a fresh guest user and return a new token in the response (see below).

## Health

### GET /api/health

Check server status. No authentication required.

**Response** `200 OK`
```json
{ "status": "ok" }
```

## Auth

### GET /api/auth/whoami

Return the authenticated caller's identity. Used by the frontend auth store to
cache `currentUserId` and `is_admin` on boot so the dashboard can gate owner-only
affordances (audit dialog, close button) without an extra round trip per row.

**Response** `200 OK`
```json
{
  "user_id": "...",
  "name": "admin",
  "is_admin": true,
  "is_guest": false,
  "session_enabled": true
}
```

| Field | Type | Description |
|-------|------|-------------|
| `user_id` | string | UUID of the caller |
| `name` | string | Current display name |
| `is_admin` | boolean | `true` for admin accounts |
| `is_guest` | boolean | `true` for invite-minted scoped guests |
| `session_enabled` | boolean | `true` when the user is allowed to create / join sessions. The Dashboard renders a "pending admin approval" banner and hides the session-create form when this is `false`. |

**Errors**
- `401 Unauthorized` — missing or invalid token. Never returns 403: "I am a guest" is still a valid identity worth surfacing.

### POST /api/auth/register

Start email registration. Creates an unverified pending account and sends a
one-time verification code to the provided address. **No authentication required.**

The endpoint always returns `201` on valid input, regardless of whether the
email is already registered or a pending registration was recently created.
This is intentional enumeration safety — callers cannot distinguish "code sent"
from "address already in use." The detailed reason (already registered, rate
limited, etc.) is captured in the audit log.

**Request Body**
```json
{
  "email": "alice@example.com",
  "password": "s3cret!",
  "display_name": "Alice"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `email` | string | yes | Email address (case-insensitive) |
| `password` | string | yes | Plaintext password; hashed with Argon2 before storage |
| `display_name` | string | yes | Display name for the new account |

**Response** `201 Created`
```json
{
  "message": "Verification code sent to your email."
}
```

**Errors**
- `400 Bad Request` — malformed request body
- `503 Service Unavailable` — SMTP is not configured on this instance; contact the administrator

### POST /api/auth/verify

Submit the OTP code received via email to complete registration. Returns a
bearer token on success. **No authentication required.**

Every failure mode (bad code, expired, locked after too many attempts) is
collapsed into the same `401` shape so the API cannot be used to enumerate
which addresses have pending registrations. The detailed reason is still
captured in the audit log.

**Request Body**
```json
{
  "email": "alice@example.com",
  "code": "839204"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `email` | string | yes | Email address used during registration |
| `code` | string | yes | 6-digit OTP code from the verification email |

**Response** `200 OK`
```json
{
  "token": "newly-minted-bearer-token"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `token` | string | Bearer token for the newly verified account. The account starts with `session_enabled = false` — an admin must enable it before the user can create or join sessions. |

**Errors**
- `400 Bad Request` — malformed request body
- `401 Unauthorized` — OTP code is wrong, expired, or the pending row is locked after too many failed attempts

### POST /api/auth/login

Unified login endpoint. Accepts either a raw bearer token (the existing
admin/guest path) or email + password credentials (email-registered users).
**No authentication required.**

**Request Body — token login**
```json
{
  "token": "existing-bearer-token"
}
```

**Request Body — email + password login**
```json
{
  "email": "alice@example.com",
  "password": "s3cret!"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `token` | string | no* | Existing bearer token to validate. Mutually exclusive with `email`/`password`. |
| `email` | string | no* | Email address for password login |
| `password` | string | no* | Password for email login |

\* Exactly one of `{token}` or `{email, password}` must be provided.

**Response** `200 OK`
```json
{
  "token": "valid-bearer-token"
}
```

For token login, the same token is echoed back after validation. For email +
password login, a fresh bearer token is minted and returned.

**Errors**
- `400 Bad Request` — neither `token` nor `email`+`password` provided, or body is malformed
- `401 Unauthorized` — token is invalid, email is unknown, password is wrong, or account is locked after too many failed attempts. All cases return the same generic error (enumeration safety). Login failures are throttled: after 5 consecutive bad passwords the account is locked for a cooldown window. The lockout is only visible in the audit trail.

**Note:** the `session_enabled` check does **not** happen at login. A disabled account can still log in (to read history, change password, etc.). Every session-mutating surface enforces the bit on its own: `POST /api/sessions`, invite mint/revoke/redeem (`POST|DELETE /api/sessions/{id}/invites[/{token}]`, `POST /api/invite/redeem` for authenticated callers), participant-role updates (`PUT /api/sessions/{id}/participants/{user_id}/role`), and WebSocket attach (`GET /ws/session/{id}`).

### POST /api/auth/change-password

Change the authenticated user's password. Requires the current password for
verification (defence in depth against session theft) even though the caller
already holds a valid bearer token. Rejects users who do not have a password
hash (admin/CLI accounts created via token, not email registration).

On success the old bearer token is invalidated and a new one is returned. The
password hash update and token rotation happen in a single SQLite transaction
so a crash between the two writes can never leave the old token valid after a
password change.

**Request Body**
```json
{
  "current_password": "old-pass",
  "new_password": "new-pass"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `current_password` | string | yes | The user's current password |
| `new_password` | string | yes | New password; must be at least 8 characters |

**Response** `200 OK`
```json
{
  "token": "new-bearer-token"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `token` | string | Fresh bearer token. The previous token is invalidated. |

**Errors**
- `400 Bad Request` — request body is malformed, new password is shorter than 8 characters, or the account does not use password authentication (admin/CLI accounts)
- `401 Unauthorized` — missing or invalid bearer token, or current password is incorrect

## Targets

### GET /api/targets

List available targets.

**Response** `200 OK`
```json
[
  {
    "name": "local-shell",
    "display": "Local Shell",
    "tags": []
  },
  {
    "name": "production-db",
    "display": "Production DB",
    "tags": ["database", "production"]
  }
]
```

## Sessions

### POST /api/sessions

Create a new session. The authenticated user becomes the session owner.

**Request Body**
```json
{
  "target_name": "local-shell",
  "input_mode": "serialized"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `target_name` | string | yes | Target to launch |
| `input_mode` | string | no | `"serialized"` (default) or `"multiplexed"` |

**Response** `201 Created`
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "owner_id": "...",
  "target_name": "local-shell",
  "input_mode": "serialized",
  "status": "active",
  "created_at": "2026-04-04T12:00:00Z",
  "closed_at": null
}
```

**Errors**
- `400 Bad Request` — `input_mode` is present but not one of `serialized` / `multiplexed` (unknown values used to silently collapse to `serialized`; that was masking client bugs)
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — target is marked `admin_only: true` in target config and the caller is not an admin
- `404 Not Found` — target does not exist

### GET /api/sessions

List sessions visible to the caller. Regular users see sessions they own plus any they joined as a participant; **admin callers see every session in the workspace** (so the admin targets page's `N active sessions` deep link resolves to a non-empty page). Supports filtering by status and target, plus `limit`/`offset` for pagination.

**Query Parameters**

| Param | Type | Description |
|-------|------|-------------|
| `status` | string | `active`, `closed`, or `all` (default: `all`). Unknown values fall back to `all` so a typo in the UI's querystring does not blow up the page. |
| `target_name` | string | Only return sessions launched from this target. Used by the admin targets page's "N active sessions" deep link. |
| `limit` | integer | Upper bound on rows returned. Missing or non-positive = unlimited. |
| `offset` | integer | Pagination offset; non-positive values collapse to `0`. |

**Response** `200 OK`
```json
[
  {
    "id": "550e8400-...",
    "owner_id": "...",
    "target_name": "local-shell",
    "input_mode": "multiplexed",
    "status": "closed",
    "created_at": "2026-04-04T12:00:00Z",
    "closed_at": "2026-04-04T12:42:00Z",
    "closed_reason": "owner"
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `closed_reason` | string \| absent | One of `owner`, `reaper`, `startup`, `error`. Omitted for active rows and for legacy v0.1.0 closed rows created before the column existed. |

### DELETE /api/sessions/{session_id}

Close a session. Only the session owner can close it. Stops the PTY process and marks the session as closed.

**Response** `204 No Content`

No response body.

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — not the session owner
- `404 Not Found` — session does not exist

### PUT /api/sessions/{session_id}/participants/{user_id}/role

Change a participant's role in a live session. Owner-only. The owner cannot
change their own role or promote anyone to `owner`.

Persists the change to the database, updates the hub's in-memory participant
map, and broadcasts a `PeerRoleChanged` WebSocket message to all connected
clients so participant lists update in lockstep and the WS handler
re-evaluates input permissions for the affected connection without a
reconnect.

**Request Body**
```json
{
  "role": "viewer"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `role` | string | yes | `"operator"` or `"viewer"` |

**Response** `204 No Content`

No response body. If the participant already has the requested role, the
endpoint is a no-op and still returns `204`.

**Errors**
- `400 Bad Request` — `role` is `owner`, target user is the owner themselves, or malformed UUID
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — not the session owner
- `404 Not Found` — session does not exist, session is not active, or target user is not an active participant

## Invites

### POST /api/sessions/{session_id}/invite

Create an invite link for a session. Only the session owner can create invites.

**Request Body**
```json
{
  "role": "operator",
  "max_uses": 1
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `role` | string | yes | `"operator"` or `"viewer"` |
| `max_uses` | integer | no | Maximum redemptions (default: 1) |

| `expires_in_minutes` | integer | no | TTL in minutes. Mutually exclusive with `expires_at`; the server resolves it to an absolute UTC timestamp before persisting. **Clamped** to `MAX_INVITE_TTL_MINUTES` (a slider overshoot is treated as a benign UX mistake). |
| `expires_in_secs` | integer | no | TTL in seconds. Takes precedence over `expires_in_minutes` when both are supplied. Useful for CLI / automated callers that need sub-minute precision; the server resolves it to an absolute `expires_at` before validation, so past / out-of-range values still return `400 invalid_input`. |
| `expires_at` | string (ISO 8601) | no | Absolute expiry. Wins if both `expires_in_minutes` (or `expires_in_secs`) and `expires_at` are set. **Rejected** with `400 invalid_input` if it exceeds `MAX_INVITE_TTL_MINUTES` — the server never silently rewrites an explicit wall-clock pick. |

**Response** `201 Created`
```json
{
  "token": "abc123...",
  "role": "operator",
  "max_uses": 1,
  "expires_at": "2026-04-04T13:00:00Z",
  "session_id": "550e8400-..."
}
```

The raw `token` is returned **once** — the DB only stores its SHA-256 digest. Capture it now; there is no endpoint to recover it later.

**Errors**
- `400 Bad Request` — `role` is `owner` (only `operator` / `viewer` can be invited), `max_uses` is zero / negative / over the `MAX_INVITE_USES` cap, TTL is non-positive, both `expires_in_minutes` and `expires_at` are in the past, or the body is otherwise malformed
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — not the session owner
- `404 Not Found` — session does not exist
- `410 Gone` — session exists but is already closed

### GET /api/sessions/{session_id}/invites

List every invite ever minted for this session, newest first. Owner-only. Deliberately
includes expired and exhausted rows so the management dialog can show post-mortem
state ("these were the invites in flight when the session closed") without a second
endpoint.

The raw token is **not** returned — only the sha256 digest (used as the revoke path
parameter) and an 8-char prefix label. There is no way to recover a forgotten link;
mint a new invite instead.

**Response** `200 OK`
```json
[
  {
    "token_sha256": "7d2b...a1",
    "token_prefix": "7d2ba1f4",
    "session_id": "550e8400-...",
    "role": "operator",
    "max_uses": 3,
    "used_count": 1,
    "remaining_uses": 2,
    "expires_at": "2026-04-04T13:00:00Z",
    "created_at": "2026-04-04T12:00:00Z"
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `token_sha256` | string | Full SHA-256 digest of the invite token. Opaque label; used as the path parameter on the revoke endpoint. |
| `token_prefix` | string | First 8 chars of `token_sha256` — short stable identifier for the UI. |
| `remaining_uses` | integer | Precomputed `max(0, max_uses - used_count)` so every client renders the same number. |

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — not the session owner
- `404 Not Found` — session does not exist

### DELETE /api/sessions/{session_id}/invites/{token_sha256}

Hard-delete an invite row. Owner-only. **Idempotent**: a double-revoke, an
unknown sha, and a cross-session probe (a sha that exists but belongs to a
different session) all return `204 No Content`. This prevents the endpoint from
leaking cross-session invite existence and lets the UI drop a "revoke" click
without a special error toast when two admins race the operation. Side effects
remain session-scoped — the underlying invite is only deleted when it actually
belongs to the path-parameter session.

Revoked invites cannot be redeemed (the row is gone, so `POST /api/invite/redeem`
returns `400` on the raw token).

**Response** `204 No Content`

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — not the session owner
- `404 Not Found` — session does not exist

### POST /api/invite/redeem

Redeem an invite token to join a session.

**Authentication is optional.** The endpoint accepts three shapes of caller:

1. **Anonymous visitor (the common case).** Drop the `Authorization` header entirely. The server consumes one `max_uses` slot, mints a new guest user named `guest-<nanoid>`, and returns a freshly issued token in the `token` field. The client is expected to persist that token and use it for all subsequent API / WebSocket calls in that tab.
2. **Authenticated caller.** Send a valid bearer token. The server reuses the caller's existing identity (useful for an admin previewing their own invite link) and returns `"token": null` — you already have a token.
3. **Stale bearer token.** An invalid `Authorization` header is treated the same as an anonymous visitor: the server silently drops it and mints a guest. This is intentional so a browser with an expired admin token can still follow an invite link.

**Request Body**
```json
{
  "token": "abc123..."
}
```

**Response** `200 OK`
```json
{
  "session_id": "550e8400-...",
  "role": "operator",
  "token": "newly-minted-guest-token-or-null"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | string | Session to join |
| `role` | string | Role granted by the invite (`operator` or `viewer`) |
| `token` | string \| null | Bearer token for the newly minted guest, or `null` when an authenticated caller reused their existing identity |

**Errors**
- `400 Bad Request` — the invite token is unknown, expired, or has hit `max_uses` (guest accounts are **not** created on rejected invites, so a bad link never leaves orphan users behind)
- `404 Not Found` — the invite points at a session that never existed
- `410 Gone` — the target session has been closed; the invite is not consumed, so the operator can still revoke it or reassign uses to a new session

## Session Audit

### GET /api/sessions/{session_id}/audit

Return the audit events that touched this session, newest first. Owner-only —
uses the same 403/404 gate as `GET /api/sessions/{id}/invites`. Closed sessions
are still readable (the whole point of a history view), so the ownership check
does not require an active session.

Capped at 500 rows (no pagination surface yet — real session footprints are
orders of magnitude below this). See [Architecture — Audit events](architecture.md#audit-events)
for the event taxonomy and write paths.

**Response** `200 OK`
```json
[
  {
    "id": 12,
    "ts": "2026-04-04T12:10:05Z",
    "actor_id": "...",
    "actor_name": "admin",
    "event_type": "session.closed",
    "session_id": "550e8400-...",
    "detail": {
      "reason": "owner",
      "duration_s": 600
    }
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | integer | Monotonic insertion order — stable pagination cursor when we eventually need it. |
| `ts` | string (ISO 8601) | Emit time, UTC. |
| `actor_id` | string \| null | UUID of the initiator. `null` for system events and failed logins. |
| `actor_name` | string \| null | Denormalized display name snapshot — a later rename does not rewrite history. |
| `event_type` | string | Tagged string: `session.created`, `session.closed`, `participant.joined`, `participant.left`, `participant.role_changed`, `invite.minted`, `invite.redeemed`, `invite.revoked`, `auth.login_success`, `auth.login_failed`, `auth.register_rejected`, `auth.register_completed`, `auth.verify_failed`, `auth.user_enabled`, `auth.user_disabled`, `auth.session_access_denied`, `auth.password_changed`, `target.access_denied`, `target.reloaded`. |
| `session_id` | string \| null | `null` for events without a session (logins, target reload). |
| `detail` | object | Event-specific JSON blob. For example, `session.closed` carries `{reason, duration_s}`; `invite.minted` carries `{role, max_uses, expires_at}`. |

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — not the session owner
- `404 Not Found` — session does not exist

## User Targets

User-owned virtual targets. Each user can create, read, update, and delete their
own targets. These targets appear alongside global targets from `targets.yaml` in
the target list (with `"source": "user"`). Scoped guests get `403` on all user
target routes — they are invite-minted and session-local.

### POST /api/user-targets

Create a user-owned virtual target. The authenticated user becomes the owner.

**Request Body**
```json
{
  "name": "my-dev-db",
  "display": "My Dev Database",
  "command": "psql",
  "args": ["-h", "localhost", "-U", "dev", "mydb"],
  "env": { "PGPASSWORD": "devpass" },
  "tags": ["database", "dev"]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique target name (must be non-blank) |
| `display` | string | yes | Human-readable display name (must be non-blank) |
| `command` | string | yes | Command to execute (must be non-blank) |
| `args` | string[] | no | Command arguments (default: `[]`) |
| `env` | object | no | Environment variables for the target process (default: `{}`) |
| `tags` | string[] | no | Descriptive tags (default: `[]`) |

**Response** `201 Created`
```json
{
  "id": "a1b2c3d4e5",
  "user_id": "550e8400-e29b-41d4-a716-446655440000",
  "name": "my-dev-db",
  "display": "My Dev Database",
  "command": "psql",
  "args": ["-h", "localhost", "-U", "dev", "mydb"],
  "env": { "PGPASSWORD": "devpass" },
  "tags": ["database", "dev"],
  "created_at": "2026-04-13T10:00:00Z",
  "updated_at": "2026-04-13T10:00:00Z"
}
```

**Errors**
- `400 Bad Request` — `name`, `display`, or `command` is blank, or the body is malformed
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is a scoped guest

### GET /api/user-targets/{id}

Fetch a single user-owned target. Only the target owner can read it.

**Response** `200 OK`
```json
{
  "id": "a1b2c3d4e5",
  "user_id": "550e8400-...",
  "name": "my-dev-db",
  "display": "My Dev Database",
  "command": "psql",
  "args": ["-h", "localhost", "-U", "dev", "mydb"],
  "env": { "PGPASSWORD": "devpass" },
  "tags": ["database", "dev"],
  "created_at": "2026-04-13T10:00:00Z",
  "updated_at": "2026-04-13T10:00:00Z"
}
```

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is a scoped guest
- `404 Not Found` — target does not exist or is not owned by the caller

### PUT /api/user-targets/{id}

Update a user-owned target. Only the target owner can update it. The `name`
field is ignored on update — only `display`, `command`, `args`, `env`, and
`tags` are mutable.

**Request Body**
```json
{
  "display": "My Dev Database (updated)",
  "command": "psql",
  "args": ["-h", "localhost", "-U", "dev", "mydb_v2"],
  "env": { "PGPASSWORD": "newpass" },
  "tags": ["database", "dev", "v2"]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `display` | string | yes | Human-readable display name (must be non-blank) |
| `command` | string | yes | Command to execute (must be non-blank) |
| `args` | string[] | no | Command arguments (default: `[]`) |
| `env` | object | no | Environment variables (default: `{}`) |
| `tags` | string[] | no | Descriptive tags (default: `[]`) |

**Response** `200 OK`

Returns the updated `UserTarget` object (same shape as the create response).

**Errors**
- `400 Bad Request` — `display` or `command` is blank, or the body is malformed
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is a scoped guest
- `404 Not Found` — target does not exist or is not owned by the caller
- `409 Conflict` — an active session still references this target; close the session first, then retry

### DELETE /api/user-targets/{id}

Delete a user-owned target. Only the target owner can delete it.

**Response** `204 No Content`

No response body.

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is a scoped guest
- `404 Not Found` — target does not exist or is not owned by the caller
- `409 Conflict` — an active session still references this target; close the session first, then retry

## Admin

The `/api/admin/*` routes require an admin bearer token. Non-admins get `403`.
Guest tokens cannot reach these routes — the caller is authenticated but out of
scope.

### GET /api/admin/targets

Return every target currently loaded by the in-memory `TargetEngine`, including
the raw command / args / shell strings, env key presence, and the per-target
active session count.

**Security note:** env values are **never** serialized. Each key is returned as
`{"key": "PGPASSWORD", "set": true|false}`, where `set` reflects whether
`std::env::var(key)` would succeed right now on the server process. Telepair
already trusts whoever can write `targets.yaml`, but exposing resolved secrets
through an HTTP API would widen the blast radius beyond that implicit trust.

**Response** `200 OK`
```json
[
  {
    "name": "production-db",
    "display": "Production DB",
    "type": "virtual",
    "command": "psql",
    "args": ["-h", "db.internal", "-U", "readonly", "production"],
    "shell": null,
    "tags": ["database", "production"],
    "admin_only": true,
    "env": [
      { "key": "PGPASSWORD", "set": true }
    ],
    "active_sessions": 2
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | `virtual` for yaml-defined targets, `local` for the built-in default shell. |
| `command` | string \| null | Literal command string from `targets.yaml`. `${VAR}` placeholders are preserved verbatim — interpolation happens at spawn time. |
| `args` | string[] | Literal argv tail with `${VAR}` placeholders preserved. |
| `shell` | string \| null | Override shell for `local`-kind targets. |
| `admin_only` | boolean | Mirrors the `admin_only: true` flag in the yaml config. |
| `env` | array | Sorted-by-key list of `{key, set}` pairs. Values are never included. |
| `active_sessions` | integer | Live count from one grouped `SELECT` on `sessions` — matches the chip on the admin UI. |

Results are sorted by `name` so the admin UI does not shuffle between polls.

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is authenticated but not an admin

### POST /api/admin/targets/validate

Parse `targets.yaml` from disk and diff it against the in-memory engine
without applying any changes. Admin-only, read-only, safe to call at any
time. The admin UI runs this before every reload so the operator sees
exactly what will change, and the returned sha-256 can be echoed back
through `/api/admin/targets/reload` to close the validate → confirm
TOCTOU window.

The request takes no body.

**Response** `200 OK` — parse succeeded

```json
{
  "valid": true,
  "path": "/home/admin/.telepair/targets.yaml",
  "total": 4,
  "diff": {
    "added":     ["ops-shell"],
    "removed":   ["legacy-db"],
    "changed":   ["prod-db"],
    "unchanged": ["redis", "minio"]
  },
  "blocked": [
    { "target": "legacy-db", "active_sessions": 2 }
  ],
  "expected_sha256": "9a8b7c...e1f2"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `valid` | boolean | `true` when the file parsed and the diff was computed. |
| `path` | string | Absolute path that was read. |
| `total` | integer | Target count in the proposed engine. |
| `diff.added` / `removed` / `changed` / `unchanged` | string[] | Target names classified against the currently loaded engine. `changed` means the name exists on both sides but the config differs. |
| `blocked` | array | Subset of `removed` that still has live sessions referencing them. A subsequent reload would fail with `still_referenced` until those sessions are closed. Validate itself does NOT block on this. |
| `expected_sha256` | string | Hex SHA-256 of the raw `targets.yaml` bytes. Echo this into the reload body to reject any concurrent writer. |

**Response** `200 OK` — parse failed

Parse failure is an expected outcome (admin pasted broken yaml), not an
error. The endpoint still returns `200` with `valid: false` so the UI can
render the message inline without branching on network-level errors.

```json
{
  "valid": false,
  "errors": ["invalid yaml at line 12: mapping values are not allowed here"]
}
```

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is authenticated but not an admin
- `500 Internal Server Error` — tokio blocking-task join failure (not expected under normal operation)

### POST /api/admin/targets/reload

Re-read `targets.yaml` from disk and atomically install the resulting
`TargetEngine` into application state. The swap uses `arc_swap` so in-flight
requests complete against the old engine and subsequent requests see the new
one — there is no lock window.

Emits a `target.reloaded` audit event on success with `{path, targets}` in the
detail blob.

**Request body** (optional)
```json
{ "expected_sha256": "<hex sha-256 from the preceding validate response>" }
```

When present, the server re-hashes the on-disk bytes and refuses the reload
with `409` if the hash has drifted. This closes the validate → confirm TOCTOU:
an admin who previewed version A never applies version B, even if a second
writer overwrote the file mid-flow. Omit the body to opt out (CLI / no-preview
callers).

**Response** `200 OK`
```json
{
  "path": "/home/admin/.telepair/targets.yaml",
  "targets": 4
}
```

| Field | Type | Description |
|-------|------|-------------|
| `path` | string | Absolute path that was re-read. |
| `targets` | integer | New target count after the swap. |

**Errors**
- `400 Bad Request` with body `{ "reason": "no_targets_path", "message": "..." }` — the server was started without a `targets.yaml` path, so there is nothing to reload. The old engine stays loaded.
- `400 Bad Request` with body `{ "reason": "parse_error", "message": "...", "path": "..." }` — the file on disk is now malformed. The old engine stays loaded and the `message` carries the parse error verbatim so the admin can fix the yaml.
- `400 Bad Request` with body `{ "reason": "still_referenced", "message": "...", "targets": [{ "target": "...", "active_sessions": N }, ...] }` — the new `targets.yaml` would drop one or more targets that still have live sessions in the hub. The old engine stays loaded and the `targets` array lists exactly which targets are blocking the reload with their live session counts, so the operator can close those sessions (or restore the target in yaml) and retry. Admin UI renders this as a persistent banner instead of a one-shot toast.
- `409 Conflict` with body `{ "reason": "file_changed", "message": "...", "expected_sha256": "...", "actual_sha256": "..." }` — the caller sent `expected_sha256` but the file's current bytes hash to a different value. The old engine stays loaded; the admin must re-run validate before retrying.
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is authenticated but not an admin

### GET /api/admin/users

List every non-guest user account, newest first. Admin-only. Scoped guests are
not included — they are invite-minted, session-local, and disappear on close.

This endpoint backs the admin Users page. In v0.1.4 it gained server-side
filtering, pagination, and a wrapped response shape. **Breaking change vs.
v0.1.3**: the response is no longer a bare array — callers must read
`users` and `total` out of the enclosing object.

**Query Parameters**

| Param | Type | Description |
|-------|------|-------------|
| `q` | string (optional) | Case-insensitive substring match on name or email. |
| `status` | `"enabled"` \| `"disabled"` \| `"pending"` (optional) | Filter by admin-approval bucket (see `approval_state` below). Unknown values are ignored. |
| `limit` | integer (optional, default `50`, capped at `500`) | Maximum rows returned. |
| `offset` | integer (optional, default `0`) | Rows to skip for pagination. |

**Response** `200 OK`
```json
{
  "users": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "name": "alice",
      "email": "alice@example.com",
      "is_admin": false,
      "session_enabled": false,
      "approval_state": "pending",
      "created_at": "2026-04-13T08:00:00Z",
      "updated_at": "2026-04-13T08:00:00Z"
    }
  ],
  "total": 1
}
```

| Field | Type | Description |
|-------|------|-------------|
| `users` | array | Matching rows for this page. |
| `total` | integer | Total matching rows across all pages (ignores `limit`/`offset`). |

Per-row fields:

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | UUID of the user |
| `name` | string | Display name |
| `email` | string \| null | Email address. `null` for admin/CLI accounts that never registered with email. Exposed here because the caller is already an admin with full target-reload and session-close rights. |
| `is_admin` | boolean | `true` for admin accounts |
| `session_enabled` | boolean | `true` when the user is allowed to create / join sessions. New email registrations start with `false`. |
| `approval_state` | `"pending"` \| `"approved"` | Admin-approval bucket. `"pending"` = completed OTP verification and waiting for an admin to enable the account. `"approved"` = has been enabled at some point; may still be currently disabled (`session_enabled: false`) if an admin flipped it off later. The `POST /enable` endpoint atomically sets this to `"approved"` and `session_enabled` to `true`; `POST /disable` leaves `approval_state` alone. |
| `created_at` | string (ISO 8601) | Account creation time, UTC |
| `updated_at` | string (ISO 8601) | Last modification time, UTC |

The three `status` filter values map to:

| Filter | SQL predicate |
|--------|---------------|
| `enabled` | `approval_state = 'approved' AND session_enabled = TRUE` |
| `disabled` | `approval_state = 'approved' AND session_enabled = FALSE` |
| `pending` | `approval_state = 'pending'` |

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is authenticated but not an admin

### POST /api/admin/users/{id}/enable

Enable session access for a user. Flips `session_enabled = true` on the target
row and emits an audit event. Admin-only.

**Response** `200 OK`

Returns the updated user object (same shape as rows in `GET /api/admin/users`).

```json
{
  "id": "550e8400-...",
  "name": "alice",
  "email": "alice@example.com",
  "is_admin": false,
  "session_enabled": true,
  "approval_state": "approved",
  "created_at": "2026-04-13T08:00:00Z",
  "updated_at": "2026-04-13T09:00:00Z"
}
```

**Errors**
- `400 Bad Request` — malformed UUID in path, or admin attempted to enable/disable themselves (self-mutation guard)
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is authenticated but not an admin
- `404 Not Found` — user does not exist

### POST /api/admin/users/{id}/disable

Disable session access for a user. Flips `session_enabled = false` on the target
row and emits an audit event. Admin-only.

The user keeps their bearer token — `whoami` and session history still work. The
next session create or WebSocket attach they attempt fails closed via the
`session_enabled` gate.

**Response** `200 OK`

Returns the updated user object (same shape as rows in `GET /api/admin/users`).

**Errors**
- `400 Bad Request` — malformed UUID in path, or admin attempted to enable/disable themselves (self-mutation guard)
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is authenticated but not an admin
- `404 Not Found` — user does not exist

### GET /api/admin/audit

Global audit log, admin-only. Returns events newest-first with optional
filtering on time range, actor, event type, and session. Default limit is
100 rows, capped at 500 to prevent accidental full-table dumps.

**Query Parameters**

| Param | Type | Description |
|-------|------|-------------|
| `limit` | integer | Upper bound on rows returned (default: 100, max: 500). |
| `offset` | integer | Pagination offset (default: 0). |
| `since` | string (ISO 8601) | Inclusive lower bound on `ts`. |
| `until` | string (ISO 8601) | Exclusive upper bound on `ts`. |
| `actor_id` | string | Filter by actor UUID. Invalid UUIDs are silently ignored. |
| `event_type` | string | Single dotted-lowercase type (e.g. `auth.login_failed`). Invalid values are silently ignored. |
| `session_id` | string | Filter to events touching a specific session. |

**Response** `200 OK`

Same row shape as `GET /api/sessions/{id}/audit` — see the session audit
section above for the field reference.

```json
[
  {
    "id": 42,
    "ts": "2026-04-14T08:00:00Z",
    "actor_id": "...",
    "actor_name": "alice",
    "event_type": "auth.password_changed",
    "session_id": null,
    "detail": { "email": "alice@example.com" }
  }
]
```

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is authenticated but not an admin

### GET /api/admin/audit/export

Export the audit log as a downloadable JSON or CSV attachment. Admin-only.
Accepts the same filter parameters as `GET /api/admin/audit` but does not
paginate — the endpoint returns the full matching set up to 10,000 rows
and rejects larger exports with `413` to prevent accidental full-table
dumps.

**Query Parameters**

| Param | Type | Description |
|-------|------|-------------|
| `format` | `"json"` \| `"csv"` | **Required.** Anything else returns `400`. |
| `since` | string (ISO 8601) | Inclusive lower bound on `ts`. |
| `until` | string (ISO 8601) | Exclusive upper bound on `ts`. |
| `actor_id` | string | Filter by actor UUID. Invalid UUIDs are silently ignored. |
| `event_type` | string | Single dotted-lowercase type (e.g. `session.closed`). Invalid values are silently ignored. |
| `session_id` | string | Filter to events touching a specific session. |

**Response** `200 OK`

- `format=json` — `Content-Type: application/json`. Body is the same array shape as `GET /api/admin/audit`.
- `format=csv` — `Content-Type: text/csv; charset=utf-8`. RFC 4180 quoting (commas, quotes, newlines handled). Columns: `id,timestamp,event_type,actor_id,actor_name,session_id,detail`. `detail` is a JSON-stringified object.

Both formats return `Content-Disposition: attachment; filename="telepair-audit-<UTC-timestamp>.<ext>"` so the browser offers a download dialog.

**Security: CSV formula injection.** Any string cell beginning with `=`,
`+`, `-`, `@`, TAB, or CR is prefixed with a single quote before quoting.
Excel / Numbers / Google Sheets would otherwise evaluate such cells as
formulas, which is an exfiltration vector when a user-controlled field
(display name, audit detail) lands in a spreadsheet. The single-quote
prefix is a well-known mitigation that stays invisible to most viewers
while disarming formula evaluation.

**Errors**
- `400 Bad Request` — `format` missing or not one of `json` / `csv`.
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is authenticated but not an admin
- `413 Payload Too Large` — filtered result would exceed the 10,000-row cap. Narrow the time range or filter and retry.

### GET /api/admin/system

Snapshot of server-level diagnostics for the admin dashboard: version,
filesystem paths, SMTP status, live session count, registered user count,
and uptime. Admin-only. Useful for a one-shot sanity check without SSH'ing
into the box.

**Response** `200 OK`

```json
{
  "version": "0.1.4",
  "data_dir": "/home/admin/.telepair",
  "db_path": "/home/admin/.telepair/telepair.db",
  "targets_path": "/home/admin/.telepair/targets.yaml",
  "smtp_configured": true,
  "live_sessions": 3,
  "registered_users": 42,
  "uptime_seconds": 86400
}
```

| Field | Type | Description |
|-------|------|-------------|
| `version` | string | `CARGO_PKG_VERSION` at build time. |
| `data_dir` | string | Absolute path of the telepair data directory. |
| `db_path` | string | Absolute path of the sqlite file inside `data_dir`. |
| `targets_path` | string \| null | Absolute path of the configured `targets.yaml`, or `null` if telepair was started without one. |
| `smtp_configured` | boolean | Whether a working SMTP transport is available for OTP email delivery. |
| `live_sessions` | integer | Count of sessions currently attached in `SessionHub`. |
| `registered_users` | integer | Count of non-guest user rows — uses `SELECT COUNT(*)` without materialising rows. |
| `uptime_seconds` | integer | Seconds since the gateway process started. |

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — caller is authenticated but not an admin
