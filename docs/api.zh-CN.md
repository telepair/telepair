[English](api.md) | 简体中文

# REST API 参考

Base URL:`http://localhost:7700/api`

除 `/api/health` 和 `POST /api/invite/redeem` 之外,所有端点都需要通过 Bearer token 认证:

```
Authorization: Bearer <token>
```

`POST /api/invite/redeem` 同时接受已认证和匿名调用方 —— 匿名兑换会创建一个新的 guest 用户,并在响应中返回一个新 token(见下文)。

## Health

### GET /api/health

检查服务器状态。无需认证。

**响应** `200 OK`
```json
{ "status": "ok" }
```

## Auth

### GET /api/auth/whoami

返回当前调用方的身份信息。前端的 auth store 启动时会调一次这个端点,把
`currentUserId` 和 `is_admin` 缓存下来,这样 Dashboard 就能在不额外发请求的前提下
正确地按行判断是否显示 owner-only 的操作(审计对话框、关闭按钮)。

**响应** `200 OK`
```json
{
  "user_id": "...",
  "name": "admin",
  "is_admin": true,
  "is_guest": false
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `user_id` | string | 调用方的 UUID |
| `name` | string | 当前显示名 |
| `is_admin` | boolean | 管理员账号为 `true` |
| `is_guest` | boolean | 邀请兑换生成的 scoped guest 为 `true` |

**错误**
- `401 Unauthorized` —— token 缺失或无效。不会返回 403:"我是 guest" 本身就是一个有意义的身份。

## Targets

### GET /api/targets

列出可用的 target。

**响应** `200 OK`
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

创建新会话。调用者自动成为该会话的 owner。

**请求体**
```json
{
  "target_name": "local-shell",
  "input_mode": "serialized"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `target_name` | string | 是 | 要拉起的 target |
| `input_mode` | string | 否 | `"serialized"`(默认)或 `"multiplexed"` |

**响应** `201 Created`
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

**错误**
- `400 Bad Request` —— 传了 `input_mode` 但不是 `serialized` / `multiplexed`(旧版本会悄悄把未知值当成 `serialized`,这反而掩盖了客户端 bug)
- `401 Unauthorized` —— token 缺失或无效
- `403 Forbidden` —— 该 target 在配置里被标记为 `admin_only: true`,但调用方不是管理员
- `404 Not Found` —— target 不存在

### GET /api/sessions

列出对调用方可见的会话。普通用户看到自己拥有的会话以及作为参与者加入过的会话;
**管理员可以看到 workspace 内的全部会话**(这样 admin targets 页面的
`N active sessions` 深链接才能落到非空页面)。
支持按状态 / 目标过滤,以及 `limit`/`offset` 分页。

**查询参数**

| 参数 | 类型 | 说明 |
|------|------|------|
| `status` | string | `active`、`closed`,或 `all`(默认 `all`)。未知值回退到 `all`,所以 URL 里的拼写错误不会把页面整崩。 |
| `target_name` | string | 只返回从该 target 启动的会话。admin targets 页面的 "N active sessions" 深链接就用它。 |
| `limit` | integer | 返回行数上限。缺失或非正数视为不限 |
| `offset` | integer | 分页偏移;非正数会折叠成 `0` |

**响应** `200 OK`
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

| 字段 | 类型 | 说明 |
|------|------|------|
| `closed_reason` | string \| 缺省 | 取值为 `owner`、`reaper`、`startup`、`error` 之一。活跃会话以及 v0.1.0 时代没有该列的旧记录会省略此字段。 |

### DELETE /api/sessions/{session_id}

关闭一个会话。只有会话 owner 能关闭。会停掉 PTY 进程并把会话标记为关闭。

**响应** `204 No Content`

无响应体。

**错误**
- `401 Unauthorized` —— token 缺失或无效
- `403 Forbidden` —— 调用方不是会话 owner
- `404 Not Found` —— 会话不存在

## Invites

### POST /api/sessions/{session_id}/invite

为一个会话创建邀请链接。只有会话 owner 能创建邀请。

**请求体**
```json
{
  "role": "operator",
  "max_uses": 1
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `role` | string | 是 | `"operator"` 或 `"viewer"` |
| `max_uses` | integer | 否 | 最大兑换次数(默认 1) |

| `expires_in_minutes` | integer | 否 | 相对 TTL,单位分钟。与 `expires_at` 互斥;服务端会先把它转成绝对 UTC 时间再落库。**会被夹到** `MAX_INVITE_TTL_MINUTES`(滑杆拉过头视为无害的 UX 误操作)。 |
| `expires_at` | string (ISO 8601) | 否 | 绝对过期时间。如果同时传了 `expires_in_minutes` 和 `expires_at`,以 `expires_at` 为准。**超过** `MAX_INVITE_TTL_MINUTES` 时会被 `400 invalid_input` **拒绝**—— 服务端绝不会悄悄改写调用方显式指定的 wall-clock 时间。 |

**响应** `201 Created`
```json
{
  "token": "abc123...",
  "role": "operator",
  "max_uses": 1,
  "expires_at": "2026-04-04T13:00:00Z",
  "session_id": "550e8400-..."
}
```

原始 `token` 只会返回**一次** —— 数据库只存它的 SHA-256 摘要。现在捕获它,事后没有端点能恢复。

**错误**
- `400 Bad Request` —— `role` 是 `owner`(只能邀请 `operator` / `viewer`)、`max_uses` 为零 / 负数 / 超过 `MAX_INVITE_USES` 上限、TTL 非正数、`expires_in_minutes` 或 `expires_at` 落在过去,或请求体格式不合法
- `401 Unauthorized` —— token 缺失或无效
- `403 Forbidden` —— 调用方不是会话 owner
- `404 Not Found` —— 会话不存在
- `410 Gone` —— 会话存在但已关闭

### GET /api/sessions/{session_id}/invites

按最新优先的顺序,列出该会话下**所有曾经创建过**的邀请(包括已过期、已耗尽的)。
仅 owner 可访问。故意包含 post-mortem 的行,这样管理对话框可以展示 "会话关闭时还
在流通的邀请是哪些",而不用走第二个端点。

响应**不**包含原始 token —— 只有 sha256 摘要(作为 revoke 端点的路径参数)和一个
8 位的前缀标签。丢失的链接无法恢复,请重新 mint。

**响应** `200 OK`
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

| 字段 | 类型 | 说明 |
|------|------|------|
| `token_sha256` | string | 邀请 token 的完整 SHA-256 摘要。对用户不可读;作为 revoke 端点的路径参数使用。 |
| `token_prefix` | string | `token_sha256` 的前 8 个字符 —— UI 用来给每一行打标签。 |
| `remaining_uses` | integer | 服务端预计算的 `max(0, max_uses - used_count)`,保证所有客户端显示一致。 |

**错误**
- `401 Unauthorized` —— token 缺失或无效
- `403 Forbidden` —— 调用方不是会话 owner
- `404 Not Found` —— 会话不存在

### DELETE /api/sessions/{session_id}/invites/{token_sha256}

硬删除一条邀请记录。仅 owner 可操作。路径里的 session id 必须与邀请指向的会话一致
—— 不一致会返回 `404`,防止调用方枚举其他会话的邀请。对同一个 token 重复 revoke
会得到 `404`("已经没了"),前端把这当作刷新列表的信号。

已撤销的邀请无法再被兑换(记录本身已经删掉,所以 `POST /api/invite/redeem` 会按
未知 token 返回 `400`)。

**响应** `204 No Content`

**错误**
- `401 Unauthorized` —— token 缺失或无效
- `403 Forbidden` —— 调用方不是会话 owner
- `404 Not Found` —— 会话不存在、邀请不存在,或邀请属于其他会话

### POST /api/invite/redeem

兑换邀请 token 以加入会话。

**认证是可选的。** 该端点接受三种形态的调用方:

1. **匿名访问者(常见场景)。** 完全不带 `Authorization` header。服务器会消耗一次 `max_uses`,创建一个名为 `guest-<nanoid>` 的新 guest 用户,并在响应的 `token` 字段里返回一个新签发的 token。客户端应把这个 token 保存下来,用于该标签页后续所有 API / WebSocket 调用。
2. **已认证调用方。** 带一个有效 bearer token。服务器会复用调用方已有的身份(方便管理员自己预览邀请链接),响应的 `token` 字段为 `null` —— 因为你已经有 token 了。
3. **失效的 bearer token。** 无效的 `Authorization` header 会被当作匿名访问者处理:服务器会静默丢弃它并创建一个 guest。这是故意设计的,这样带着过期 admin token 的浏览器依然能正常点邀请链接。

**请求体**
```json
{
  "token": "abc123..."
}
```

**响应** `200 OK`
```json
{
  "session_id": "550e8400-...",
  "role": "operator",
  "token": "newly-minted-guest-token-or-null"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `session_id` | string | 要加入的会话 |
| `role` | string | 邀请所赋予的角色(`operator` 或 `viewer`) |
| `token` | string \| null | 新创建的 guest 的 bearer token;如果是已认证调用方复用已有身份,则为 `null` |

**错误**
- `400 Bad Request` —— 邀请 token 未知、已过期,或已达到 `max_uses`(被拒绝的邀请**不会**创建 guest 账号,所以坏链接不会遗留孤儿用户)
- `404 Not Found` —— 邀请指向的会话从未存在
- `410 Gone` —— 目标会话已被关闭;此时邀请**不会**被消耗,所以操作者仍然可以撤销它或把剩余次数转到新会话

## Session Audit

### GET /api/sessions/{session_id}/audit

返回与该会话相关的审计事件,最新优先。仅 owner 可访问 —— 与
`GET /api/sessions/{id}/invites` 共用同一套 403/404 鉴权。已关闭的会话仍然可读
(这本来就是 history 视图的意义),所以 ownership 检查并不要求会话是活跃状态。

响应上限 500 行(目前没有分页接口 —— 真实会话的事件量离这个数量级差好几个零)。
事件分类和写入路径参见 [架构 —— 审计事件](architecture.zh-CN.md#审计事件)。

**响应** `200 OK`
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

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | integer | 单调递增的插入序号 —— 将来需要时可以直接作为分页游标。 |
| `ts` | string (ISO 8601) | 事件发射时间,UTC。 |
| `actor_id` | string \| null | 触发者的 UUID。系统事件和登录失败时为 `null`。 |
| `actor_name` | string \| null | 发射时刻的用户名快照 —— 用户后续改名不会改写历史。 |
| `event_type` | string | 形如以下之一:`session.created`、`session.closed`、`participant.joined`、`participant.left`、`invite.minted`、`invite.redeemed`、`invite.revoked`、`auth.login_success`、`auth.login_failed`、`target.access_denied`、`target.reloaded`。 |
| `session_id` | string \| null | 没有绑定到具体会话的事件为 `null`(登录、目标热重载)。 |
| `detail` | object | 事件专属的 JSON blob。例如 `session.closed` 带 `{reason, duration_s}`、`invite.minted` 带 `{role, max_uses, expires_at}`。 |

**错误**
- `401 Unauthorized` —— token 缺失或无效
- `403 Forbidden` —— 调用方不是会话 owner
- `404 Not Found` —— 会话不存在

## Admin

`/api/admin/*` 下的路由需要 admin bearer token。非管理员会得到 `403`。guest token
无法访问这些路由 —— 调用方虽然已认证,但超出了它的 scope。

### GET /api/admin/targets

返回 in-memory `TargetEngine` 当前加载的所有 target,包括原始的 command / args /
shell 字符串、环境变量 key 的存在性,以及每个 target 的活跃会话数。

**安全提示:** 环境变量的**值永远不会**被序列化出去。每个 key 返回为
`{"key": "PGPASSWORD", "set": true|false}`,其中 `set` 反映当前服务器进程上
`std::env::var(key)` 是否会成功。Telepair 本来就信任任何能写 `targets.yaml` 的人,
但如果通过 HTTP API 再把解析后的 secret 暴露出去,就会超出这个隐式信任边界。

**响应** `200 OK`
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

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | string | yaml 定义的为 `virtual`,内置默认 shell 为 `local`。 |
| `command` | string \| null | 直接来自 `targets.yaml` 的命令字符串。`${VAR}` 占位符保留原样 —— 展开发生在 spawn 时。 |
| `args` | string[] | 直接的 argv 尾部,同样保留 `${VAR}` 占位符。 |
| `shell` | string \| null | `local` 类型 target 的 shell 覆盖。 |
| `admin_only` | boolean | 对应 yaml 里的 `admin_only: true`。 |
| `env` | array | 按 key 排序的 `{key, set}` 列表。**永远不包含值。** |
| `active_sessions` | integer | 对 `sessions` 表做的一次带索引的分组 `SELECT` —— 与 admin UI 上的 chip 数值一致。 |

结果按 `name` 稳定排序,保证 admin UI 每次轮询都不会乱序。

**错误**
- `401 Unauthorized` —— token 缺失或无效
- `403 Forbidden` —— 调用方已认证但不是管理员

### POST /api/admin/targets/reload

从磁盘重新读取 `targets.yaml`,并把新的 `TargetEngine` 原子地安装到应用状态里。
切换使用 `arc_swap`,所以已经在飞的请求仍然走旧 engine 跑完,之后的请求看到新
engine —— 整个过程没有锁窗口。

成功时会发一条 `target.reloaded` 的审计事件,detail 里带 `{path, targets}`。

**响应** `200 OK`
```json
{
  "path": "/home/admin/.telepair/targets.yaml",
  "targets": 4
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `path` | string | 被重新读取的绝对路径。 |
| `targets` | integer | 切换后的 target 数量。 |

**错误**
- `400 Bad Request`,body 为 `{ "reason": "no_targets_path", "message": "..." }` —— 服务器启动时没有配置 `targets.yaml` 路径,没有东西可以重载。旧 engine 保持不动。
- `400 Bad Request`,body 为 `{ "reason": "parse_error", "message": "...", "path": "..." }` —— 磁盘上的文件当前不合法。旧 engine 保持不动,`message` 原样带上 parse 错误,方便管理员修复 yaml。
- `400 Bad Request`,body 为 `{ "reason": "still_referenced", "message": "...", "targets": [{ "target": "...", "active_sessions": N }, ...] }` —— 新的 `targets.yaml` 会删掉仍有活跃会话的 target。旧 engine 保持不动,`targets` 数组精确列出哪些 target 正在阻塞重载以及各自的活跃会话数,管理员可以先关掉这些会话(或在 yaml 中恢复 target)再重试。admin 页面会把它渲染为常驻的 banner,而不是一闪而过的 toast。
- `401 Unauthorized` —— token 缺失或无效
- `403 Forbidden` —— 调用方已认证但不是管理员
