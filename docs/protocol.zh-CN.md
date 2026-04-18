[English](protocol.md) | 简体中文

# WebSocket 协议

端点:`ws://localhost:7700/ws/session/{session_id}`

telepair 在单条 WebSocket 连接上使用一种混合协议:
- **文本帧**:JSON 消息,用于控制和协作
- **二进制帧**:原始 PTY 字节 —— 服务器 → 客户端(PTY 输出)以及
  客户端 → 服务器(按键 / 粘贴)

## 连接流程

```
Client                              Server
  │                                    │
  │──── WS connect ───────────────────▶│
  │                                    │
  │──── SessionJoin (JSON) ───────────▶│  auth + 角色查询
  │                                    │
  │◀─── SessionState (JSON) ──────────│  会话信息 + 参与者列表
  │                                    │
  │◀─── PTY output (binary) ──────────│  PTY 输出流
  │──── keystrokes (binary) ──────────▶│  原始字节,无封装
  │                                    │
  │◀─── PeerJoined ──────────────────│  协作事件
  │◀─── PeerChat ────────────────────│
  │──── ChatMessage ─────────────────▶│
  │                                    │
```

## JSON 消息(文本帧)

所有 JSON 消息用 `type` 字段做判别(Rust 中是 `#[serde(tag = "type")]`)。

### 客户端到服务器

#### SessionJoin

必须是连接建立后的第一条消息。用于认证客户端。

```json
{
  "type": "SessionJoin",
  "session_id": "550e8400-...",
  "token": "bearer-token-here"
}
```

如果认证失败或用户不是参与者,连接会被关闭。

#### 终端输入(二进制帧,不是 JSON)

按键作为**原始二进制 WebSocket 帧**发送 —— 客户端直接把 UTF-8 字节写入 socket(无 JSON 包装、无封装头)。需要 `operator` 或 `owner` 角色。

例如 `ls\n` 作为一条 3 字节的二进制帧发送:`[0x6c, 0x73, 0x0a]`。

#### TermResize

调整 PTY 大小。需要 `operator` 或 `owner` 角色。

```json
{
  "type": "TermResize",
  "cols": 120,
  "rows": 40
}
```

#### ChatMessage

发送聊天消息。所有角色都能聊天。

```json
{
  "type": "ChatMessage",
  "text": "Hello everyone!"
}
```

### 服务器到客户端

#### SessionState

在 `SessionJoin` 成功后立即发送。

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

| 字段 | 类型 | 说明 |
|------|------|------|
| `chat_history` | array | 有界的会话内聊天回放(从旧到新,短暂的 —— 会话关闭即销毁)。每条 entry 与 `PeerChat` 字段一一对应,同一套渲染器即可处理回放与实时消息。 |
| `recording` | object \| null | 加入时若有活跃录制,则为 `{ recording_id, started_at }`;否则缺省(或为 `null`),这样后加入者无需额外 REST 调用就能看到录制指示。 |

#### PeerJoined

有新参与者连入时广播。

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

参与者断开时广播。

```json
{
  "type": "PeerLeft",
  "user_id": "..."
}
```

#### PeerChat

广播聊天消息,带服务端打的时间戳。

```json
{
  "type": "PeerChat",
  "user_id": "...",
  "name": "alice",
  "text": "Hello everyone!",
  "ts": "2026-04-04T12:05:30.123Z"
}
```

#### PeerRoleChanged

当会话 owner 通过 `PUT /api/sessions/:id/participants/:user_id/role` 改变
某个参与者的角色时广播。所有已连接的客户端都会收到,以便参与者列表同步更新。
WS handler 还会拦截指向当前连接的消息,就地重新计算输入权限,无需重连。

```json
{
  "type": "PeerRoleChanged",
  "user_id": "...",
  "new_role": "viewer"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `user_id` | string | 角色被变更的参与者 UUID |
| `new_role` | string | 新角色:`"operator"` 或 `"viewer"` |

#### RecordingStarted

会话开始录制时广播给所有参与者。客户端据此切换会话内的 "● REC" 指示灯,无需轮询 REST 端点。需要服务端以 `--recording-enabled` 启动。

```json
{
  "type": "RecordingStarted",
  "recording_id": "e4a5b2c1-..."
}
```

#### RecordingStopped

当前活跃录制停止时广播(显式调用 `POST /api/sessions/{id}/recording/stop`,或会话关闭时隐式停止)。客户端据此隐藏录制指示并清除缓存的活跃录制状态。

```json
{
  "type": "RecordingStopped",
  "recording_id": "e4a5b2c1-..."
}
```

#### Error

服务端错误通知。

```json
{
  "type": "Error",
  "code": "PERMISSION_DENIED",
  "message": "viewers cannot send input"
}
```

## 二进制帧

PTY I/O 在双向都使用原始二进制 WebSocket 帧 —— 没有类型字节,也没有长度前缀,就是不透明的负载。服务器把客户端帧直接写入 PTY writer,又把 PTY 输出原样流回客户端。终端 resize 仍然走 JSON 的 `TermResize` 消息。

PTY 输出 `$ `(2 字节)示例:
```
24 20
└── 原始 PTY 字节: "$ "
```

## 权限校验

服务器对每一个动作都做权限校验:

| 动作 | Owner | Operator | Viewer |
|------|-------|----------|--------|
| 终端输入(二进制) | Yes | Yes | Rejected |
| TermResize | Yes | Yes | Rejected |
| ChatMessage | Yes | Yes | Yes |

被拒绝的动作**静默丢弃** —— 服务器不会回 `Error` 消息,也不会关连接。权限校验失败时客户端不会收到任何反馈。在 serialized 输入模式下,只有会话 owner 能发送终端输入;其他 operator 的输入也会被静默丢弃。
