# Stage 1: Build backend
FROM rust:1.94-bookworm AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
RUN cargo build --release

# Stage 2: Build frontend
FROM node:22-bookworm-slim AS frontend
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ .
RUN npm run build

# Stage 3: Runtime
FROM debian:bookworm-slim

# OCI labels — GHCR auto-links the published package to this repo when
# `org.opencontainers.image.source` points at a GitHub repo URL.
LABEL org.opencontainers.image.source="https://github.com/telepair/telepair"
LABEL org.opencontainers.image.description="Self-hosted, browser-based collaborative terminal with live PTY sharing, RBAC, and invite links. Rust + SolidJS."
LABEL org.opencontainers.image.licenses="MIT"

# Runtime deps: ca-certificates for outbound SMTP (email/OTP),
# curl for the HEALTHCHECK probe below.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Non-root runtime user. Fixed uid/gid (10001) so bind-mounted host
# volumes have predictable ownership across image versions.
RUN groupadd --system --gid 10001 telepair \
    && useradd --system --uid 10001 --gid telepair \
       --home-dir /home/telepair --shell /usr/sbin/nologin \
       --create-home telepair

COPY --from=backend /build/target/release/telepair /usr/local/bin/
COPY --from=frontend /build/web/dist /opt/telepair/web/dist

USER telepair
WORKDIR /home/telepair

EXPOSE 7700

# Data directory lives under the runtime user's home. The binary
# creates it on first boot if missing.
VOLUME /home/telepair/.telepair

# HEALTHCHECK hits the gateway's own liveness route. `start-period`
# covers the boot path (migrations + admin-token generation) before
# failures count toward `retries`.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD curl -fsS http://127.0.0.1:7700/api/health || exit 1

CMD ["telepair", "--web-dir", "/opt/telepair/web/dist"]
