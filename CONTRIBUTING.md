# Contributing to telepair

Thank you for your interest in contributing to telepair! This guide will help you get started.

## Getting Started

### Prerequisites

- Rust 1.85+ (edition 2024)
- Node.js 18+
- SQLite (bundled via sqlx)

### Setup

```bash
git clone https://github.com/telepair/telepair.git
cd telepair

# Build backend
cargo build

# Install frontend dependencies
cd web && npm install && cd ..

# Run tests to verify setup
cargo test --workspace
cd web && npm test && cd ..
```

### Development Workflow

Run the backend and frontend in separate terminals:

```bash
# Terminal 1: backend on :7700
cargo run

# Terminal 2: frontend dev server on :5173 (proxies API to :7700)
cd web && npm run dev
```

Open `http://localhost:5173` in your browser. The Vite dev server proxies `/api` and `/ws` requests to the backend.

## Project Structure

```
telepair/
├── crates/
│   ├── telepair-core/       # Shared types, Storage trait, protocol
│   ├── telepair-agent/      # PTY management, virtual targets
│   ├── telepair-control/    # Session lifecycle, target registry
│   ├── telepair-gateway/    # HTTP/WS server, REST API
│   └── telepair-cli/        # Binary entry point
├── web/                     # SolidJS + TypeScript frontend
│   └── src/
│       ├── lib/             # API client, WebSocket, protocol types
│       ├── stores/          # Reactive state (auth, sessions)
│       ├── pages/           # Route pages
│       └── components/      # UI components
├── migrations/              # SQLite schema
└── docs/                    # Documentation
```

## Code Style

### Rust

- Edition 2024, stable toolchain (>= 1.85)
- Directory-based modules (`foo/bar.rs`), **not** `mod.rs` style
- Prefer returning `Result` over panicking
- Run `cargo clippy` before submitting

### TypeScript

- Strict mode enabled
- Run `npm run type-check` before submitting

### Commits

- [Conventional Commits](https://www.conventionalcommits.org/) format: `feat|fix|chore|refactor|perf|docs|test|ci`
- English imperative mood ("add feature", not "added feature")
- Sign-off required: use `git commit -s`
- One logical change per commit

## Testing

```bash
# Backend: all workspace tests
cargo test --workspace

# Frontend: unit tests
cd web && npm test

# Frontend: type checking
cd web && npm run type-check
```

- All new logic must have unit tests
- Test files live alongside the code they test (Rust: `tests/` directories, TS: `*.test.ts` next to source)

## Pull Requests

1. Fork the repo and create a feature branch from `main`
2. Make your changes with tests
3. Ensure all tests pass and clippy/type-check is clean
4. Submit a PR with a clear description of what and why

## Reporting Issues

File issues at [github.com/telepair/telepair/issues](https://github.com/telepair/telepair/issues). Include:

- What you expected vs. what happened
- Steps to reproduce
- OS, Rust version, browser version

## License

By contributing, you agree that your contributions will be licensed under the MIT OR Apache-2.0 license.
