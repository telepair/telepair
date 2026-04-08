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

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=backend /build/target/release/telepair /usr/local/bin/
COPY --from=frontend /build/web/dist /opt/telepair/web/dist

EXPOSE 7700
VOLUME /root/.telepair

CMD ["telepair", "--web-dir", "/opt/telepair/web/dist"]
