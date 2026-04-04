# Telepair v1 -- Design Specification

## Context

Existing web terminal tools each solve a piece of the puzzle but none deliver the full picture: **sshx** offers collaboration but lacks permissions, recording, and chat; **termpair** has encryption but no multi-user collaboration; **WebTTY** provides basic P2P but nothing production-grade. Telepair aims to be the "Google Docs for Terminal" -- a single open-source tool that combines real-time multi-user collaboration, fine-grained permissions, and a target abstraction layer (local shells, virtual targets, and eventually Kubernetes pods).

**v1 Goal:** Ship a working single-node product with the core collaboration experience -- web terminal + agent/control/gateway architecture + real-time multi-user sessions + basic permissions + virtual targets. Defer E2EE, recording/playback, AI enhancements, P2P-only mode, and cluster mode to v2+.

## 1. Architecture Overview

### 1.1 Single Binary, Composable Roles

Telepair ships as **one binary** (`telepair`) where each instance can run **any combination of roles** via flags:

```bash
telepair                              # default: all roles (agent + control + gateway)
telepair --agent --gateway            # agent + gateway on one instance
telepair --control                    # control only
telepair --agent                      # agent only
telepair --agent --control --gateway  # explicit all (same as default)
```

No flag = all roles enabled (single-node mode). This composable approach allows flexible deployment topologies:

- **Single node**: one instance, all roles, zero network overhead (in-process channels)
- **Two-tier**: one instance runs control + gateway, another runs agent(s)
- **Full split**: each role on a separate instance, communicating over the network
- **Edge agent**: agents deployed close to targets, control + gateway centralized

When roles are co-located in one process, they communicate via in-process tokio channels. When separated across instances, they communicate via the same protocol over the network (WebSocket).

### 1.2 Component Responsibilities

| Component | Role |
|-----------|------|
| **Gateway** | Client-facing entry point: serves SPA, HTTP REST API, WebSocket endpoints, WebRTC signaling |
| **Control** | Internal services: authentication, session lifecycle, target registry, SQLite storage |
| **Agent** | PTY management, virtual target engine, transport layer (WS/WebRTC <-> PTY bridge) |

### 1.3 Data Flow

```
Terminal Input:  Browser keystroke -> WS/WebRTC -> Gateway -> Agent -> PTY stdin
Terminal Output: PTY stdout -> Agent -> Gateway -> broadcast to all participants
Collaboration:   Cursor/presence/chat -> Gateway -> broadcast (low-priority channel)
Control Plane:   REST API for auth, session CRUD, target listing, user management
```

## 2. Cargo Workspace Structure

```
telepair/
|-- Cargo.toml                    # workspace root
|-- Cargo.lock
|-- README.md
|-- LICENSE-MIT / LICENSE-APACHE
|
|-- crates/
|   |-- telepair-core/            # shared types, traits, protocols
|   |   +-- src/
|   |       |-- lib.rs
|   |       |-- protocol.rs       # WS/binary message types (serde)
|   |       |-- auth.rs           # Auth trait + TokenAuth impl
|   |       |-- session.rs        # Session, Participant types
|   |       |-- target.rs         # Target, VirtualTarget types
|   |       |-- permission.rs     # Role enum (Owner/Operator/Viewer)
|   |       |-- storage.rs        # Storage trait + SqliteStorage impl
|   |       +-- error.rs          # Error types (thiserror)
|   |
|   |-- telepair-agent/           # PTY management + virtual targets
|   |   +-- src/
|   |       |-- lib.rs
|   |       |-- pty.rs            # PTY spawn/resize/IO (portable-pty)
|   |       |-- virtual_target.rs # config -> command mapping
|   |       +-- transport.rs      # WS/WebRTC <-> PTY bridge
|   |
|   |-- telepair-control/         # business logic services
|   |   +-- src/
|   |       |-- lib.rs
|   |       |-- auth_service.rs   # token validation, user management
|   |       |-- session_service.rs# session lifecycle
|   |       +-- target_service.rs # target registry
|   |
|   |-- telepair-gateway/         # HTTP/WS/WebRTC endpoints
|   |   +-- src/
|   |       |-- lib.rs
|   |       |-- http.rs           # axum REST routes
|   |       |-- ws.rs             # WebSocket handler
|   |       |-- rtc.rs            # WebRTC signaling
|   |       +-- session_hub.rs    # multi-user session broadcast
|   |
|   +-- telepair-cli/             # binary entry point
|       +-- src/
|           +-- main.rs           # clap CLI: --agent --control --gateway flags
|
|-- web/                          # SolidJS frontend
|   |-- package.json
|   |-- tsconfig.json
|   |-- vite.config.ts
|   +-- src/
|       |-- index.tsx
|       |-- App.tsx
|       |-- pages/
|       |   |-- Login.tsx
|       |   |-- Dashboard.tsx
|       |   +-- Session.tsx
|       |-- components/
|       |   |-- Terminal.tsx
|       |   |-- TargetSelector.tsx
|       |   |-- CollabOverlay.tsx
|       |   |-- ParticipantList.tsx
|       |   |-- ChatPanel.tsx
|       |   +-- InviteDialog.tsx
|       |-- stores/
|       |   |-- session.ts
|       |   |-- auth.ts
|       |   +-- connection.ts
|       +-- lib/
|           |-- ws.ts
|           |-- rtc.ts
|           |-- protocol.ts
|           +-- terminal-bridge.ts
|
|-- migrations/
|   +-- 001_initial.sql
|
+-- docs/
```

### 2.1 Crate Dependency Graph

```
              +---------------+
              | telepair-cli  |  (binary)
              +------+--------+
                     | depends on all
        +------------+------------+
        v            v            v
  +----------+ +----------+ +----------+
  | gateway  | | control  | |  agent   |
  +----+-----+ +----+-----+ +----+-----+
       +-------------+------------+
                     v
             +---------------+
             | telepair-core |
             +---------------+
```

## 3. Protocol Design

### 3.1 WebSocket Messages (JSON, control channel)

**Client -> Server:**

| Message | Fields | Description |
|---------|--------|-------------|
| `SessionJoin` | `session_id`, `token` | Join an existing session |
| `TermInput` | `data: Vec<u8>` | Keystrokes (when not using WebRTC) |
| `TermResize` | `cols`, `rows` | Terminal resize |
| `CursorMove` | `x`, `y` | Collaboration cursor position |
| `ChatMessage` | `text` | In-session chat |
| `RtcOffer` | `sdp` | WebRTC signaling |
| `RtcAnswer` | `sdp` | WebRTC signaling |
| `RtcCandidate` | `candidate` | ICE candidate |

**Server -> Client:**

| Message | Fields | Description |
|---------|--------|-------------|
| `SessionState` | `session`, `participants`, `permissions` | Initial session state on join |
| `TermOutput` | `data: Vec<u8>` | PTY output (fallback when WebRTC unavailable) |
| `PeerJoined` | `user_id`, `name`, `role`, `color` | New participant |
| `PeerLeft` | `user_id` | Participant disconnected |
| `PeerCursor` | `user_id`, `x`, `y` | Peer cursor position |
| `PeerChat` | `user_id`, `name`, `text`, `ts` | Chat message |
| `PermUpdate` | `user_id`, `new_role` | Permission change |
| `Error` | `code`, `message` | Error notification |

### 3.2 Binary Channel (WebRTC DataChannel or WS binary frames)

For terminal I/O, a lightweight binary protocol avoids JSON overhead:

```
[1-byte type][2-byte payload length (big-endian)][payload]

Types:
  0x01 = terminal output (server -> client)
  0x02 = terminal input (client -> server)
  0x03 = resize (client -> server): payload = [2B cols][2B rows]
```

### 3.3 Hybrid Transport Strategy

- **WebSocket (always on):** Control messages, chat, presence, signaling. Reliable and ordered.
- **WebRTC DataChannel (optional):** Terminal I/O bytes. Lower latency, P2P capable when possible.
- **Fallback:** If WebRTC negotiation fails (NAT, firewall, browser support), terminal data flows over WebSocket binary frames transparently. The client detects this and uses WS binary without user intervention.

## 4. Collaboration Model

### 4.1 Permission Hierarchy

| Role | Input | Resize | Chat | View | Manage Participants | Close Session |
|------|-------|--------|------|------|---------------------|---------------|
| **Owner** | Y | Y | Y | Y | Y | Y |
| **Operator** | Y | Y | Y | Y | N | N |
| **Viewer** | N | N | Y | Y | N | N |

### 4.2 Input Conflict Resolution

Two modes, configurable per session:

- **Serialized** (default): One active typer at a time. Others must request control. Like screen sharing -- safe for production environments.
- **Multiplexed**: All operators can type simultaneously. Keystrokes interleave at the PTY. True pair programming experience -- best for collaborative debugging.

### 4.3 Collaboration Features (v1)

- **Colored cursors**: Each participant has a unique color, cursor position shown on the terminal overlay
- **Presence indicator**: Connected participants list with roles and online status
- **In-session chat**: Sidebar text chat for communication without affecting the terminal
- **Live permission changes**: Owner can promote/demote/kick participants in real-time

### 4.4 Session Lifecycle

1. Owner creates session -> selects target -> Agent spawns PTY
2. Owner generates invite link (session_id + one-time invite token with role assignment)
3. Others join via link -> token validated -> assigned role
4. All participants receive real-time terminal output + scrollback buffer on join (configurable, default 10,000 lines max)
5. Session ends when: owner closes it, PTY process exits, or idle timeout triggers

### 4.5 Terminal Resize Strategy

When multiple participants have different window sizes, the session uses the **minimum dimensions** across all connected participants (intersection). This ensures all participants can see the full terminal content. When a participant leaves, the terminal may resize up.

## 5. Virtual Target System

### 5.1 Configuration Format (YAML)

```yaml
# ~/.telepair/targets.yaml (also manageable via API/UI)

targets:
  - name: production-db
    display: "Production DB"
    command: psql
    args: ["-h", "db.internal", "-U", "readonly", "production"]
    env:
      PGPASSWORD: "${PROD_DB_PASS}"    # env var substitution
    tags: [database, production]
    required_role: operator

  - name: monitor
    display: "System Monitor"
    command: htop
    tags: [monitoring]
    required_role: viewer

  - name: staging-server
    display: "Staging SSH"
    command: ssh
    args: ["deploy@staging.example.com"]
    tags: [server, staging]
    required_role: operator

  - name: local-shell
    display: "Local Shell"
    type: local                        # built-in: spawns $SHELL
    shell: "${SHELL}"
```

### 5.2 Target Types

- **Local shell** (built-in): Spawns user's default shell via PTY. Always available, no config needed.
- **Virtual target** (configurable): Maps a friendly name to `command + args + env`. Supports `${VAR}` environment variable substitution. Per-target access control via `required_role`.
- **Kubernetes pod** (v2): `kubectl exec` integration with namespace/pod/container selection.

## 6. Storage Layer

### 6.1 SQLite Schema (v1)

```sql
CREATE TABLE users (
    id          TEXT PRIMARY KEY,       -- UUID
    name        TEXT NOT NULL,
    token_hash  TEXT NOT NULL,          -- bcrypt hash
    is_admin    BOOLEAN DEFAULT FALSE,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,       -- short URL-safe ID
    owner_id    TEXT REFERENCES users(id),
    target_name TEXT NOT NULL,
    input_mode  TEXT DEFAULT 'serialized',  -- 'serialized' | 'multiplexed'
    status      TEXT DEFAULT 'active',      -- 'active' | 'closed'
    created_at  TEXT NOT NULL,
    closed_at   TEXT
);

CREATE TABLE participants (
    session_id  TEXT REFERENCES sessions(id),
    user_id     TEXT REFERENCES users(id),
    role        TEXT NOT NULL,          -- 'owner' | 'operator' | 'viewer'
    joined_at   TEXT NOT NULL,
    left_at     TEXT,
    PRIMARY KEY (session_id, user_id)
);

CREATE TABLE invite_tokens (
    token_hash  TEXT PRIMARY KEY,
    session_id  TEXT REFERENCES sessions(id),
    role        TEXT NOT NULL,          -- role assigned on join
    max_uses    INTEGER DEFAULT 1,
    used_count  INTEGER DEFAULT 0,
    expires_at  TEXT
);
```

### 6.2 Storage Trait

```rust
// in telepair-core/src/storage.rs
trait Storage: UserRepo + SessionRepo + InviteRepo + Send + Sync {}

// v1: SqliteStorage implements all sub-traits
// v2: PgStorage for cluster mode
```

## 7. Authentication

### 7.1 v1: Token-Based

- Users have API tokens (generated on first setup or via CLI: `telepair user create <name>`)
- Token sent in `Authorization: Bearer <token>` header for REST API
- Token sent in `SessionJoin` message for WebSocket
- Tokens stored as bcrypt hashes in SQLite
- First run auto-creates an admin user and prints the token

### 7.2 Auth Trait (for future OIDC)

```rust
// in telepair-core/src/auth.rs
trait AuthProvider: Send + Sync {
    async fn validate(&self, credential: &str) -> Result<User>;
    async fn create_user(&self, name: &str) -> Result<(User, String)>; // returns token
}

// v1: TokenAuthProvider (validates against SQLite)
// v2: OidcAuthProvider (GitHub, Google, custom OIDC)
```

## 8. Frontend Architecture

### 8.1 Tech Stack

- **SolidJS** + TypeScript -- reactive UI framework with fine-grained reactivity
- **xterm.js** -- terminal emulator (`@xterm/xterm`, `@xterm/addon-fit`, `@xterm/addon-webgl`)
- **Vite** -- build tool, dev server with proxy to backend
- **@solidjs/router** -- SPA routing

### 8.2 Pages

| Page | Route | Description |
|------|-------|-------------|
| Login | `/login` | Token input |
| Dashboard | `/` | Target cards grid + active sessions list |
| Session | `/session/:id` | Terminal view + collaboration sidebar |

### 8.3 Key Components

| Component | Description |
|-----------|-------------|
| `Terminal` | xterm.js wrapper, handles resize, bridges to WS/WebRTC |
| `TargetSelector` | Grid of available targets with tags and access indicators |
| `CollabOverlay` | Colored cursor overlay on terminal for each participant |
| `ParticipantList` | Connected users with roles, promote/demote/kick controls |
| `ChatPanel` | Sidebar text chat |
| `InviteDialog` | Generate and copy invite link with role selection |

### 8.4 Embedding Strategy

Frontend built by Vite, output embedded in the Gateway binary via `rust-embed`. In dev mode, Vite dev server runs separately with proxy to backend API.

## 9. Configuration

All config files use YAML format.

### 9.1 Server Config

```yaml
# ~/.telepair/config.yaml
server:
  host: "0.0.0.0"
  port: 7700

storage:
  type: sqlite                         # v1 only
  path: "~/.telepair/telepair.db"

auth:
  type: token                          # v1 only

webrtc:
  enabled: true
  stun_servers:
    - "stun:stun.l.google.com:19302"

session:
  idle_timeout: 3600                   # seconds
  max_scrollback: 10000                # lines
```

## 10. Key Dependencies

### Backend (Rust)

| Crate | Purpose |
|-------|---------|
| `axum` | HTTP/WS framework |
| `tokio` | Async runtime |
| `tokio-tungstenite` | WebSocket |
| `portable-pty` | Cross-platform PTY |
| `sqlx` | Async SQLite (compile-time checked) |
| `serde` / `serde_json` / `serde_yaml` | Serialization |
| `webrtc` | WebRTC (Rust native) |
| `rust-embed` | Embed frontend assets |
| `clap` | CLI parsing |
| `tracing` | Structured logging |
| `bcrypt` | Token hashing |
| `thiserror` | Error types |
| `uuid` | ID generation |

### Frontend (SolidJS)

| Package | Purpose |
|---------|---------|
| `solid-js` | Reactive UI |
| `@solidjs/router` | SPA routing |
| `@xterm/xterm` | Terminal emulator |
| `@xterm/addon-fit` | Auto-resize |
| `@xterm/addon-webgl` | GPU rendering |
| `vite` | Build tool |
| `typescript` | Type safety |

## 11. v2+ Roadmap (Out of Scope for v1)

| Feature | Description |
|---------|-------------|
| **E2EE** | End-to-end encryption for terminal data using Signal protocol or similar |
| **Recording/Playback** | Session recording (asciinema-compatible) with playback UI |
| **AI Enhancement** | AI-powered command suggestions, error explanation, auto-completion |
| **Kubernetes Integration** | `kubectl exec` support with namespace/pod/container selection |
| **Cluster Mode** | Multiple instances with PostgreSQL, service discovery, session migration |
| **OIDC Auth** | GitHub, Google, custom OIDC provider support |
| **P2P Mode** | Direct WebRTC connections without relay server |
| **Audit Log** | Detailed audit trail for compliance |

## 12. Verification Plan

### 12.1 Build Verification

```bash
# Backend
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Frontend
cd web && npm install && npm run build && npm run type-check
```

### 12.2 Functional Verification

1. **Basic flow**: Start `telepair standalone` -> open browser -> login with token -> select local shell target -> type commands -> see output
2. **Collaboration**: Open two browser tabs -> join same session -> verify both see same output -> test cursor visibility
3. **Virtual targets**: Configure a virtual target in `targets.yaml` -> verify it appears in dashboard -> connect and verify correct command executes
4. **Permissions**: Create session -> generate invite link with viewer role -> join with second user -> verify input is blocked
5. **WebRTC fallback**: Disable WebRTC in config -> verify terminal still works over WebSocket
6. **Chat**: Send chat messages between participants -> verify delivery

### 12.3 Edge Cases

- Terminal resize with multiple participants at different window sizes (uses minimum dimensions)
- Participant disconnect/reconnect mid-session (scrollback resent on rejoin)
- PTY process exit while session is active (session marked closed, participants notified)
- Invalid/expired invite tokens (clear error message returned)
- Concurrent session creation (no conflicts -- each session has unique ID)
