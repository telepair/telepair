English | [简体中文](api.zh-CN.md)

# REST API Reference

Base URL: `http://localhost:7700/api`

All endpoints except `/api/health` and `POST /api/invite/redeem` require authentication via Bearer token:

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

List active sessions.

**Response** `200 OK`
```json
[
  {
    "id": "550e8400-...",
    "owner_id": "...",
    "target_name": "local-shell",
    "input_mode": "serialized",
    "status": "active",
    "created_at": "2026-04-04T12:00:00Z",
    "closed_at": null
  }
]
```

### DELETE /api/sessions/{session_id}

Close a session. Only the session owner can close it. Stops the PTY process and marks the session as closed.

**Response** `204 No Content`

No response body.

**Errors**
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — not the session owner
- `404 Not Found` — session does not exist

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

**Response** `201 Created`
```json
{
  "token": "abc123...",
  "role": "operator",
  "max_uses": 1,
  "session_id": "550e8400-..."
}
```

**Errors**
- `400 Bad Request` — `role` is `owner` (only `operator` / `viewer` can be invited), `max_uses` is zero or negative, or the body is otherwise malformed
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
