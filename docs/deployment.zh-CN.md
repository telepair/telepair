[English](deployment.md) | 简体中文

# 部署指南

## 单机部署

最简方案:一个二进制,一台机器。

### 构建

```bash
# 构建 release 二进制
cargo build --release

# 构建前端
cd web && npm install && npm run build && cd ..
```

### 运行

```bash
./target/release/telepair --web-dir web/dist
```

会在 7700 端口启动所有角色(agent + control + gateway)。首次运行会把 admin token 打印到控制台 —— 请妥善保存。

### Systemd 服务

```ini
# /etc/systemd/system/telepair.service
[Unit]
Description=telepair terminal collaboration
After=network.target

[Service]
Type=simple
User=telepair
ExecStart=/usr/local/bin/telepair --host 127.0.0.1 --web-dir /opt/telepair/web/dist
WorkingDirectory=/opt/telepair
Restart=on-failure
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

```bash
sudo useradd -r -s /bin/false telepair
sudo mkdir -p /opt/telepair
sudo cp target/release/telepair /usr/local/bin/
sudo cp -r web/dist /opt/telepair/web/dist
sudo systemctl enable --now telepair
```

### 反向代理(nginx)

```nginx
server {
    listen 443 ssl;
    server_name telepair.example.com;

    ssl_certificate /etc/letsencrypt/live/telepair.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/telepair.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:7700;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    # WebSocket 升级
    location /ws/ {
        proxy_pass http://127.0.0.1:7700;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_read_timeout 86400;
    }
}
```

要点:
- WebSocket 需要 `Upgrade` 和 `Connection` header
- `proxy_read_timeout 86400` 防止 nginx 关闭长连接 WS

## Docker

### Dockerfile

```dockerfile
# 构建后端
FROM rust:1.94 AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
RUN cargo build --release

# 构建前端
FROM node:22 AS frontend
WORKDIR /build
COPY web/ web/
RUN cd web && npm install && npm run build

# Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=backend /build/target/release/telepair /usr/local/bin/
COPY --from=frontend /build/web/dist /opt/telepair/web/dist

EXPOSE 7700
VOLUME /root/.telepair

CMD ["telepair", "--web-dir", "/opt/telepair/web/dist"]
```

### 运行

```bash
docker build -t telepair .
docker run -d -p 7700:7700 -v telepair-data:/root/.telepair telepair
```

### Docker Compose

```yaml
services:
  telepair:
    build: .
    ports:
      - "7700:7700"
    volumes:
      - telepair-data:/root/.telepair
    environment:
      - RUST_LOG=info
    restart: unless-stopped

volumes:
  telepair-data:
```

## 配置

### 虚拟目标(Virtual Targets)

挂载一个自定义的 targets 配置:

```bash
# Docker
docker run -v ./targets.yaml:/root/.telepair/targets.yaml telepair

# Systemd
./telepair --targets /etc/telepair/targets.yaml --web-dir web/dist
```

targets.yaml 的格式详见 [README](../README.zh-CN.md#虚拟目标virtual-targets)。

### CORS

telepair 在生产环境中把前端作为**同源**(same-origin)静态资源一起提供,所以当浏览器从提供 `index.html` 的同一个主机请求 `/api` 和 `/ws` 时,浏览器会直接绕过 CORS,完全不需要额外配置。只有当前端和 API 不在同一个 origin 时,你才需要关心 CORS:

- **反向代理部署(推荐)。** nginx 终止 TLS,既提供 `/` 的前端文件,又把 `/api` 和 `/ws/` 代理到 `127.0.0.1:7700`。从浏览器的角度看就是同源 —— 不需要任何 CORS flag。
- **直接暴露,没有代理。** 如果前端和 API 不在一个域名下(例如用 CDN 分发 `web/dist`,而 API 跑在别处),你必须传入前端的精确 origin:

  ```bash
  ./telepair --web-dir web/dist \
             --allowed-origins https://telepair.example.com
  ```

  多个 origin 用逗号分隔。畸形的 origin 会让启动直接失败 —— 拼错绝不会悄悄退化成空白 allowlist。
- **默认(不传 flag)。** 回退到 `http://localhost:5173, http://127.0.0.1:5173`,只为 Vite dev server 服务。这是刻意设计的 —— **不**是"允许任意 origin"。早期版本默认 `*`,那是一个大坑。
- **`--allow-any-origin`。** 只在 dev 环境,或上游由反向代理强制 CORS 的情况下使用。会覆盖 `--allowed-origins`。

### 环境变量

下表每个环境变量都有对应的 CLI flag(`--data-dir`、`--smtp-host` 等);两者同时设置时 flag 获胜。

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `RUST_LOG` | `info` | 日志级别(`debug`、`info`、`warn`、`error`) |
| `TELEPAIR_DATA_DIR` | `~/.telepair` | 覆盖数据目录(DB、admin token、targets.yaml、recordings)。 |
| `TELEPAIR_TRUST_FORWARDED_HEADERS` | `false` | 允许每 IP 注册限流器信任 `X-Forwarded-For` / `X-Real-IP`。**仅当** telepair 跑在会对每个请求重写这两个 header 的反向代理之后才打开;直接暴露时启用会让任何客户端伪造 header 绕过限流。 |
| `TELEPAIR_SMTP_HOST` | *(未设置)* | SMTP 服务器主机名。设置后才启用邮箱注册;不设置则 OTP 路径禁用。 |
| `TELEPAIR_SMTP_PORT` | `587` | SMTP 端口(STARTTLS)。 |
| `TELEPAIR_SMTP_USER` | *(未设置)* | SMTP 用户名。 |
| `TELEPAIR_SMTP_PASS` | *(未设置)* | SMTP 密码。 |
| `TELEPAIR_SMTP_FROM` | *(未设置)* | SMTP 发件人地址,形如 `"Telepair <noreply@example.com>"`。 |
| `TELEPAIR_RECORDING_ENABLED` | `false` | 会话录制的总开关。关闭时任何会话都不会被录制。 |
| `TELEPAIR_RECORDING_TTL_DAYS` | `30` | 保留天数。`0` 表示永久(不触发 TTL 清理)。 |
| `TELEPAIR_RECORDING_DIR` | `<data-dir>/recordings` | `.cast` 文件的存放目录。 |

### 数据目录

telepair 所有持久化数据放在 `~/.telepair/`(可用 `--data-dir` / `TELEPAIR_DATA_DIR` 覆盖):

| 路径 | 用途 |
|------|------|
| `telepair.db` | SQLite 数据库(users、sessions、participants、invites、audit_events、recordings、recording_shares) |
| `admin_token` | admin bearer token(首次运行时创建,权限 0600) |
| `targets.yaml` | 虚拟目标定义(可选) |
| `recordings/` | 会话录制 `.cast` 文件,每个录制一个,按 recording id 命名;首次录制时创建。可用 `--recording-dir` / `TELEPAIR_RECORDING_DIR` 覆盖。 |

备份 `telepair.db`(启用录制时再加上 `recordings/`)即可保住用户账号、会话历史和回放。

## 会话录制

会话录制**默认关闭**,必须显式 opt-in。使用 `--recording-enabled`(或 `TELEPAIR_RECORDING_ENABLED=true`)启用:

```bash
./telepair --web-dir web/dist \
           --recording-enabled \
           --recording-ttl-days 30
```

启用后你会得到:

- 会话所有者可以从会话内的 Recording 面板 **开始 / 停止** 录制(`POST /api/sessions/{id}/recording/{start,stop}`)。同一会话同一时刻只能有一个活跃录制。
- Owner 和 admin 可以 **列表 / 回放 / 删除** 自己的录制;admin 还能通过 `GET /api/admin/recordings` 看到所有人的。
- Owner 可以通过 `POST /api/recordings/{id}/shares` 生成 **带签名的分享链接**(TTL + 最多使用次数)。匿名观看者访问 `/recordings/{id}/play#token=...` 会绕过 `AuthGuard`；播放器读取 URL fragment 后立即用 `replaceState` 清除,再以 `X-Share-Token: <raw>` 请求头拉取 `/api/recordings/{id}/data`。token 会通过单条 `UPDATE … RETURNING` 同时校验 recording id、剩余使用次数和过期时间,无 TOCTOU 窗口。使用 fragment + header 的组合是为了日志卫生 —— NGINX `$request`、ALB `request_url`、CloudFront standard log 默认都会记录 query string,但不会记录自定义请求头,所以原始 token 永远不会出现在访问日志中。
- 后台 **cleaner** 每隔数分钟扫描 `expires_at`,删除过期行。候选集中**始终**排除活跃录制(`status = 'recording'`)作为防御性兜底。`expires_at IS NULL` 表示"永久保留"。

存储细节:

- 录制以 asciicast v2 `.cast` 文件存放在 `--recording-dir`(默认 `<data-dir>/recordings/`)下,以 recording id 命名。
- 元数据(`file_size`、`duration_ms`、`event_count`、`status`、`expires_at`)存入 `recordings` 表,share token 存入 `recording_shares` 表。两张表都通过 `ON DELETE CASCADE` 跟随父行级联清理。
- 若录制期间 writer 因背压丢过事件,该录制会被最终化为 `status = 'failed'`(而不是 `completed`),这样 "completed" 永远意味着 "asciicast 完整无缺口"。
- 启用录制会占用磁盘 —— 粗略量级是每个活跃会话每秒几 KB 的 PTY 输出,再加上聊天 / 参与者事件。请据此规划卷容量或调小 `TELEPAIR_RECORDING_TTL_DAYS`。

录制**关闭**时,前端的 Recording 面板会隐藏,`POST /api/sessions/{id}/recording/start` 返回 `403 Forbidden`("session recording is disabled on this server"),也不会有 writer 被拉起。读路径(`GET /api/recordings`、`GET /api/recordings/{id}/data`、shares 等)照常工作,这样在运维把开关关掉之后,已有的录制依然可回放。

## 安全注意事项

- **生产环境务必用 TLS**(走反向代理)
- **保存好 admin token** —— 首次运行时会打印,同时也写入 `~/.telepair/admin_token`(权限 0600)作为备份。丢了?跑 `telepair admin show-token` 即可打印已缓存的 token
- **收紧网络访问** —— telepair 默认绑定 `0.0.0.0`;在反向代理后面部署时请用 `--host 127.0.0.1`,这样监听端口绝不会对外暴露
- **锁死 CORS** —— 绝对不要在直接暴露的主机上用 `--allow-any-origin`。要么跑在代理后面(同源,完全不需要 CORS),要么用 `--allowed-origins` 显式列出前端 origin
- **邀请 token** 默认单次使用;仅在必要时才调大 `max_uses`
- **PTY 访问等同于 shell 访问** —— 请谨慎管理谁拿到 operator/owner 角色
