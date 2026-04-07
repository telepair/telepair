[English](architecture.md) | 简体中文

# 架构

telepair 是一个由 5 个 crate 组成的 Cargo workspace,每个 crate 负责一个独立的层次。单一二进制 `telepair` 通过角色 flag 把这些层组合起来。

## Crate 依赖图

```
telepair-cli
├── telepair-gateway ──┬── telepair-core
│                      └── telepair-agent ── telepair-core
├── telepair-control ──── telepair-core
└── telepair-core
```

## Crate 职责

### telepair-core

基础 crate。不包含业务逻辑,只提供共享抽象。

| 模块 | 用途 |
|------|------|
| `session.rs` | 领域类型:`User`、`Session`、`Participant`、`InviteToken`、`InputMode`、`SessionStatus` |
| `permission.rs` | `Role` 枚举(Owner/Operator/Viewer),带能力方法(`can_input`、`can_resize`、`can_manage`) |
| `protocol.rs` | `ClientMessage` / `ServerMessage` 枚举(JSON,`#[serde(tag = "type")]`);PTY 输出作为原始二进制 WS 帧发送 |
| `storage.rs` | 异步 `Storage` trait —— users、sessions、participants、invite tokens 的 CRUD |
| `storage/sqlite.rs` | 基于 sqlx 的 `SqliteStorage` 实现 |
| `auth.rs` | `TokenAuthProvider` —— token 使用 SHA-256 哈希校验(原始 token 在创建时返回一次,之后不再持久化) |
| `target.rs` | `Target` 和 `TargetKind` 定义 |
| `error.rs` | `Error` 枚举(Auth、NotFound、Storage、Internal) |

### telepair-agent

管理 PTY 进程和虚拟目标解析。

| 模块 | 用途 |
|------|------|
| `pty.rs` | `PtyManager` —— 通过 portable-pty 拉起 shell,处理读/写/resize |
| `virtual_target.rs` | `TargetEngine` —— 加载 YAML 配置,把 target 名解析为命令,做环境变量替换 |

### telepair-control

协调核心抽象的业务逻辑服务层。

| 模块 | 用途 |
|------|------|
| `session_service.rs` | `SessionService` —— 创建/关闭会话,管理参与者,委托给 Storage |
| `target_service.rs` | `TargetService` —— 包装 TargetEngine,提供目标列表和解析能力 |

### telepair-gateway

面向客户端的一层。跑 HTTP 服务器、WebSocket 升级,并对外暴露前端静态文件。

| 模块 | 用途 |
|------|------|
| `lib.rs` | Axum 路由设置与路由定义 |
| `state.rs` | `AppState` —— 共享应用状态(storage、auth、services、session hubs) |
| `http.rs` | REST handler:health、targets、sessions、invites |
| `ws.rs` | WebSocket handler —— 认证、角色校验、消息分发、PTY I/O 桥接 |
| `session_hub.rs` | `SessionHub` —— 单会话状态:PTY 进程、已连接参与者、广播通道 |

### telepair-cli

最小化的二进制 crate。解析 CLI 参数、初始化 storage、配置 tracing、启动服务器。

## 运行时架构

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

## 数据流

### 终端 I/O

1. 用户在 xterm.js 中输入
2. 前端把原始 UTF-8 字节作为**二进制 WebSocket 帧**发送(无 JSON 包装)
3. WS handler 检查 `role.can_input()` —— 如果是 viewer 则静默丢弃
4. 字节经由 `SessionHub` 命令通道进入 PTY
5. PTY 输出通过 `output_tx` 广播给所有已连接参与者
6. 每个 WS handler 以原始二进制 WS 帧转发一个 chunk
7. 前端直接把这些字节写入 xterm.js

### 协作消息

1. 客户端通过 WS 发送 `ChatMessage { text }`
2. WS handler 包装成带服务端时间戳的 `PeerChat { user_id, name, text, ts }`
3. `SessionHub` 经 `collab_tx` 广播给所有参与者
4. 各 WS handler 转发给自己的客户端

### 会话生命周期

1. 客户端带 target name 调用 `POST /api/sessions`
2. `SessionService` 在 SQLite 中创建会话,把 owner 作为参与者加入
3. Owner 通过 `WS /ws/session/{id}` 连入,发送 `SessionJoin`
4. `SessionHub` 拉起 PTY,启动 I/O 循环
5. Owner 通过 `POST /api/sessions/{id}/invite` 创建邀请
6. 协作者通过 `POST /api/invite/redeem` 兑换邀请
7. 协作者连入同一 WS 端点,`PeerJoined` 广播给所有参与者

## 广播通道

每个活跃会话都有两个独立的广播通道:

| 通道 | 容量 | 内容 |
|------|------|------|
| `output_tx` | 256 条 | PTY 字节(作为原始二进制 WS 帧转发) |
| `collab_tx` | 64 条 | `PeerJoined`、`PeerLeft`、`PeerChat`、`PeerCursor` |

分离是为了确保高频的终端输出不会把协作消息饿死。两者都用 `tokio::broadcast` —— 接收端太慢时会丢掉最旧的消息。

## 存储 Schema

```sql
users (id, name, token_sha256, is_admin, created_at, updated_at)
sessions (id, owner_id, target_name, input_mode, status, created_at, closed_at)
participants (session_id, user_id, role, joined_at, left_at)
invite_tokens (token_sha256, session_id, role, max_uses, used_count, expires_at)
```

所有 ID 都是存为 TEXT 的 UUID。时间戳是 ISO 8601 TEXT。`Storage` trait 是异步且与具体实现无关的 —— v1 的后端是 SQLite。

## 安全模型

- **认证(Authentication)**:`Authorization` header 里的 bearer token。Token 只以 SHA-256 hex 摘要形式存储 —— 原始值仅在创建时返回给调用方一次,此后不再持久化。
- **授权(Authorization)**:按会话做基于角色的授权。WS handler 在每次 input / resize 动作时都检查角色。
- **邀请 token**:默认单次使用。以 SHA-256 摘要存储;原子化的 `used_count < max_uses` 自增防止并发兑换的竞态。
- **CORS**:可通过 `--allowed-origins`(逗号分隔的绝对 URL)配置。不设置时,服务器默认**仅允许 loopback dev 源**(`http://localhost:5173`、`http://127.0.0.1:5173`)以匹配 Vite dev server。畸形的 origin 会让启动失败 —— 拼错不会悄悄让 allowlist 变宽。`--allow-any-origin` 显式启用 `Access-Control-Allow-Origin: *`,仅适合 dev 或下游已有强制 CORS 的反向代理。生产环境直连(无反代)时,请把 `--allowed-origins` 设为你信任的前端域名。
