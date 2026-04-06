# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-04-07

Initial public release of **telepair** — a web-based terminal collaboration
tool ("Google Docs for your terminal") that lets multiple users share one live
PTY through the browser, with role-based permissions, invite links, and
real-time chat.

### Added — Backend (Rust workspace)

- Cargo workspace with 5 crates (`telepair-core`, `telepair-agent`,
  `telepair-control`, `telepair-gateway`, `telepair-cli`) built on Rust
  Edition 2024, toolchain 1.85+, and `#![deny(unsafe_code)]` everywhere.
- Shared types for sessions, targets, config, protocol, errors, and the
  Owner / Operator / Viewer permission model.
- `Storage` trait with an async SQLite implementation (sqlx) and auto-applied
  migrations from `/migrations/`.
- Token-based authentication provider.
- PTY manager built on `portable-pty` (spawn, I/O, resize).
- Virtual target engine driven by `~/.telepair/targets.yaml`.
- Session and target services in `telepair-control`.
- Axum HTTP gateway exposing REST routes for health, targets, sessions, and
  invite tokens; static file serving for the web frontend.
- WebSocket handler with a `SessionHub` that multiplexes PTY I/O and
  collaboration traffic across participants.
- Multi-user participant tracking with join/leave/chat/cursor messages.
- Binary WebSocket frames for terminal I/O (zero-copy via `Bytes`); JSON
  frames for control and collaboration messages with `#[serde(tag = "type")]`
  discriminants.
- Bounded WebSocket chat size, cursor update rate, and frame limits.
- `telepair` CLI with composable `--agent`, `--control`, `--gateway` role
  flags (no flags = all roles, single-node default).
- `telepair admin show-token` subcommand to recover the admin token.
- Admin token persisted to `~/.telepair/admin_token` with mode `0600`.

### Added — Web frontend (SolidJS + Vite)

- Vite + SolidJS + TypeScript scaffold.
- Typed REST API client and WebSocket client mirroring the backend protocol.
- Reactive auth and session stores.
- Login page, dashboard with available targets, and session page with
  xterm.js terminal (WebGL rendering).
- Invite flow: invite dialog, join page, and REST/WS integration.
- Real-time collaboration UI: participants list and chat panel wired into
  the session page.
- UI primitives (toast, banner, skeleton) and a banner-based reconnect UX.

### Added — Documentation

- Project README, contributing guide, and MIT license.
- `docs/architecture.md`, `docs/protocol.md`, `docs/api.md`,
  `docs/deployment.md` (with systemd, nginx, Docker, and Docker Compose
  recipes).

### Security

- TOCTOU-safe invite token redemption.
- Force-disconnect WebSocket clients when a session is stopped.
- Configurable CORS origins with loopback defaults and wildcard warning;
  malformed origins are rejected.
- PTY child-process environment sanitization.
- `list_sessions` scoped to owner and participant sessions only.
- Idempotent invite redemption via `upsert_participant`.
- Admin-only target gating enforced in the REST `create_session` handler.
- Per-message permission enforcement on WebSocket input and resize.
- Frontend persists the auth token only after successful validation.
- WebSocket auth timeout and graceful shutdown.

### Performance

- Zero-copy PTY output via `Bytes` broadcast.
- Parallelized auth + session lookup in the WebSocket handshake.
- Terminal output sent as binary WebSocket frames.
- Idle session reaper with reconnection-aware refcounting so multi-tab users
  stay present and orphan PTYs do not leak.
- Indexed `participants(user_id)` with cascading foreign-key deletes.
- Atomic session create / join / close paths.

### Testing

- **77** Rust tests across the workspace (unit, integration, and backend
  collaboration end-to-end flows).
- **63** Vitest unit tests covering the protocol, stores, REST client, and
  WebSocket client.
- **16** Playwright browser E2E tests covering authentication, dashboard,
  session lifecycle, terminal I/O, and collaboration.

### Build & CI

- Makefile wiring `cargo fmt --check`, `cargo clippy`, and test targets.
- GitHub Actions CI pipeline and release workflow.
- MIT license across the workspace.

[0.1.0]: https://github.com/telepair/telepair/releases/tag/v0.1.0
