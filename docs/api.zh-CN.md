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

列出活跃的会话。

**响应** `200 OK`
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

**响应** `201 Created`
```json
{
  "token": "abc123...",
  "role": "operator",
  "max_uses": 1,
  "session_id": "550e8400-..."
}
```

**错误**
- `400 Bad Request` —— `role` 是 `owner`(只能邀请 `operator` / `viewer`)、`max_uses` 为零或负数,或请求体格式不合法
- `401 Unauthorized` —— token 缺失或无效
- `403 Forbidden` —— 调用方不是会话 owner
- `404 Not Found` —— 会话不存在

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
