# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
# Backend
cargo build --release
cargo run                                    # dev mode, port 7700

# Frontend
cd web && npm install && npm run build       # production build
cd web && npm run dev                        # dev server on :5173, proxies /api and /ws to :7700

# Full production run
./target/release/telepair --web-dir web/dist
```

## Testing

```bash
cargo test --workspace                       # 372 Rust tests across all crates
cargo test -p telepair-core                  # single crate
cargo test -p telepair-core storage          # single test by name substring

cd web && npm test                           # 159 Vitest unit tests
cd web && npm run test:watch                 # watch mode
cd web && npm run type-check                 # TypeScript type checking

cd web && npm run e2e                        # 44 Playwright browser E2E tests
cd web && npm run e2e:ui                     # Playwright UI mode (interactive)
```

E2E tests require a built frontend (`npm run build`) and Chromium (`npx playwright install chromium`). The server auto-starts via `cargo run` or reuses an existing one on port 7700. Admin token is read from `~/.telepair/admin_token`.

## Linting

```bash
cargo clippy --workspace                     # Rust linting
```

All crates use `#![deny(unsafe_code)]`.

## Pre-push Gate

**`make all` must pass before any `git push`.** No exceptions. This is the single
source of truth for "ready to push": it runs `fmt-check`, `lint` (clippy + tsc),
`test` (cargo + vitest), `build` (release binary + web bundle), and `e2e`
(Playwright) in one shot.

```bash
make all                                     # required before every push
```

If `make all` fails, fix the issue at its root — do not bypass with `--no-verify`,
`fmt`-only partial runs, or skipping a subcommand.

`make all` is necessary but not sufficient to release: it runs against the
local toolchain (macOS + whatever Node is on PATH). Use `web/.nvmrc` so
`nvm use` (or `fnm use`) inside `web/` matches the Node version CI runs
(currently 22). Cross-platform invariants (PTY, signals, fs semantics)
still need to be verified by CI itself.

## Release Flow

Releases are tag-driven. **Never tag or publish a GitHub Release before
CI on `main` is green** for the release commit — the published bits stay
in lockstep with a verified build.

```text
1. push the release commit to main
2. wait for the CI workflow on that commit to go green
3. git tag -s vX.Y.Z + git push origin vX.Y.Z   (Release workflow uploads artifacts)
4. gh release create vX.Y.Z                      (only after the Release workflow is green)
```

If CI fails after a tag/release went out, ship a hotfix to `main`
immediately rather than retagging — the tag is immutable history.

## Architecture

Telepair is a web-based terminal collaboration tool — "Google Docs for your terminal." It's a Cargo workspace with 5 crates following a composable role architecture:

```
telepair-cli          Entry point. Parses --agent/--control/--gateway flags.
                      No flags = all roles (single-node default).
    │
    ├── telepair-gateway    Axum HTTP/WS server. REST API + static file serving.
    │                       SessionHub manages live sessions (PTY channels + participants).
    │
    ├── telepair-control    Session lifecycle, auth service (nanoid tokens, SHA-256 at rest), target registry.
    │
    ├── telepair-agent      PTY spawning (portable-pty), virtual target engine (targets.yaml).
    │
    └── telepair-core       Shared types, Storage trait + SQLite impl, protocol enums,
                            permission model (Owner/Operator/Viewer), error types.
```

### Key data flow

1. User authenticates with bearer token via REST API
2. POST `/api/sessions` creates a session + DB record
3. WebSocket connects at `/ws/session/{id}`, first message must be `SessionJoin`
4. `SessionHub` spawns a `LiveSession` with PTY, broadcast channels (output + collab), and participant map
5. Terminal I/O uses binary WebSocket frames (raw bytes, no framing header; resize uses JSON text frames)
6. Collaboration messages (join/leave/chat/cursor) use JSON WebSocket frames with `#[serde(tag = "type")]` discriminant

### Storage

SQLite via sqlx (async). Database at `~/.telepair/telepair.db`. Migrations run automatically on startup from `/migrations/`. The `Storage` trait in `telepair-core/src/storage.rs` abstracts all DB access.

### Frontend

SolidJS + xterm.js. Reactive stores in `web/src/stores/` manage auth and session state. The WebSocket client (`web/src/lib/ws.ts`) and REST client (`web/src/lib/api.ts`) implement the protocol types from `web/src/lib/protocol.ts`.

## Protocol

Client/server messages are defined in `telepair-core/src/protocol.rs` (Rust) and mirrored in `web/src/lib/protocol.ts` (TypeScript). These must stay in sync. JSON messages use `#[serde(tag = "type")]` — the `type` field is the discriminant.

## Permissions

Role capabilities are checked in `telepair-core/src/permission.rs` via `can_input()` and `can_resize()`. The WebSocket handler in `telepair-gateway/src/ws.rs` enforces these on every input/resize message; admin-only target gating happens in the REST `create_session` handler against `Target::admin_only`.

## Environment

```bash
RUST_LOG=debug ./target/release/telepair     # adjust log level (default: info)
```

Data directory: `~/.telepair/` (db + optional `targets.yaml` for virtual targets).
