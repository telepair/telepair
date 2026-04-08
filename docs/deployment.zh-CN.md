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
FROM rust:1.86 AS backend
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

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `RUST_LOG` | `info` | 日志级别(`debug`、`info`、`warn`、`error`) |

### 数据目录

telepair 所有持久化数据放在 `~/.telepair/`:

| 文件 | 用途 |
|------|------|
| `telepair.db` | SQLite 数据库(users、sessions、participants、invites) |
| `admin_token` | admin bearer token(首次运行时创建,权限 0600) |
| `targets.yaml` | 虚拟目标定义(可选) |

备份 `telepair.db` 就可以保住用户账号和会话历史。

## 安全注意事项

- **生产环境务必用 TLS**(走反向代理)
- **保存好 admin token** —— 首次运行时会打印,同时也写入 `~/.telepair/admin_token`(权限 0600)作为备份。丢了?跑 `telepair admin show-token` 即可打印已缓存的 token
- **收紧网络访问** —— telepair 默认绑定 `0.0.0.0`;在反向代理后面部署时请用 `--host 127.0.0.1`,这样监听端口绝不会对外暴露
- **锁死 CORS** —— 绝对不要在直接暴露的主机上用 `--allow-any-origin`。要么跑在代理后面(同源,完全不需要 CORS),要么用 `--allowed-origins` 显式列出前端 origin
- **邀请 token** 默认单次使用;仅在必要时才调大 `max_uses`
- **PTY 访问等同于 shell 访问** —— 请谨慎管理谁拿到 operator/owner 角色
