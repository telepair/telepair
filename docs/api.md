# REST API Reference

Base URL: `http://localhost:7700/api`

All endpoints except `/api/health` require authentication via Bearer token:

```
Authorization: Bearer <token>
```

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
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — target requires a role the user does not have (see `required_role` in target config)
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
- `401 Unauthorized` — missing or invalid token
- `403 Forbidden` — not the session owner
- `404 Not Found` — session does not exist

### POST /api/invite/redeem

Redeem an invite token to join a session.

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
  "role": "operator"
}
```

**Errors**
- `400 Bad Request` — invalid or exhausted invite token
- `401 Unauthorized` — missing or invalid auth token
