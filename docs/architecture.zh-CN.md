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
| `session.rs` | 领域类型:`User`、`Session`、`Participant`、`InviteToken`、`InputMode`、`SessionStatus`、`CloseReason` |
| `permission.rs` | `Role` 枚举(Owner/Operator/Viewer),带能力方法(`can_input`、`can_resize`、`can_manage`) |
| `protocol.rs` | `ClientMessage` / `ServerMessage` 枚举(JSON,`#[serde(tag = "type")]`);PTY 输出作为原始二进制 WS 帧发送 |
| `storage.rs` | 异步 `Storage` trait —— users、sessions、participants、invite tokens、audit events 的 CRUD |
| `storage/sqlite.rs` | 基于 sqlx 的 `SqliteStorage` 实现,启动时通过 `run_migrations()` 幂等追加新列 / 新表 |
| `auth.rs` | `TokenAuthProvider` —— token 使用 SHA-256 哈希校验(原始 token 在创建时返回一次,之后不再持久化) |
| `target.rs` | `Target` 和 `TargetKind` 定义 |
| `audit.rs` | `AuditEvent`、`AuditEventType`、`AuditSink` trait —— 只追加的事件日志,支撑 `telepair admin audit` 和应用内会话时间线 |
| `error.rs` | `Error` 枚举 —— Auth (401)、SessionNotFound/TargetNotFound (404)、SessionClosed (410)、PermissionDenied (403)、InvalidInput (400)、Conflict (409)、RateLimited (429)、ServiceUnavailable (503)、Internal/Storage/Io (500)。每个 variant 都有 `http_status()` 方法做统一的 HTTP 状态码映射。 |

### telepair-agent

管理 PTY 进程和虚拟目标解析。

| 模块 | 用途 |
|------|------|
| `pty.rs` | `PtyManager` —— 通过 portable-pty 拉起 shell,处理读/写/resize |
| `virtual_target.rs` | `TargetEngine` —— 加载 YAML 配置,把 target 名解析为命令,做环境变量替换 |

### telepair-control

协调核心抽象的业务逻辑服务层。从 0.1.1 起,这是生产代码中**唯一**直接触碰 `Storage` trait 的层次 —— gateway 的每一次读写都走 service。HTTP handler 与 WebSocket hub 里因此不再残留业务规则,service 也可以脱离 HTTP 基础设施做单元测试。

| 模块 | 用途 |
|------|------|
| `session_service.rs` | `SessionService` —— 会话生命周期(`create_session`、`close_session(reason)`)、参与者查询(`list_participants`、`list_sessions_for_user`)、授权辅助(`require_owner`)以及跨层聚合查询(如 `active_session_counts_per_target`)。会话创建 / 关闭以及启动清理都会在这里发审计事件。 |
| `invite_service.rs` | `InviteService` —— 邀请生命周期(`create`、`redeem`、`list_for_session`、`revoke`)。`MAX_INVITE_USES` / TTL 校验、跨会话 scoped-guest 检查、兑换成功后的 guest mint 都收敛在这里,并发出 `invite.minted` / `invite.redeemed` / `invite.revoked` 审计事件。 |
| `auth_service.rs` | `AuthService` —— 基于邮箱的注册（含 OTP 验证）、密码登录（Argon2 哈希）、管理员用户管理（`list_accounts`、`set_session_access`）。负责 SMTP 发送 OTP（通过 lettre）、登录限流（5 次错误后锁定 15 分钟）、以及防枚举的统一错误折叠。发出 `auth.register_rejected` / `auth.register_completed` / `auth.verify_failed` / `auth.login_failed` / `auth.user_enabled` / `auth.user_disabled` 审计事件。 |
| `user_target_service.rs` | `UserTargetService` —— 用户自有目标的 CRUD（`create`、`update`、`delete`、`get`、`list`、`resolve_by_id`）。每次变更都校验所有权,在活跃会话引用该目标时阻止修改 / 删除（通过 `Conflict` 错误实现引用完整性），resolve 时故意不做 `${VAR}` 展开以防止用户提交的命令字符串泄露进程环境变量。 |
| `target_service.rs` | `TargetService` —— 包装 `TargetEngine`,提供目标列表和解析能力。 |

### telepair-gateway

面向客户端的一层。跑 HTTP 服务器、WebSocket 升级,并对外暴露前端静态文件。

| 模块 | 用途 |
|------|------|
| `lib.rs` | Axum 路由设置与路由定义 |
| `state.rs` | `AppState` —— 共享应用状态:storage、auth、`SessionService`、`InviteService`、`AuthService`、`UserTargetService`、`Arc<ArcSwap<TargetEngine>>`(用于原子化的目标热重载)、`Arc<dyn AuditSink>`、以及 `SessionHub` |
| `http.rs` | REST handler:health、targets、sessions、invites(list / revoke)、会话历史、会话审计、whoami、admin targets(list + reload)。所有 handler 都走 service —— 生产代码不再直接访问 `.storage()`。 |
| `ws.rs` | WebSocket handler —— 认证、角色校验、消息分发、PTY I/O 桥接,`participant.joined` / `participant.left` 审计事件发射 |
| `session_hub.rs` | `SessionHub` —— 单会话状态:PTY 进程、已连接参与者、广播通道。持有 `Arc<SessionService>`(而非裸 Storage),所以空闲清理的关闭也会和所有者主动关闭走同一条审计路径,带 `CloseReason::Reaper`。 |

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
                           │    ├── AuthService                │
                           │    ├── UserTargetService          │
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

### 邮箱注册与登录

1. 客户端调用 `POST /api/auth/register`,携带 email、password 和 display name
2. `AuthService` 用 Argon2 哈希密码、生成 6 位 OTP、写入 `pending_registrations` 行,并通过 SMTP 发送 OTP
3. 客户端调用 `POST /api/auth/verify`,携带 email 和 OTP 码
4. `AuthService` 对比 pending 行中的验证码（带尝试次数限制和 TTL），成功后物化 `users` 行,返回 bearer token
5. 后续登录时客户端调用 `POST /api/auth/login`，携带 email 和 password
6. `AuthService` 校验 Argon2 哈希,执行 5 次错误锁定窗口,成功时清空计数器,返回新 bearer token
7. 管理员可通过 `PUT /api/admin/users/{id}/session-access` 启用/禁用用户的会话权限 —— 登录本身不受影响（密码重置、查看历史仍可用），但会话创建和 WS 接入会被阻止

### 会话生命周期

1. 客户端带 target name（或 user target ID）调用 `POST /api/sessions`
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

所有 ID 都是存为 TEXT 的 UUID。时间戳是 ISO 8601 TEXT。`Storage` trait 是异步且与具体实现无关的 —— v1 的后端是 SQLite。

**Schema 演进（0.1.x）。** 迁移状态保存在单一的 `migrations/001_initial.sql` 中，每次启动都会整份加载。`telepair-core/src/storage/sqlite.rs` 里的 `run_migrations()` 会先执行完整 SQL 文件，再通过 `pragma_table_info` 做列存在性检查，以幂等方式给旧库补上新列 —— 如 `sessions.closed_reason`、`sessions.user_target_id`、以及 `users` 表的邮箱认证字段（`email`、`password_hash`、`session_enabled`、`login_failed_count`、`login_locked_until`）。新表（`audit_events`、`pending_registrations`、`user_targets`）用 `CREATE TABLE IF NOT EXISTS` 达成同样的效果。这让 0.1.x 范围内的原地升级保持可用，而不必引入正式的迁移框架；真正出现 schema 冲突时，pre-1.0 的"删库重建"兜底仍然适用。正式迁移框架留给后续 minor 版本。

### 审计事件

`audit_events` 表是只追加(append-only)的。每一行都是一次安全相关状态转移的不可变记录 —— 登录、会话生命周期、参与者加入 / 离开、邀请发放 / 兑换 / 撤销、目标访问被拒绝、以及目标热重载。高频事件(聊天、光标、PTY 字节流)**不**入审计:这类事件会让表爆炸,而且它们承载的信息已经被更粗粒度的事件覆盖。

| 列 | 用途 |
|----|------|
| `id` | 自增 i64 主键 —— 稳定的插入顺序,便于分页读取 |
| `ts` | ISO 8601 UTC 字符串,建立索引用于时间范围查询 |
| `actor_id` | 触发者的 user id(系统事件和登录失败时为空) |
| `actor_name` | 发射时刻的用户名快照 —— 用户后续改名不应改写历史 |
| `event_type` | 形如 `session.created` 或 `invite.revoked` 的标签字符串,通过 `AuditEventType` 的 `#[serde(rename = "...")]` 序列化 |
| `session_id` | 建立索引 —— 支撑每会话时间线视图以及 `telepair admin audit --session <id>` 过滤 |
| `detail` | 事件专属字段的 JSON blob:`reason`、`duration_s`、`role`、`max_uses`、`expires_at` 等 |

四个索引覆盖四种查询形态:时间范围(`idx_audit_ts`)、单会话时间线(`idx_audit_session`)、单 actor 历史(`idx_audit_actor`)、按类型扫描(`idx_audit_type`)。写入端有 `SessionService`、`InviteService`、`AuthService` 登录路径、以及 admin targets reload handler;读取端有 `GET /api/sessions/{id}/audit` 和 `telepair admin audit` CLI。

## 安全模型

- **认证（Authentication）**：`Authorization` header 里的 bearer token。Token 只以 SHA-256 hex 摘要形式存储 —— 原始值仅在创建时返回给调用方一次，此后不再持久化。邮箱注册提供了第二条认证路径：用户以 email + password 注册，通过 SMTP 发送的 6 位 OTP 验证后获得 bearer token。密码使用 Argon2 哈希（每行独立 salt）；OTP 有效期 15 分钟，每个邮箱 60 秒限流。
- **登录限流**：密码登录执行 5 次错误锁定 —— 连续 5 次错误密码后账户锁定 15 分钟。一次成功登录即清空计数器。所有失败模式（未知邮箱、错误密码、已锁定）返回完全相同的错误形态以防止枚举。
- **待注册**：`pending_registrations` 表是一个无权限的暂存区 —— 在 OTP 验证通过前不会创建 `users` 行。对已验证邮箱的重复注册会静默成功（不泄露信息）并写入审计行。
- **管理员审批门控**：邮箱注册的新用户默认 `session_enabled = FALSE`。管理员须通过 `PUT /api/admin/users/{id}/session-access` 将其置为 `TRUE` 后用户才能创建会话或接入 WebSocket。登录本身不受限，用户可在等待审批期间查看历史或修改密码。
- **授权(Authorization)**:按会话做基于角色的授权。WS handler 在每次 input / resize 动作时都检查角色。
- **邀请 token**:默认单次使用。以 SHA-256 摘要存储;原子化的 `used_count < max_uses` 自增防止并发兑换的竞态。
- **CORS**:可通过 `--allowed-origins`(逗号分隔的绝对 URL)配置。不设置时,服务器默认**仅允许 loopback dev 源**(`http://localhost:5173`、`http://127.0.0.1:5173`)以匹配 Vite dev server。畸形的 origin 会让启动失败 —— 拼错不会悄悄让 allowlist 变宽。`--allow-any-origin` 显式启用 `Access-Control-Allow-Origin: *`,仅适合 dev 或下游已有强制 CORS 的反向代理。生产环境直连(无反代)时,请把 `--allowed-origins` 设为你信任的前端域名。
