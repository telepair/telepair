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
cargo test --workspace                       # 49 Rust tests across all crates
cargo test -p telepair-core                  # single crate
cargo test -p telepair-core storage          # single test by name substring

cd web && npm test                           # 22 Vitest tests
cd web && npm run test:watch                 # watch mode
cd web && npm run type-check                 # TypeScript type checking
```

## Linting

```bash
cargo clippy --workspace                     # Rust linting
```

All crates use `#![deny(unsafe_code)]`.

## Architecture

Telepair is a web-based terminal collaboration tool — "Google Docs for your terminal." It's a Cargo workspace with 5 crates following a composable role architecture:

```
telepair-cli          Entry point. Parses --agent/--control/--gateway flags.
                      No flags = all roles (single-node default).
    │
    ├── telepair-gateway    Axum HTTP/WS server. REST API + static file serving.
    │                       SessionHub manages live sessions (PTY channels + participants).
    │
    ├── telepair-control    Session lifecycle, auth service (bcrypt tokens), target registry.
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
5. Terminal I/O uses binary WebSocket frames (type byte prefix: 0x01=output, 0x02=input, 0x03=resize)
6. Collaboration messages (join/leave/chat/cursor) use JSON WebSocket frames with `#[serde(tag = "type")]` discriminant

### Storage

SQLite via sqlx (async). Database at `~/.telepair/telepair.db`. Migrations run automatically on startup from `/migrations/`. The `Storage` trait in `telepair-core/src/storage.rs` abstracts all DB access.

### Frontend

SolidJS + xterm.js. Reactive stores in `web/src/stores/` manage auth and session state. The WebSocket client (`web/src/lib/ws.ts`) and REST client (`web/src/lib/api.ts`) implement the protocol types from `web/src/lib/protocol.ts`.

## Protocol

Client/server messages are defined in `telepair-core/src/protocol.rs` (Rust) and mirrored in `web/src/lib/protocol.ts` (TypeScript). These must stay in sync. JSON messages use `#[serde(tag = "type")]` — the `type` field is the discriminant.

## Permissions

Role capabilities are checked in `telepair-core/src/permission.rs` via methods like `can_input()`, `can_resize()`, `can_manage_participants()`. The WebSocket handler in `telepair-gateway/src/ws.rs` enforces these before processing messages.

## Environment

```bash
RUST_LOG=debug ./target/release/telepair     # adjust log level (default: info)
```

Data directory: `~/.telepair/` (db + optional `targets.yaml` for virtual targets).
