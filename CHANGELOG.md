# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Invite links now work for brand-new collaborators.** `POST /api/invite/redeem`
  previously required a valid bearer token, which meant the "share a link to
  collaborate" flow was broken for anyone who had not already been handed an
  admin token. The endpoint now accepts anonymous callers, mints a guest user
  (`guest-<nanoid>`) on redemption, and returns the freshly issued bearer token
  in the response body. Authenticated callers still reuse their existing
  identity and get `token: null`. Guests are only created **after** the invite
  is successfully consumed, so a rejected link never leaves an orphan user
  behind.
- **Invitees can reconnect after a transient disconnect.** The WebSocket
  handler used to eagerly stamp `left_at` on every socket close, while the
  session reaper kept the in-memory `LiveSession` alive for a 120 s grace
  window. Clients auto-retrying inside that window hit the `NOT_PARTICIPANT`
  branch and were closed with 4001, meaning an invitee could only ever join
  once per socket lifetime. Participant cleanup is now done by `close_session`
  and `close_stale_sessions` inside a single transaction that also flips the
  session status, so a socket blip no longer marks the participant row gone.
  The owner path was masked from the old bug by a `is_owner` short-circuit,
  which is why the original reaper test used an owner and missed it — a new
  regression test exercises the invitee path explicitly.
- **Stale admin tokens no longer leave the dashboard in a broken state.** The
  REST client now intercepts 401 on any protected path, clears the cached
  token, and forces a navigation to `/login`. Previously the dashboard would
  silently render "No targets available" while hiding the real auth failure.
  The redeem endpoint is explicitly exempt so that a half-filled guest flow
  does not bounce the visitor out of the invite.
- **`input_mode` typos return 400 instead of silently degrading.** Unknown
  values used to fall back to `serialized`, which masked client bugs and could
  surprise the caller with the wrong input semantics. The `POST /api/sessions`
  handler now parses `input_mode` strictly.
- **Handler error mappings no longer leak `InvalidInput` as 500.** Introduced
  `Error::http_status()` and a handler-side `status_for()` helper so that
  authorization failures always come back as 401 / 403 and input errors as
  400, regardless of which storage call produced them.

### Changed

- `POST /api/invite/redeem` response schema gained a nullable `token` field.
  It is populated only when a new guest was minted; authenticated callers
  keep using their original token and get `"token": null`.
- Join page (`web/src/pages/Join.tsx`) no longer shows a "please paste a
  token first" wall — it always attempts to redeem and only surfaces an
  error when the invite itself is invalid, expired, or points at a closed
  session (410 Gone).
- Login help copy updated to reflect the new "open the link, no token
  needed" flow for guests.

### Removed

- Dead `TokenAuthProvider::create_user` method (no callers).
- Dead `Storage::remove_participant` method and its only caller in the
  WebSocket handler. Participant `left_at` writes now happen exclusively
  inside `close_session` / `close_stale_sessions`.

### Testing

- Rust workspace up to **87** tests (from 77), with new coverage for
  `create_guest` uniqueness, the anonymous-redeem happy path, invitee
  reconnect within the reaper grace window, bulk `close_session` participant
  settling, strict `input_mode` rejection, and the 400/410 error codes on
  `POST /api/invite/redeem`.
- Web Vitest suite up to **67** tests (from 63), covering the new 401
  interceptor — including the `/invite/redeem` opt-out so anonymous guests
  are not yanked back to `/login`.
- Playwright suite up to **18** tests (from 16), adding a full
  anonymous-guest redeem flow and an invalid-invite error path.

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

[Unreleased]: https://github.com/telepair/telepair/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/telepair/telepair/releases/tag/v0.1.0
