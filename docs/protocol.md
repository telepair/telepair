# WebSocket Protocol

Endpoint: `ws://localhost:7700/ws/session/{session_id}`

telepair uses a hybrid protocol over a single WebSocket connection:
- **Text frames**: JSON messages for control and collaboration
- **Binary frames**: Compact encoding for terminal I/O

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
  │◀─── TermOutput (JSON/binary) ─────│  PTY output stream
  │──── TermInput (JSON/binary) ──────▶│  user keystrokes
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

#### TermInput

Send keystrokes to the PTY. Requires `operator` or `owner` role.

```json
{
  "type": "TermInput",
  "data": [108, 115, 10]
}
```

`data` is an array of bytes (UTF-8 encoded). For example, `ls\n` encodes as `[108, 115, 10]`.

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
  "your_role": "owner"
}
```

#### TermOutput

PTY output data, broadcast to all connected clients.

```json
{
  "type": "TermOutput",
  "data": [36, 32]
}
```

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

#### PermUpdate

Broadcast when a participant's role changes.

```json
{
  "type": "PermUpdate",
  "user_id": "...",
  "new_role": "operator"
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

## Binary Frame Protocol

For high-throughput terminal I/O, binary frames provide lower overhead than JSON.

### Frame Format

Binary WebSocket frames carry **raw bytes** with no framing header:

| Direction | Content | Description |
|-----------|---------|-------------|
| Server → Client | Raw PTY output bytes | Sent as binary WS frame, write directly to xterm.js |
| Client → Server | Raw keystroke bytes | Sent as binary WS frame, forwarded to PTY stdin |

There is no type byte or length prefix — each binary frame is a single opaque payload. Terminal resize is handled via the JSON `TermResize` message (text frame), not binary.

### Examples

Terminal input `ls\n` (3 bytes):
```
6C 73 0A
└── raw UTF-8 bytes: "ls\n"
```

PTY output `$ ` (2 bytes):
```
24 20
└── raw PTY bytes: "$ "
```

## Permission Enforcement

The server enforces permissions on every action:

| Action | Owner | Operator | Viewer |
|--------|-------|----------|--------|
| TermInput | Yes | Yes | Rejected |
| TermResize | Yes | Yes | Rejected |
| ChatMessage | Yes | Yes | Yes |
| CursorMove | Yes | Yes | Yes |

Rejected actions are **silently dropped** — the server does not send an `Error` message or close the connection. The client receives no feedback when a permission check fails. In serialized input mode, only the session owner can send terminal input; other operators' input is also silently dropped.
