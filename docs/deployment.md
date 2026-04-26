English | [简体中文](deployment.zh-CN.md)

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
FROM rust:1.94 AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
RUN cargo build --release

# Build frontend
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

### CORS

telepair serves a same-origin frontend in production, so when the browser fetches `/api` and opens `/ws` from the same host that served `index.html`, the browser-origin checks pass without extra config. HTTP uses CORS response headers; WebSocket upgrades validate the `Origin` header before accepting the handshake. You only need to think about allowed origins when the frontend lives on a different origin than the API:

- **Reverse-proxy deployment (recommended).** nginx terminates TLS, serves `/` and proxies `/api` + `/ws/` to `127.0.0.1:7700`. Same-origin from the browser's point of view — no CORS flags needed.
- **Direct exposure without a proxy.** If you host the frontend on a different domain (e.g. serving `web/dist` from a CDN while the API runs elsewhere), you must pass the exact frontend origin:

  ```bash
  ./telepair --web-dir web/dist \
             --allowed-origins https://telepair.example.com
  ```

  Comma-separate to allow multiple. Malformed origins abort startup — a typo can never silently degrade to an empty list.
- **Default (no flag).** Falls back to `http://localhost:5173, http://127.0.0.1:5173` for the Vite dev server and always allows same-host browser WebSocket upgrades. This is intentionally **not** "allow any" — earlier versions defaulted to `*` and that was a footgun.
- **`--allow-any-origin`.** Only safe in dev or when a reverse proxy enforces CORS upstream. Overrides `--allowed-origins`.

### Environment Variables

Every env var below has a matching CLI flag (`--data-dir`, `--smtp-host`, etc.); flags win when both are set.

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level (`debug`, `info`, `warn`, `error`) |
| `TELEPAIR_DATA_DIR` | `~/.telepair` | Override the data directory (DB, admin token, targets.yaml, recordings). |
| `TELEPAIR_TRUST_FORWARDED_HEADERS` | `false` | Trust `X-Forwarded-For` / `X-Real-IP` when keying the per-IP register rate limiter. Enable **only** behind a reverse proxy that rewrites those headers on every request — with this on in a direct-exposure setup, any client can forge the header and bypass throttling. |
| `TELEPAIR_SMTP_HOST` | *(unset)* | SMTP server hostname. Required to enable email registration; unset disables the OTP path. |
| `TELEPAIR_SMTP_PORT` | `587` | SMTP port (STARTTLS). |
| `TELEPAIR_SMTP_USER` | *(unset)* | SMTP username. |
| `TELEPAIR_SMTP_PASS` | *(unset)* | SMTP password. |
| `TELEPAIR_SMTP_FROM` | *(unset)* | SMTP sender address, e.g. `"Telepair <noreply@example.com>"`. |
| `TELEPAIR_RECORDING_ENABLED` | `false` | Master switch for session recording. When false, no session is ever recorded. |
| `TELEPAIR_RECORDING_TTL_DAYS` | `30` | Retention in days. `0` means permanent (no TTL sweep). |
| `TELEPAIR_RECORDING_DIR` | `<data-dir>/recordings` | Directory for `.cast` files. |

### Data Directory

telepair stores all persistent data in `~/.telepair/` (override with `--data-dir` / `TELEPAIR_DATA_DIR`):

| Path | Purpose |
|------|---------|
| `telepair.db` | SQLite database (users, sessions, participants, invites, audit events, recordings, recording shares) |
| `admin_token` | Admin bearer token (created on first run, mode 0600) |
| `targets.yaml` | Virtual target definitions (optional) |
| `recordings/` | `.cast` files for session recordings, one per recording id; created on first recording. Override with `--recording-dir` / `TELEPAIR_RECORDING_DIR`. |

Back up `telepair.db` (and `recordings/` when recording is enabled) to preserve user accounts, session history, and playback.

## Session recording

Session recording is **off by default** and must be explicitly opted in. Enable with `--recording-enabled` (or `TELEPAIR_RECORDING_ENABLED=true`):

```bash
./telepair --web-dir web/dist \
           --recording-enabled \
           --recording-ttl-days 30
```

What you get when it's on:

- Session owners can **start / stop** recordings from the in-session Recording panel (`POST /api/sessions/{id}/recording/{start,stop}`). Only one recording can be active on a session at a time.
- Owners and admins can **list / play / delete** their own recordings; admins can list everyone's via `GET /api/admin/recordings`.
- Owners can mint **signed share links** (TTL + max-uses) via `POST /api/recordings/{id}/shares`. Anonymous viewers hitting `/recordings/{id}/play#token=...` bypass `AuthGuard`; the player reads the URL fragment, scrubs it from history via `replaceState`, then fetches `/api/recordings/{id}/data` with an `X-Share-Token: <raw>` header. Share links default to one use; pass `max_uses: 0` only when deliberately creating an unlimited link. The data endpoint checks the token before reading the `.cast` file, then atomically consumes a use only after the file read succeeds, so a missing file cannot burn a limited-use link. Keeping the secret out of the query string matters for log hygiene — NGINX `$request` / ALB `request_url` / CloudFront standard logs all capture query strings by default, but none of them log `X-Share-Token` unless explicitly told to.
- A background **cleaner** scans `expires_at` every few minutes and deletes expired rows. Active recordings (`status = 'recording'`) are always excluded from the candidate set as defence-in-depth. `expires_at IS NULL` means "keep forever".

Storage notes:

- Recordings live as asciicast v2 `.cast` files in `--recording-dir` (default `<data-dir>/recordings/`), named by recording id.
- Metadata (`file_size`, `duration_ms`, `event_count`, `status`, `expires_at`) goes into the `recordings` table; share tokens into `recording_shares`. Both are on `ON DELETE CASCADE` from their parent rows.
- A recording whose writer dropped events under back-pressure is finalized as `status = 'failed'` (not `completed`) so "completed" always means "gapless asciicast".
- Enabling recording costs disk — rough order of magnitude is a few KB/s per active session of PTY output plus chat/participant events. Size the volume or lower `TELEPAIR_RECORDING_TTL_DAYS` accordingly.

When recording is **disabled**, the Recording panel is hidden in the UI, `POST /api/sessions/{id}/recording/start` returns `403 Forbidden` ("session recording is disabled on this server"), and no new writer task is spawned. Read endpoints (`GET /api/recordings`, `GET /api/recordings/{id}/data`, shares, etc.) keep working so previously-created recordings remain playable after an operator flips the switch off.

## Security Considerations

- **Always use TLS** in production (via reverse proxy)
- **Save the admin token** from first run — it is also written to `~/.telepair/admin_token` (mode 0600) for recovery. Lost it? Run `telepair admin show-token` to print the cached value.
- **Restrict network access** — telepair binds to `0.0.0.0` by default; use `--host 127.0.0.1` when behind a reverse proxy so the port is never reachable from the outside
- **Pin CORS** — never deploy with `--allow-any-origin` on a direct-exposed host. Either run behind a proxy (same-origin, no CORS) or explicitly list the frontend origin with `--allowed-origins`.
- **Invite tokens** are single-use by default; increase `max_uses` only when needed
- **PTY access** is equivalent to shell access — carefully manage who gets operator/owner roles
