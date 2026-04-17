English | [简体中文](protocol.zh-CN.md)

# WebSocket Protocol

Endpoint: `ws://localhost:7700/ws/session/{session_id}`

telepair uses a hybrid protocol over a single WebSocket connection:
- **Text frames**: JSON messages for control and collaboration
- **Binary frames**: Raw PTY bytes — server → client (PTY output) and
  client → server (keystrokes/paste)

## Connection Flow

```
Client                              Server
  │                                    │
  │──── WS connect ───────────────────▶│
  │                                    │
  │──── SessionJoin (JSON) ───────────▶│  auth + role lookup
  │                                    │
  │◀─── SessionState (JSON) ──────────│  session info + participants
  │                                    │
  │◀─── PTY output (binary) ──────────│  PTY output stream
  │──── keystrokes (binary) ──────────▶│  raw bytes, no framing
  │                                    │
  │◀─── PeerJoined ──────────────────│  collaboration events
  │◀─── PeerChat ────────────────────│
  │──── ChatMessage ─────────────────▶│
  │                                    │
```

## JSON Messages (Text Frames)

All JSON messages use `type` field discrimination (`#[serde(tag = "type")]` in Rust).

### Client to Server

#### SessionJoin

Must be the first message after connection. Authenticates the client.

```json
{
  "type": "SessionJoin",
  "session_id": "550e8400-...",
  "token": "bearer-token-here"
}
```

If authentication fails or the user is not a participant, the connection is closed.

#### Terminal input (binary frame, not JSON)

Keystrokes are sent as **raw binary WebSocket frames** — the client writes
UTF-8 bytes straight into the socket (no JSON wrapper, no framing header).
Requires `operator` or `owner` role.

For example, `ls\n` is sent as a 3-byte binary frame containing `[0x6c, 0x73, 0x0a]`.

#### TermResize

Resize the PTY. Requires `operator` or `owner` role.

```json
{
  "type": "TermResize",
  "cols": 120,
  "rows": 40
}
```

#### ChatMessage

Send a chat message. All roles can chat.

```json
{
  "type": "ChatMessage",
  "text": "Hello everyone!"
}
```

#### CursorMove

Report cursor position (for collaborative cursors).

```json
{
  "type": "CursorMove",
  "x": 42,
  "y": 10
}
```

### Server to Client

#### SessionState

Sent immediately after successful `SessionJoin`.

```json
{
  "type": "SessionState",
  "session": {
    "id": "550e8400-...",
    "owner_id": "...",
    "target_name": "local-shell",
    "input_mode": "serialized",
    "status": "active",
    "created_at": "2026-04-04T12:00:00Z",
    "closed_at": null
  },
  "participants": [
    { "user_id": "...", "name": "alice", "role": "owner", "color": "#58a6ff" }
  ],
  "your_role": "owner",
  "your_user_id": "...",
  "chat_history": [
    {
      "user_id": "...",
      "name": "alice",
      "text": "hi team",
      "ts": "2026-04-04T12:04:12.001Z"
    }
  ],
  "recording": {
    "recording_id": "e4a5b2c1-...",
    "started_at": "2026-04-04T12:02:00Z"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `chat_history` | array | Bounded replay of the in-session chat backlog (oldest-first, ephemeral — dies with the session). Each entry mirrors `PeerChat` 1:1 so the same renderer handles replay and live. |
| `recording` | object \| null | `{ recording_id, started_at }` when a recording is active at join time; absent (or `null`) otherwise, so late joiners see the indicator without extra REST round-trips. |

#### PeerJoined

Broadcast when a new participant connects.

```json
{
  "type": "PeerJoined",
  "user_id": "...",
  "name": "bob",
  "role": "operator",
  "color": "#d2a8ff"
}
```

#### PeerLeft

Broadcast when a participant disconnects.

```json
{
  "type": "PeerLeft",
  "user_id": "..."
}
```

#### PeerChat

Broadcast chat message with server-assigned timestamp.

```json
{
  "type": "PeerChat",
  "user_id": "...",
  "name": "alice",
  "text": "Hello everyone!",
  "ts": "2026-04-04T12:05:30.123Z"
}
```

#### PeerCursor

Forwarded cursor position from another participant.

```json
{
  "type": "PeerCursor",
  "user_id": "...",
  "x": 42,
  "y": 10
}
```

#### PeerRoleChanged

Broadcast when the session owner changes a participant's role via
`PUT /api/sessions/:id/participants/:user_id/role`. Every connected
client receives this so participant lists update in lockstep. The WS
handler also intercepts messages targeting the current connection to
re-evaluate input permissions without a reconnect.

```json
{
  "type": "PeerRoleChanged",
  "user_id": "...",
  "new_role": "viewer"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `user_id` | string | UUID of the participant whose role changed |
| `new_role` | string | New role: `"operator"` or `"viewer"` |

#### RecordingStarted

Broadcast to every participant when a recording starts on this session. Clients use this to flip the in-session "● REC" indicator without polling the REST endpoint. Requires the server to be started with `--recording-enabled`.

```json
{
  "type": "RecordingStarted",
  "recording_id": "e4a5b2c1-..."
}
```

#### RecordingStopped

Broadcast to every participant when the active recording stops (either explicitly via `POST /api/sessions/{id}/recording/stop` or implicitly when the session closes). Clients hide the recording indicator and invalidate any cached active-recording state.

```json
{
  "type": "RecordingStopped",
  "recording_id": "e4a5b2c1-..."
}
```

#### Error

Server-side error notification.

```json
{
  "type": "Error",
  "code": "PERMISSION_DENIED",
  "message": "viewers cannot send input"
}
```

## Binary Frames

PTY I/O uses raw binary WebSocket frames in both directions — no type byte,
no length prefix, just opaque payloads. The server forwards client frames
straight into the PTY writer and streams PTY output straight back. Terminal
resize is still handled via the JSON `TermResize` message.

Example PTY output `$ ` (2 bytes):
```
24 20
└── raw PTY bytes: "$ "
```

## Permission Enforcement

The server enforces permissions on every action:

| Action | Owner | Operator | Viewer |
|--------|-------|----------|--------|
| Terminal input (binary) | Yes | Yes | Rejected |
| TermResize | Yes | Yes | Rejected |
| ChatMessage | Yes | Yes | Yes |
| CursorMove | Yes | Yes | Yes |

Rejected actions are **silently dropped** — the server does not send an `Error` message or close the connection. The client receives no feedback when a permission check fails. In serialized input mode, only the session owner can send terminal input; other operators' input is also silently dropped.
