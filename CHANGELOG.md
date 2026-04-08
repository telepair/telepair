# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- Token-based authentication with SHA-256 hashed storage — raw tokens are
  returned exactly once at creation and never persisted in plaintext.
- PTY manager built on `portable-pty` (spawn, I/O, resize).
- Virtual target engine driven by `~/.telepair/targets.yaml`.
- Session and target services in `telepair-control`.
- Axum HTTP gateway exposing REST routes for health, targets, sessions, and
  invite tokens; static file serving for the web frontend.
- **Anonymous invite redemption** on `POST /api/invite/redeem`: opening a
  shared invite link no longer requires the visitor to already hold a
  bearer token. The endpoint mints a throwaway `guest-<nanoid>` user on
  successful redemption and returns the freshly issued token in the response
  body. Authenticated callers reuse their existing identity and get
  `"token": null` in the response. Guests are only created **after** the
  invite is successfully consumed, so a rejected link never leaves an
  orphan user behind.
- Strict `input_mode` parsing on `POST /api/sessions`: unknown values return
  `400 Bad Request` rather than silently collapsing to `serialized`, so
  client typos surface instead of masking themselves as the wrong semantics.
- Structured error-to-HTTP mapping via `Error::http_status()`: authorization
  failures always surface as 401/403 and input errors as 400 regardless of
  which storage call produced them — no more `InvalidInput` leaking out as
  500. `POST /api/invite/redeem` returns 400 for unknown/expired/exhausted
  tokens and 410 Gone when the target session is closed (the invite is not
  consumed so it can still be revoked or reassigned).
- WebSocket handler with a `SessionHub` that multiplexes PTY I/O and
  collaboration traffic across participants.
- Reconnect-safe participant lifecycle: the WebSocket handler does not
  stamp `left_at` on socket close — participant cleanup happens
  exclusively inside `close_session` / `close_stale_sessions`, in the
  same transaction that flips session status, so a transient socket blip
  inside the reaper's grace window does not evict an invitee.
- Multi-user participant tracking with join/leave/chat/cursor messages.
- Binary WebSocket frames for terminal I/O (zero-copy via `Bytes`); JSON
  frames for control and collaboration messages with `#[serde(tag = "type")]`
  discriminants.
- Bounded WebSocket chat size, cursor update rate, and frame limits.
- Single-node `telepair` binary running agent + control + gateway in one
  process; hidden role flags are reserved for future clustering work.
- `telepair admin show-token` subcommand for recovering the admin token
  from `~/.telepair/admin_token` (mode `0600`, written on first startup).

### Fixed

- **SPA deep-link status code** — `build_router_with_options` used
  `ServeDir::not_found_service(ServeFile(index.html))` which correctly
  served the shell body for client-side routes like `/login`,
  `/join/<token>`, and `/session/<id>` but returned `HTTP 404`
  (tower-http's `not_found_service` wraps the fallback in
  `SetStatus::new(..., NOT_FOUND)`, which forcibly rewrites the status).
  The 404 broke nginx `proxy_intercept_errors`, CDN rules that treat
  404s as cacheable dead links, uptime probes, and OG/SEO crawlers.
  Replace the `ServeFile` fallback with a `service_fn` that reads
  `index.html` once at boot into an `Arc<Vec<u8>>` and hand it to
  `ServeDir::fallback(..)` (not `not_found_service`) so the reply is
  `200 OK` with `Cache-Control: no-cache`. Failing to read `index.html`
  at startup now hard-errors instead of silently serving an empty
  body.
- WebSocket connect is now deferred until xterm completes its initial
  `fit()`, so the PTY is spawned with the real terminal cols/rows
  instead of the default 80×24 (so `vim`, `htop`, etc. open at the
  right size).
- The gateway closes the session row and aborts the output forwarder
  when WebSocket launch fails midway, preventing orphan session rows
  that would block subsequent joins.
- Transient `STORAGE_ERROR` codes are now propagated as **retryable**
  across the gateway → WS → frontend stack, instead of forcing a logout
  the first time SQLite returns `SQLITE_BUSY` under load.
- Unknown `/api/*` and `/ws/*` paths now return `404 Not Found` instead
  of falling through to the SPA shell, so misnamed API calls fail loudly
  instead of receiving HTML.
- `POST /api/sessions/{id}/invite` returns `410 Gone` when the target
  session has already been closed, matching the redeem path so the
  client can show a consistent "session ended" message.
- Scoped invite guests now get bounced off the dashboard cleanly
  (redirected back to their session) instead of being left staring at
  an empty target list.
- The CLI no longer prints a stray warning when the optional
  `~/.telepair/targets.yaml` is absent — a missing file is the default,
  not an error.

### Added — Web frontend (SolidJS + Vite)

- Vite + SolidJS + TypeScript scaffold.
- Typed REST API client and WebSocket client mirroring the backend protocol.
- Reactive auth and session stores.
- Login page, dashboard with available targets, and session page with
  xterm.js terminal (WebGL rendering).
- Anonymous invite flow: the Join page always attempts to redeem the token
  in the URL — no "paste your admin token first" wall — and only surfaces
  an error when the invite itself is invalid, expired, or points at a
  closed session (410 Gone). The newly minted guest token is cached in
  the browser for the rest of the tab's lifetime.
- Login copy mirrors the anonymous flow: invitees are told to just open
  the link, admins paste their token.
- REST client 401 interceptor: any protected path returning 401 clears the
  cached token and forces a redirect to `/login`, so a stale admin token
  can never leave the dashboard silently rendering "No targets available".
  `POST /api/invite/redeem` is explicitly exempt so a half-filled guest
  flow does not bounce the visitor out of the invite.
- Real-time collaboration UI: participants list and chat panel wired into
  the session page.
- UI primitives (toast, banner, skeleton) and a banner-based reconnect UX.
- **Bilingual English / Simplified Chinese UI** with a live locale switcher
  in the topbar; the choice is persisted in `localStorage` and the bundle
  ships both dictionaries plus a small auto-detect helper.
- **JetBrainsMono Nerd Font bundled with the frontend** so powerline,
  dev-icon, and box-drawing glyphs render correctly in xterm.js without
  the user having to install a font on the host machine.
- Owner-only **Close session** control on the session top bar with a
  two-step confirm — clicking once arms the button, clicking again issues
  `DELETE /api/sessions/{id}`.
- **System notices in the chat panel** when participants join or leave the
  session, rendered as italic centered lines so they don't get mistaken
  for chat messages.
- Invite-link **Copy** button falls back to `document.execCommand('copy')`
  in non-secure contexts (HTTP / raw-IP origins) so the dashboard still
  works without HTTPS.

### Added — Documentation

- Project README, contributing guide, and MIT license.
- `docs/architecture.md`, `docs/protocol.md`, `docs/api.md`, and
  `docs/deployment.md` — the deployment guide covers systemd, nginx,
  Docker, Docker Compose, and a dedicated CORS section covering same-origin
  reverse-proxy, direct-exposure with `--allowed-origins`, and dev-only
  `--allow-any-origin`. The systemd example binds `--host 127.0.0.1` so
  the listener is loopback-only behind a proxy by default.
- README documents the full `telepair --help` surface including
  `--allowed-origins` / `--allow-any-origin`, the `admin show-token`
  recovery path, and calls out that the binary runs as a single-node
  process today.

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
- **Scoped invite guests** — guest users minted via `POST /api/invite/redeem`
  are now pinned to the redeemed `session_id` via a new
  `users.scoped_session_id` column. A dedicated `require_unscoped` check
  blocks scoped guests from `GET /api/targets` and `POST /api/sessions`
  with 403, `POST /api/invite/redeem` rejects cross-session redemption
  attempts, and the WebSocket handshake enforces the pin a second time so
  a guest cannot open `/ws/session/{other}` even if a future bug adds a
  stray participant row. Closes an invite-time privilege-escalation path
  where any redeemed invite produced a fully privileged non-admin account
  that could enumerate targets and create new sessions of its own.
- `GET /api/targets` now filters out `admin_only` targets for non-admin
  callers. Previously the REST list leaked admin-only target names to
  every authenticated user even though `POST /api/sessions` correctly
  refused to create a session on them, turning the list endpoint into a
  free enumeration oracle for reconnaissance.

### Performance

- Zero-copy PTY output via `Bytes` broadcast.
- Parallelized auth + session lookup in the WebSocket handshake.
- Terminal output sent as binary WebSocket frames.
- Idle session reaper with reconnection-aware refcounting so multi-tab users
  stay present and orphan PTYs do not leak.
- Indexed `participants(user_id)` with cascading foreign-key deletes.
- Atomic session create / join / close paths.
- SPA shell `index.html` is loaded once at boot into a refcounted
  `Bytes` and shared across requests, removing per-request file I/O on
  the deep-link fallback.

### Testing

- **107** Rust tests across the workspace (unit, integration, and backend
  collaboration end-to-end flows), including coverage for `create_guest`
  uniqueness, the anonymous-redeem happy path, invitee reconnection inside
  the reaper grace window, bulk `close_session` participant settling, the
  strict `input_mode` rejection path, and the 400 / 410 error codes on
  `POST /api/invite/redeem`.
- **112** Vitest unit tests covering the protocol, stores, REST client
  (including the 401 interceptor and its `/invite/redeem` opt-out), the
  WebSocket client, and the i18n dictionaries (locale auto-detect,
  English ↔ Chinese key symmetry, template rendering, and label coverage).
- **19** Playwright browser E2E tests covering authentication, dashboard,
  session lifecycle, terminal I/O, collaboration, the full anonymous-guest
  redeem flow, and the invalid-invite error path.

### Build & CI

- Makefile wiring `fmt` / `fmt-check` / `lint` / `test` / `build` / `e2e`
  targets plus an `all` target that chains the full pre-push pipeline
  (fmt-check + clippy + type-check + unit tests + release binary +
  frontend bundle + Playwright E2E) in one shot. `.DEFAULT_GOAL` stays
  on `help` so the bare `make` experience remains a menu.
- GitHub Actions CI pipeline and release workflow.
- MIT license across the workspace.

[Unreleased]: https://github.com/telepair/telepair/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/telepair/telepair/releases/tag/v0.1.0
