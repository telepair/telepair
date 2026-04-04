# Deployment Guide

## Single-Machine Deployment

The simplest deployment: one binary, one machine.

### Build

```bash
# Build release binary
cargo build --release

# Build frontend
cd web && npm install && npm run build && cd ..
```

### Run

```bash
./target/release/telepair --web-dir web/dist
```

This starts all roles (agent + control + gateway) on port 7700. On first run, an admin token is printed to the console — save it.

### Systemd Service

```ini
# /etc/systemd/system/telepair.service
[Unit]
Description=telepair terminal collaboration
After=network.target

[Service]
Type=simple
User=telepair
ExecStart=/usr/local/bin/telepair --web-dir /opt/telepair/web/dist
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

### Reverse Proxy (nginx)

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

    # WebSocket upgrade
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

Key points:
- WebSocket requires `Upgrade` and `Connection` headers
- `proxy_read_timeout 86400` prevents nginx from closing long-lived WS connections

## Docker

### Dockerfile

```dockerfile
# Build backend
FROM rust:1.85 AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
RUN cargo build --release

# Build frontend
FROM node:18 AS frontend
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

### Run

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

## Configuration

### Virtual Targets

Mount a custom targets config:

```bash
# Docker
docker run -v ./targets.yaml:/root/.telepair/targets.yaml telepair

# Systemd
./telepair --targets /etc/telepair/targets.yaml --web-dir web/dist
```

See the [README](../README.md#virtual-targets) for targets.yaml format.

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level (`debug`, `info`, `warn`, `error`) |

### Data Directory

telepair stores all persistent data in `~/.telepair/`:

| File | Purpose |
|------|---------|
| `telepair.db` | SQLite database (users, sessions, participants, invites) |
| `targets.yaml` | Virtual target definitions (optional) |

Back up `telepair.db` to preserve user accounts and session history.

## Security Considerations

- **Always use TLS** in production (via reverse proxy)
- **Save the admin token** from first run — it cannot be recovered
- **Restrict network access** — telepair binds to `0.0.0.0` by default; use `--host 127.0.0.1` if only accessed via reverse proxy
- **Invite tokens** are single-use by default; increase `max_uses` only when needed
- **PTY access** is equivalent to shell access — carefully manage who gets operator/owner roles
