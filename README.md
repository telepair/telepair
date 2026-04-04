# telepair

**Google Docs for your terminal.** Share terminal sessions with collaborators in real time, right from the browser.

telepair is an open-source web terminal collaboration tool. Run it on any machine, open a browser, and invite teammates to view, operate, or co-drive terminal sessions with fine-grained permissions.

## Features

- **Real-time collaboration** — multiple users in one terminal session with live output streaming
- **Role-based permissions** — Owner, Operator, and Viewer roles control who can type, resize, or just watch
- **Invite links** — share a link to let others join your session with a specific role
- **Virtual targets** — define named commands (SSH, psql, htop, etc.) as launchable targets via YAML config
- **Built-in chat** — sidebar chat alongside the terminal for coordination
- **Single binary** — one binary with composable roles for flexible deployment
- **Web UI** — SolidJS frontend with xterm.js, no client install required

## Quick Start

### Prerequisites

- Rust 1.85+ (edition 2024)
- Node.js 18+

### Build

```bash
# Build the backend
cargo build --release

# Build the frontend
cd web && npm install && npm run build && cd ..
```

### Run

```bash
# Start telepair (all roles, default port 7700)
./target/release/telepair --web-dir web/dist
```

On first run, an admin user is created and a token is printed to the console:

```
INFO telepair: === First run: admin user created ===
INFO telepair: Admin token: <your-token>
INFO telepair: Save this token — it won't be shown again!
```

Open `http://localhost:7700` in your browser, paste the admin token to log in.

### Invite a collaborator

1. Launch a session from the dashboard by clicking a target
2. Click **Invite** in the top bar
3. Choose a role (Operator or Viewer) and copy the invite link
4. Share the link — the collaborator pastes their token and joins

## Architecture

telepair is a Cargo workspace with composable roles:

```
┌─────────────────────────────────────────────────┐
│                  telepair-cli                    │
│             (single binary entry point)          │
├────────────────┬────────────────┬────────────────┤
│  telepair-agent│telepair-control│telepair-gateway│
│  PTY management│  auth, sessions│  HTTP, WS, UI  │
│  virtual targets│  storage       │  API endpoints  │
├────────────────┴────────────────┴────────────────┤
│                  telepair-core                    │
│        types, traits, protocols, storage          │
└─────────────────────────────────────────────────┘
```

| Crate | Responsibility |
|-------|---------------|
| `telepair-core` | Shared types, Storage trait, protocol definitions, permissions |
| `telepair-agent` | PTY spawning via portable-pty, virtual target engine |
| `telepair-control` | Session lifecycle, target registry, auth service |
| `telepair-gateway` | Axum HTTP/WS server, REST API, static file serving |
| `telepair-cli` | CLI argument parsing, initialization, server startup |

### Composable Roles

```bash
telepair                          # all roles (single-node, default)
telepair --agent --gateway        # agent + gateway only
telepair --control                # control-only (headless)
```

No flags = all roles enabled. This is the recommended mode for single-machine use.

## Configuration

### CLI Options

```
telepair [OPTIONS]

Options:
      --agent              Enable agent role (PTY, virtual targets)
      --control            Enable control role (auth, sessions, storage)
      --gateway            Enable gateway role (HTTP/WS endpoints)
      --host <HOST>        Bind address [default: 0.0.0.0]
      --port <PORT>        Server port [default: 7700]
      --config <PATH>      Path to config file
      --targets <PATH>     Path to targets config [default: ~/.telepair/targets.yaml]
      --web-dir <PATH>     Path to web frontend dist directory
```

### Virtual Targets

Define named commands in `~/.telepair/targets.yaml`:

```yaml
targets:
  - name: production-db
    display: "Production DB"
    command: psql
    args: ["-h", "db.internal", "-U", "readonly", "production"]
    env:
      PGPASSWORD: "${PROD_DB_PASS}"
    tags: [database, production]

  - name: staging-ssh
    display: "Staging SSH"
    command: ssh
    args: ["deploy@staging.example.com"]
    tags: [server, staging]

  - name: monitor
    display: "System Monitor"
    command: htop
    tags: [monitoring]
```

Environment variables in `${VAR}` syntax are expanded at launch time. A default local shell target is always available.

### Data Directory

telepair stores its data in `~/.telepair/`:

```
~/.telepair/
├── telepair.db       # SQLite database (users, sessions, participants, invites)
└── targets.yaml      # Virtual target definitions (optional)
```

## Permissions

| Capability | Owner | Operator | Viewer |
|-----------|-------|----------|--------|
| View terminal output | Yes | Yes | Yes |
| Type into terminal | Yes | Yes | No |
| Resize terminal | Yes | Yes | No |
| Send chat messages | Yes | Yes | Yes |
| Create invite links | Yes | No | No |
| Close session | Yes | No | No |

## Development

```bash
# Run backend tests (49 tests)
cargo test --workspace

# Run frontend tests (22 tests)
cd web && npm test

# Type-check frontend
cd web && npm run type-check

# Dev mode: backend on :7700, frontend on :5173 with proxy
cargo run                          # terminal 1
cd web && npm run dev              # terminal 2
```

### Environment

```bash
# Adjust log level
RUST_LOG=debug ./target/release/telepair
```

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust, axum, tokio, sqlx (SQLite), portable-pty |
| Frontend | SolidJS, TypeScript, xterm.js, Vite |
| Protocol | JSON over WebSocket (control + collab), binary frames (terminal I/O) |
| Auth | Token-based with bcrypt hashing |
| Storage | SQLite (async via sqlx) |

## License

MIT OR Apache-2.0
