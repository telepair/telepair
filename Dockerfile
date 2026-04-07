# Stage 1: Build backend
FROM rust:1.85-bookworm AS backend
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
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=backend /build/target/release/telepair /usr/local/bin/
COPY --from=frontend /build/web/dist /opt/telepair/web/dist

EXPOSE 7700
VOLUME /root/.telepair

CMD ["telepair", "--web-dir", "/opt/telepair/web/dist"]
