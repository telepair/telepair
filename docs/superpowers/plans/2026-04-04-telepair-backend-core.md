# Telepair Backend Core — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the complete Rust backend for Telepair — a web-based terminal collaboration tool. This plan delivers a working `telepair` binary that serves a REST API and WebSocket endpoint for single-user terminal sessions (local shell + virtual targets). Testable with `curl` and `wscat` — no frontend needed.

**Architecture:** Cargo workspace with 5 crates: `telepair-core` (shared types/traits/storage), `telepair-agent` (PTY + virtual targets), `telepair-control` (business logic services), `telepair-gateway` (HTTP/WS endpoints), `telepair-cli` (binary entry point). Single binary with composable role flags (`--agent`, `--control`, `--gateway`; default = all). Components communicate via in-process tokio channels when co-located.

**Tech Stack:** Rust 2024 (stable >= 1.85), axum, tokio, portable-pty, sqlx (SQLite), serde, clap, tracing, bcrypt, thiserror

**Design Spec:** `docs/superpowers/specs/2026-04-04-telepair-v1-design.md`

**Project Location:** `/Users/liys/workspace/github.com/telepair/telepair/`

---

## Task Dependencies

```
Task 1 (scaffolding)
  |
  +---> Task 2 (error + permission) --+
  |                                    |
  +---> Task 3 (session + target) ----+---> Task 5 (storage) --+
  |                                    |                        |
  +---> Task 4 (protocol) ------------+---> Task 6 (auth) -----+---> Task 9 (control) --+
                                       |                                                  |
                                       +---> Task 7 (PTY) ----+                          |
                                       |                       +---> Task 12 (CLI + wire) 
                                       +---> Task 8 (vtarget) -+                          |
                                                                                          |
                                       Task 10 (HTTP routes) --+---> Task 11 (WS + hub) -+
```

Tasks 2/3/4 can run in parallel. Tasks 5/6 can run in parallel. Tasks 7/8 can run in parallel. Tasks 10/11 are sequential.

---

### Task 1: Repository & Workspace Scaffolding

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/telepair-core/Cargo.toml`
- Create: `crates/telepair-core/src/lib.rs`
- Create: `crates/telepair-agent/Cargo.toml`
- Create: `crates/telepair-agent/src/lib.rs`
- Create: `crates/telepair-control/Cargo.toml`
- Create: `crates/telepair-control/src/lib.rs`
- Create: `crates/telepair-gateway/Cargo.toml`
- Create: `crates/telepair-gateway/src/lib.rs`
- Create: `crates/telepair-cli/Cargo.toml`
- Create: `crates/telepair-cli/src/main.rs`
- Create: `.gitignore`
- Create: `rust-toolchain.toml`

- [ ] **Step 1: Create project directory and init git**

```bash
mkdir -p /Users/liys/workspace/github.com/telepair/telepair
cd /Users/liys/workspace/github.com/telepair/telepair
git init
```

- [ ] **Step 2: Create workspace root Cargo.toml**

```toml
# Cargo.toml
[workspace]
resolver = "2"
members = [
    "crates/telepair-core",
    "crates/telepair-agent",
    "crates/telepair-control",
    "crates/telepair-gateway",
    "crates/telepair-cli",
]

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT OR Apache-2.0"
repository = "https://github.com/telepair/telepair"

[workspace.dependencies]
# Internal crates
telepair-core = { path = "crates/telepair-core" }
telepair-agent = { path = "crates/telepair-agent" }
telepair-control = { path = "crates/telepair-control" }
telepair-gateway = { path = "crates/telepair-gateway" }

# Async runtime
tokio = { version = "1", features = ["full"] }

# Web framework
axum = { version = "0.8", features = ["ws"] }
axum-extra = { version = "0.10", features = ["typed-header"] }
tower-http = { version = "0.6", features = ["cors", "fs"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"

# Database
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }

# PTY
portable-pty = "0.8"

# Error handling
thiserror = "2"

# CLI
clap = { version = "4", features = ["derive"] }

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Auth
bcrypt = "0.16"

# IDs
uuid = { version = "1", features = ["v4", "serde"] }
nanoid = "0.4"

# Time
chrono = { version = "0.4", features = ["serde"] }

# Futures
futures = "0.3"
```

- [ ] **Step 3: Create telepair-core crate**

```toml
# crates/telepair-core/Cargo.toml
[package]
name = "telepair-core"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
serde_yaml = { workspace = true }
sqlx = { workspace = true }
thiserror = { workspace = true }
uuid = { workspace = true }
nanoid = { workspace = true }
chrono = { workspace = true }
bcrypt = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["test-util"] }
```

```rust
// crates/telepair-core/src/lib.rs
#![deny(unsafe_code)]

pub mod error;
pub mod permission;
```

- [ ] **Step 4: Create telepair-agent crate**

```toml
# crates/telepair-agent/Cargo.toml
[package]
name = "telepair-agent"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
telepair-core = { workspace = true }
portable-pty = { workspace = true }
serde = { workspace = true }
serde_yaml = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
futures = { workspace = true }
```

```rust
// crates/telepair-agent/src/lib.rs
#![deny(unsafe_code)]
```

- [ ] **Step 5: Create telepair-control crate**

```toml
# crates/telepair-control/Cargo.toml
[package]
name = "telepair-control"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
telepair-core = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
```

```rust
// crates/telepair-control/src/lib.rs
#![deny(unsafe_code)]
```

- [ ] **Step 6: Create telepair-gateway crate**

```toml
# crates/telepair-gateway/Cargo.toml
[package]
name = "telepair-gateway"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
telepair-core = { workspace = true }
telepair-control = { workspace = true }
telepair-agent = { workspace = true }
axum = { workspace = true }
axum-extra = { workspace = true }
tower-http = { workspace = true }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
futures = { workspace = true }
```

```rust
// crates/telepair-gateway/src/lib.rs
#![deny(unsafe_code)]
```

- [ ] **Step 7: Create telepair-cli crate**

```toml
# crates/telepair-cli/Cargo.toml
[package]
name = "telepair"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[[bin]]
name = "telepair"
path = "src/main.rs"

[dependencies]
telepair-core = { workspace = true }
telepair-agent = { workspace = true }
telepair-control = { workspace = true }
telepair-gateway = { workspace = true }
clap = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
```

```rust
// crates/telepair-cli/src/main.rs
fn main() {
    println!("telepair v0.1.0");
}
```

- [ ] **Step 8: Create .gitignore and rust-toolchain.toml**

```gitignore
# .gitignore
/target
.superpowers/
*.swp
*.swo
.DS_Store
```

```toml
# rust-toolchain.toml
[toolchain]
channel = "stable"
```

- [ ] **Step 9: Verify workspace builds**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo check --workspace 2>&1 | cat`
Expected: compilation succeeds with no errors

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -s -m "feat: init cargo workspace with 5 crates"
```

---

### Task 2: telepair-core — Error Types & Permission Model

**Files:**
- Create: `crates/telepair-core/src/error.rs`
- Create: `crates/telepair-core/src/permission.rs`
- Modify: `crates/telepair-core/src/lib.rs`
- Create: `crates/telepair-core/tests/permission_test.rs`

**Depends on:** Task 1

- [ ] **Step 1: Write permission tests**

```rust
// crates/telepair-core/tests/permission_test.rs
use telepair_core::permission::Role;

#[test]
fn owner_has_all_permissions() {
    let role = Role::Owner;
    assert!(role.can_input());
    assert!(role.can_resize());
    assert!(role.can_manage_participants());
    assert!(role.can_close_session());
}

#[test]
fn operator_can_input_but_not_manage() {
    let role = Role::Operator;
    assert!(role.can_input());
    assert!(role.can_resize());
    assert!(!role.can_manage_participants());
    assert!(!role.can_close_session());
}

#[test]
fn viewer_is_read_only() {
    let role = Role::Viewer;
    assert!(!role.can_input());
    assert!(!role.can_resize());
    assert!(!role.can_manage_participants());
    assert!(!role.can_close_session());
}

#[test]
fn role_serializes_as_lowercase() {
    let json = serde_json::to_string(&Role::Owner).unwrap();
    assert_eq!(json, r#""owner""#);

    let parsed: Role = serde_json::from_str(r#""operator""#).unwrap();
    assert_eq!(parsed, Role::Operator);
}

#[test]
fn role_display() {
    assert_eq!(Role::Owner.as_str(), "owner");
    assert_eq!(Role::Operator.as_str(), "operator");
    assert_eq!(Role::Viewer.as_str(), "viewer");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core --test permission_test 2>&1 | cat`
Expected: FAIL — module `permission` not found

- [ ] **Step 3: Implement error.rs**

```rust
// crates/telepair-core/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("target not found: {0}")]
    TargetNotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("storage error: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 4: Implement permission.rs**

```rust
// crates/telepair-core/src/permission.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Operator,
    Viewer,
}

impl Role {
    pub fn can_input(&self) -> bool {
        matches!(self, Self::Owner | Self::Operator)
    }

    pub fn can_resize(&self) -> bool {
        matches!(self, Self::Owner | Self::Operator)
    }

    pub fn can_manage_participants(&self) -> bool {
        matches!(self, Self::Owner)
    }

    pub fn can_close_session(&self) -> bool {
        matches!(self, Self::Owner)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Operator => "operator",
            Self::Viewer => "viewer",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
```

- [ ] **Step 5: Update lib.rs**

```rust
// crates/telepair-core/src/lib.rs
#![deny(unsafe_code)]

pub mod error;
pub mod permission;

pub use error::{Error, Result};
pub use permission::Role;
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core --test permission_test 2>&1 | cat`
Expected: all 5 tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/telepair-core/src/error.rs crates/telepair-core/src/permission.rs crates/telepair-core/src/lib.rs crates/telepair-core/tests/permission_test.rs
git commit -s -m "feat(core): add error types and permission model"
```

---

### Task 3: telepair-core — Session, Target & Config Types

**Files:**
- Create: `crates/telepair-core/src/session.rs`
- Create: `crates/telepair-core/src/target.rs`
- Create: `crates/telepair-core/src/config.rs`
- Modify: `crates/telepair-core/src/lib.rs`
- Create: `crates/telepair-core/tests/target_test.rs`

**Depends on:** Task 2

- [ ] **Step 1: Write target config parsing tests**

```rust
// crates/telepair-core/tests/target_test.rs
use telepair_core::target::{TargetConfig, TargetKind};

const YAML_CONFIG: &str = r#"
targets:
  - name: production-db
    display: "Production DB"
    command: psql
    args: ["-h", "db.internal", "-U", "readonly"]
    env:
      PGPASSWORD: "${PROD_DB_PASS}"
    tags: [database]
    required_role: operator

  - name: local-shell
    display: "Local Shell"
    type: local
"#;

#[test]
fn parse_targets_yaml() {
    let config: TargetConfig = serde_yaml::from_str(YAML_CONFIG).unwrap();
    assert_eq!(config.targets.len(), 2);

    let db = &config.targets[0];
    assert_eq!(db.name, "production-db");
    assert_eq!(db.display, "Production DB");
    assert_eq!(db.kind, TargetKind::Virtual);
    assert_eq!(db.command.as_deref(), Some("psql"));
    assert_eq!(db.args, vec!["-h", "db.internal", "-U", "readonly"]);
    assert_eq!(db.env.get("PGPASSWORD").unwrap(), "${PROD_DB_PASS}");

    let shell = &config.targets[1];
    assert_eq!(shell.kind, TargetKind::Local);
    assert!(shell.command.is_none());
}

#[test]
fn env_var_substitution() {
    std::env::set_var("TEST_VAR_TELEPAIR", "secret123");
    let result = telepair_core::target::substitute_env_vars("prefix_${TEST_VAR_TELEPAIR}_suffix");
    assert_eq!(result, "prefix_secret123_suffix");
    std::env::remove_var("TEST_VAR_TELEPAIR");
}

#[test]
fn missing_env_var_kept_as_is() {
    let result = telepair_core::target::substitute_env_vars("${DEFINITELY_NOT_SET_XYZ}");
    assert_eq!(result, "${DEFINITELY_NOT_SET_XYZ}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core --test target_test 2>&1 | cat`
Expected: FAIL — module `target` not found

- [ ] **Step 3: Implement session.rs**

```rust
// crates/telepair-core/src/session.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::permission::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputMode {
    Serialized,
    Multiplexed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub owner_id: Uuid,
    pub target_name: String,
    pub input_mode: InputMode,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub session_id: String,
    pub user_id: Uuid,
    pub role: Role,
    pub joined_at: DateTime<Utc>,
    pub left_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub name: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteToken {
    pub token_hash: String,
    pub session_id: String,
    pub role: Role,
    pub max_uses: i32,
    pub used_count: i32,
    pub expires_at: Option<DateTime<Utc>>,
}
```

- [ ] **Step 4: Implement target.rs**

```rust
// crates/telepair-core/src/target.rs
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::permission::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    #[default]
    Virtual,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    pub display: String,
    #[serde(default, rename = "type")]
    pub kind: TargetKind,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub required_role: Option<Role>,
    pub shell: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    pub targets: Vec<Target>,
}

/// Substitute `${VAR}` patterns with environment variable values.
/// If the variable is not set, the pattern is left as-is.
pub fn substitute_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let var_name: String = chars.by_ref().take_while(|&c| c != '}').collect();
            match std::env::var(&var_name) {
                Ok(val) => result.push_str(&val),
                Err(_) => {
                    result.push_str("${");
                    result.push_str(&var_name);
                    result.push('}');
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}
```

- [ ] **Step 5: Implement config.rs**

```rust
// crates/telepair-core/src/config.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_server")]
    pub server: ServerConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub session: SessionDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_storage_type")]
    pub r#type: String,
    #[serde(default = "default_db_path")]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_auth_type")]
    pub r#type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDefaults {
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: u64,
    #[serde(default = "default_max_scrollback")]
    pub max_scrollback: usize,
}

fn default_server() -> ServerConfig {
    ServerConfig { host: default_host(), port: default_port() }
}
fn default_host() -> String { "0.0.0.0".into() }
fn default_port() -> u16 { 7700 }
fn default_storage_type() -> String { "sqlite".into() }
fn default_db_path() -> String { "~/.telepair/telepair.db".into() }
fn default_auth_type() -> String { "token".into() }
fn default_idle_timeout() -> u64 { 3600 }
fn default_max_scrollback() -> usize { 10000 }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: default_server(),
            storage: StorageConfig::default(),
            auth: AuthConfig::default(),
            session: SessionDefaults::default(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self { r#type: default_storage_type(), path: default_db_path() }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self { r#type: default_auth_type() }
    }
}

impl Default for SessionDefaults {
    fn default() -> Self {
        Self { idle_timeout: default_idle_timeout(), max_scrollback: default_max_scrollback() }
    }
}
```

- [ ] **Step 6: Update lib.rs**

```rust
// crates/telepair-core/src/lib.rs
#![deny(unsafe_code)]

pub mod config;
pub mod error;
pub mod permission;
pub mod session;
pub mod target;

pub use error::{Error, Result};
pub use permission::Role;
```

- [ ] **Step 7: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core 2>&1 | cat`
Expected: all tests PASS

- [ ] **Step 8: Commit**

```bash
git add crates/telepair-core/
git commit -s -m "feat(core): add session, target, and config types"
```

---

### Task 4: telepair-core — Protocol Messages

**Files:**
- Create: `crates/telepair-core/src/protocol.rs`
- Modify: `crates/telepair-core/src/lib.rs`
- Create: `crates/telepair-core/tests/protocol_test.rs`

**Depends on:** Task 2

- [ ] **Step 1: Write protocol serialization tests**

```rust
// crates/telepair-core/tests/protocol_test.rs
use telepair_core::protocol::{ClientMessage, ServerMessage};

#[test]
fn client_term_input_roundtrip() {
    let msg = ClientMessage::TermInput { data: vec![0x1b, 0x5b, 0x41] }; // ESC[A (arrow up)
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: ClientMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn client_session_join_json() {
    let msg = ClientMessage::SessionJoin {
        session_id: "abc123".into(),
        token: "tok_secret".into(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("SessionJoin"));
    assert!(json.contains("abc123"));
}

#[test]
fn server_term_output_roundtrip() {
    let msg = ServerMessage::TermOutput { data: vec![0x48, 0x65, 0x6c, 0x6c, 0x6f] };
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn server_error_json() {
    let msg = ServerMessage::Error {
        code: "PERM_DENIED".into(),
        message: "you cannot type in this session".into(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("PERM_DENIED"));
}

#[test]
fn binary_frame_encode_decode() {
    use telepair_core::protocol::{BinaryFrame, BinaryFrameType};

    let frame = BinaryFrame {
        frame_type: BinaryFrameType::Output,
        payload: b"hello world".to_vec(),
    };
    let bytes = frame.encode();
    assert_eq!(bytes[0], 0x01); // Output type
    assert_eq!(u16::from_be_bytes([bytes[1], bytes[2]]), 11); // payload len

    let decoded = BinaryFrame::decode(&bytes).unwrap();
    assert_eq!(decoded.frame_type, BinaryFrameType::Output);
    assert_eq!(decoded.payload, b"hello world");
}

#[test]
fn binary_frame_resize() {
    use telepair_core::protocol::BinaryFrame;

    let frame = BinaryFrame::resize(120, 40);
    let bytes = frame.encode();
    let decoded = BinaryFrame::decode(&bytes).unwrap();
    let (cols, rows) = decoded.parse_resize().unwrap();
    assert_eq!(cols, 120);
    assert_eq!(rows, 40);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core --test protocol_test 2>&1 | cat`
Expected: FAIL — module `protocol` not found

- [ ] **Step 3: Implement protocol.rs**

```rust
// crates/telepair-core/src/protocol.rs
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::permission::Role;
use crate::session::{InputMode, Participant, Session};

// --- Client -> Server ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    SessionJoin { session_id: String, token: String },
    TermInput { data: Vec<u8> },
    TermResize { cols: u16, rows: u16 },
    CursorMove { x: u16, y: u16 },
    ChatMessage { text: String },
}

// --- Server -> Client ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    SessionState {
        session: Session,
        participants: Vec<ParticipantInfo>,
        your_role: Role,
    },
    TermOutput { data: Vec<u8> },
    PeerJoined { user_id: Uuid, name: String, role: Role, color: String },
    PeerLeft { user_id: Uuid },
    PeerCursor { user_id: Uuid, x: u16, y: u16 },
    PeerChat { user_id: Uuid, name: String, text: String, ts: String },
    PermUpdate { user_id: Uuid, new_role: Role },
    Error { code: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub user_id: Uuid,
    pub name: String,
    pub role: Role,
    pub color: String,
}

// --- Binary Frame Protocol ---
// [1B type][2B length (big-endian)][payload]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BinaryFrameType {
    Output = 0x01,
    Input = 0x02,
    Resize = 0x03,
}

impl BinaryFrameType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Output),
            0x02 => Some(Self::Input),
            0x03 => Some(Self::Resize),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryFrame {
    pub frame_type: BinaryFrameType,
    pub payload: Vec<u8>,
}

impl BinaryFrame {
    pub fn encode(&self) -> Vec<u8> {
        let len = self.payload.len() as u16;
        let mut buf = Vec::with_capacity(3 + self.payload.len());
        buf.push(self.frame_type as u8);
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 3 {
            return None;
        }
        let frame_type = BinaryFrameType::from_byte(data[0])?;
        let len = u16::from_be_bytes([data[1], data[2]]) as usize;
        if data.len() < 3 + len {
            return None;
        }
        Some(Self {
            frame_type,
            payload: data[3..3 + len].to_vec(),
        })
    }

    pub fn resize(cols: u16, rows: u16) -> Self {
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&cols.to_be_bytes());
        payload.extend_from_slice(&rows.to_be_bytes());
        Self { frame_type: BinaryFrameType::Resize, payload }
    }

    pub fn parse_resize(&self) -> Option<(u16, u16)> {
        if self.frame_type != BinaryFrameType::Resize || self.payload.len() != 4 {
            return None;
        }
        let cols = u16::from_be_bytes([self.payload[0], self.payload[1]]);
        let rows = u16::from_be_bytes([self.payload[2], self.payload[3]]);
        Some((cols, rows))
    }
}
```

- [ ] **Step 4: Update lib.rs to export protocol**

Add `pub mod protocol;` to `crates/telepair-core/src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core --test protocol_test 2>&1 | cat`
Expected: all 6 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/telepair-core/
git commit -s -m "feat(core): add WebSocket and binary frame protocol messages"
```

---

### Task 5: telepair-core — Storage Layer (SQLite)

**Files:**
- Create: `crates/telepair-core/src/storage.rs`
- Create: `crates/telepair-core/src/storage/sqlite.rs`
- Create: `migrations/001_initial.sql`
- Modify: `crates/telepair-core/src/lib.rs`
- Create: `crates/telepair-core/tests/storage_test.rs`

**Depends on:** Tasks 2, 3

- [ ] **Step 1: Create SQL migration**

```sql
-- migrations/001_initial.sql
CREATE TABLE IF NOT EXISTS users (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    token_hash  TEXT NOT NULL,
    is_admin    BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL REFERENCES users(id),
    target_name TEXT NOT NULL,
    input_mode  TEXT NOT NULL DEFAULT 'serialized',
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT NOT NULL,
    closed_at   TEXT
);

CREATE TABLE IF NOT EXISTS participants (
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    user_id     TEXT NOT NULL REFERENCES users(id),
    role        TEXT NOT NULL,
    joined_at   TEXT NOT NULL,
    left_at     TEXT,
    PRIMARY KEY (session_id, user_id)
);

CREATE TABLE IF NOT EXISTS invite_tokens (
    token_hash  TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    role        TEXT NOT NULL,
    max_uses    INTEGER NOT NULL DEFAULT 1,
    used_count  INTEGER NOT NULL DEFAULT 0,
    expires_at  TEXT
);
```

- [ ] **Step 2: Write storage tests**

```rust
// crates/telepair-core/tests/storage_test.rs
use telepair_core::permission::Role;
use telepair_core::session::InputMode;
use telepair_core::storage::{Storage, SqliteStorage};

async fn setup() -> SqliteStorage {
    SqliteStorage::new_memory().await.unwrap()
}

#[tokio::test]
async fn create_and_get_user() {
    let store = setup().await;
    let (user, token) = store.create_user("alice", true).await.unwrap();
    assert_eq!(user.name, "alice");
    assert!(user.is_admin);
    assert!(!token.is_empty());

    let fetched = store.get_user(user.id).await.unwrap().unwrap();
    assert_eq!(fetched.name, "alice");
}

#[tokio::test]
async fn validate_token() {
    let store = setup().await;
    let (user, token) = store.create_user("bob", false).await.unwrap();
    let validated = store.validate_token(&token).await.unwrap();
    assert_eq!(validated.id, user.id);
}

#[tokio::test]
async fn invalid_token_fails() {
    let store = setup().await;
    store.create_user("carol", false).await.unwrap();
    let result = store.validate_token("wrong-token").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn create_and_get_session() {
    let store = setup().await;
    let (user, _) = store.create_user("dave", false).await.unwrap();
    let session = store
        .create_session(user.id, "local-shell", InputMode::Serialized)
        .await
        .unwrap();
    assert_eq!(session.target_name, "local-shell");

    let fetched = store.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, session.id);
}

#[tokio::test]
async fn add_and_list_participants() {
    let store = setup().await;
    let (owner, _) = store.create_user("eve", false).await.unwrap();
    let (viewer, _) = store.create_user("frank", false).await.unwrap();
    let session = store.create_session(owner.id, "shell", InputMode::Serialized).await.unwrap();

    store.add_participant(&session.id, owner.id, Role::Owner).await.unwrap();
    store.add_participant(&session.id, viewer.id, Role::Viewer).await.unwrap();

    let participants = store.list_participants(&session.id).await.unwrap();
    assert_eq!(participants.len(), 2);
}

#[tokio::test]
async fn close_session() {
    let store = setup().await;
    let (user, _) = store.create_user("grace", false).await.unwrap();
    let session = store.create_session(user.id, "shell", InputMode::Serialized).await.unwrap();

    store.close_session(&session.id).await.unwrap();
    let fetched = store.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(fetched.status, telepair_core::session::SessionStatus::Closed);
    assert!(fetched.closed_at.is_some());
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core --test storage_test 2>&1 | cat`
Expected: FAIL — module `storage` not found or missing types

- [ ] **Step 4: Implement storage trait (storage.rs)**

```rust
// crates/telepair-core/src/storage.rs
pub mod sqlite;

use uuid::Uuid;

use crate::error::Result;
use crate::permission::Role;
use crate::session::{InputMode, Participant, Session, User};

pub use sqlite::SqliteStorage;

#[trait_variant::make(Send)]
pub trait Storage: Sync {
    // Users
    async fn create_user(&self, name: &str, is_admin: bool) -> Result<(User, String)>;
    async fn get_user(&self, id: Uuid) -> Result<Option<User>>;
    async fn get_user_by_name(&self, name: &str) -> Result<Option<User>>;
    async fn validate_token(&self, token: &str) -> Result<User>;

    // Sessions
    async fn create_session(&self, owner_id: Uuid, target_name: &str, input_mode: InputMode) -> Result<Session>;
    async fn get_session(&self, id: &str) -> Result<Option<Session>>;
    async fn close_session(&self, id: &str) -> Result<()>;
    async fn list_active_sessions(&self) -> Result<Vec<Session>>;

    // Participants
    async fn add_participant(&self, session_id: &str, user_id: Uuid, role: Role) -> Result<Participant>;
    async fn remove_participant(&self, session_id: &str, user_id: Uuid) -> Result<()>;
    async fn list_participants(&self, session_id: &str) -> Result<Vec<Participant>>;
}
```

Note: `trait_variant` is needed for async trait methods that are Send. Add `trait-variant = "0.1"` to `[workspace.dependencies]` in root `Cargo.toml` and add `trait-variant = { workspace = true }` to `crates/telepair-core/Cargo.toml` dependencies.

**Alternative if `trait_variant` is not desired:** Use `#[async_trait]` from the `async-trait` crate, or just use native async traits with manual `Send` bounds:

```rust
// If preferring no extra crate, use this simpler approach:
pub trait Storage: Send + Sync {
    fn create_user(&self, name: &str, is_admin: bool) -> impl Future<Output = Result<(User, String)>> + Send;
    // ... etc
}
```

The implementer should choose whichever approach compiles cleanly on the target Rust version (>= 1.85 supports native async in traits).

- [ ] **Step 5: Implement SqliteStorage (storage/sqlite.rs)**

```rust
// crates/telepair-core/src/storage/sqlite.rs
use chrono::Utc;
use sqlx::{Pool, Sqlite, SqlitePool, Row};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::permission::Role;
use crate::session::{InputMode, Participant, Session, SessionStatus, User};
use crate::storage::Storage;

pub struct SqliteStorage {
    pool: Pool<Sqlite>,
}

impl SqliteStorage {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePool::connect(database_url).await?;
        let storage = Self { pool };
        storage.run_migrations().await?;
        Ok(storage)
    }

    pub async fn new_memory() -> Result<Self> {
        Self::new("sqlite::memory:").await
    }

    async fn run_migrations(&self) -> Result<()> {
        sqlx::query(include_str!("../../../migrations/001_initial.sql"))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

impl Storage for SqliteStorage {
    async fn create_user(&self, name: &str, is_admin: bool) -> Result<(User, String)> {
        let id = Uuid::new_v4();
        let token = nanoid::nanoid!(32);
        let token_hash = bcrypt::hash(&token, 10)
            .map_err(|e| Error::Auth(e.to_string()))?;
        let now = Utc::now();

        sqlx::query("INSERT INTO users (id, name, token_hash, is_admin, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(id.to_string())
            .bind(name)
            .bind(&token_hash)
            .bind(is_admin)
            .bind(now.to_rfc3339())
            .bind(now.to_rfc3339())
            .execute(&self.pool)
            .await?;

        let user = User { id, name: name.into(), is_admin, created_at: now, updated_at: now };
        Ok((user, token))
    }

    async fn get_user(&self, id: Uuid) -> Result<Option<User>> {
        let row = sqlx::query("SELECT id, name, is_admin, created_at, updated_at FROM users WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| User {
            id: r.get::<String, _>("id").parse().unwrap(),
            name: r.get("name"),
            is_admin: r.get("is_admin"),
            created_at: r.get::<String, _>("created_at").parse().unwrap(),
            updated_at: r.get::<String, _>("updated_at").parse().unwrap(),
        }))
    }

    async fn get_user_by_name(&self, name: &str) -> Result<Option<User>> {
        let row = sqlx::query("SELECT id, name, is_admin, created_at, updated_at FROM users WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| User {
            id: r.get::<String, _>("id").parse().unwrap(),
            name: r.get("name"),
            is_admin: r.get("is_admin"),
            created_at: r.get::<String, _>("created_at").parse().unwrap(),
            updated_at: r.get::<String, _>("updated_at").parse().unwrap(),
        }))
    }

    async fn validate_token(&self, token: &str) -> Result<User> {
        // Fetch all users and check token hash (small user count in v1 makes this fine)
        let rows = sqlx::query("SELECT id, name, token_hash, is_admin, created_at, updated_at FROM users")
            .fetch_all(&self.pool)
            .await?;

        for row in rows {
            let hash: String = row.get("token_hash");
            if bcrypt::verify(token, &hash).unwrap_or(false) {
                return Ok(User {
                    id: row.get::<String, _>("id").parse().unwrap(),
                    name: row.get("name"),
                    is_admin: row.get("is_admin"),
                    created_at: row.get::<String, _>("created_at").parse().unwrap(),
                    updated_at: row.get::<String, _>("updated_at").parse().unwrap(),
                });
            }
        }
        Err(Error::Auth("invalid token".into()))
    }

    async fn create_session(&self, owner_id: Uuid, target_name: &str, input_mode: InputMode) -> Result<Session> {
        let id = nanoid::nanoid!(10);
        let now = Utc::now();
        let mode_str = match input_mode {
            InputMode::Serialized => "serialized",
            InputMode::Multiplexed => "multiplexed",
        };

        sqlx::query("INSERT INTO sessions (id, owner_id, target_name, input_mode, status, created_at) VALUES (?, ?, ?, ?, 'active', ?)")
            .bind(&id)
            .bind(owner_id.to_string())
            .bind(target_name)
            .bind(mode_str)
            .bind(now.to_rfc3339())
            .execute(&self.pool)
            .await?;

        Ok(Session {
            id, owner_id, target_name: target_name.into(),
            input_mode, status: SessionStatus::Active,
            created_at: now, closed_at: None,
        })
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| Session {
            id: r.get("id"),
            owner_id: r.get::<String, _>("owner_id").parse().unwrap(),
            target_name: r.get("target_name"),
            input_mode: match r.get::<String, _>("input_mode").as_str() {
                "multiplexed" => InputMode::Multiplexed,
                _ => InputMode::Serialized,
            },
            status: match r.get::<String, _>("status").as_str() {
                "closed" => SessionStatus::Closed,
                _ => SessionStatus::Active,
            },
            created_at: r.get::<String, _>("created_at").parse().unwrap(),
            closed_at: r.get::<Option<String>, _>("closed_at").and_then(|s| s.parse().ok()),
        }))
    }

    async fn close_session(&self, id: &str) -> Result<()> {
        let now = Utc::now();
        sqlx::query("UPDATE sessions SET status = 'closed', closed_at = ? WHERE id = ?")
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_active_sessions(&self) -> Result<Vec<Session>> {
        let rows = sqlx::query("SELECT * FROM sessions WHERE status = 'active'")
            .fetch_all(&self.pool)
            .await?;

        Ok(rows.into_iter().map(|r| Session {
            id: r.get("id"),
            owner_id: r.get::<String, _>("owner_id").parse().unwrap(),
            target_name: r.get("target_name"),
            input_mode: match r.get::<String, _>("input_mode").as_str() {
                "multiplexed" => InputMode::Multiplexed,
                _ => InputMode::Serialized,
            },
            status: SessionStatus::Active,
            created_at: r.get::<String, _>("created_at").parse().unwrap(),
            closed_at: None,
        }).collect())
    }

    async fn add_participant(&self, session_id: &str, user_id: Uuid, role: Role) -> Result<Participant> {
        let now = Utc::now();
        sqlx::query("INSERT INTO participants (session_id, user_id, role, joined_at) VALUES (?, ?, ?, ?)")
            .bind(session_id)
            .bind(user_id.to_string())
            .bind(role.as_str())
            .bind(now.to_rfc3339())
            .execute(&self.pool)
            .await?;

        Ok(Participant {
            session_id: session_id.into(),
            user_id, role,
            joined_at: now, left_at: None,
        })
    }

    async fn remove_participant(&self, session_id: &str, user_id: Uuid) -> Result<()> {
        let now = Utc::now();
        sqlx::query("UPDATE participants SET left_at = ? WHERE session_id = ? AND user_id = ?")
            .bind(now.to_rfc3339())
            .bind(session_id)
            .bind(user_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_participants(&self, session_id: &str) -> Result<Vec<Participant>> {
        let rows = sqlx::query("SELECT * FROM participants WHERE session_id = ? AND left_at IS NULL")
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows.into_iter().map(|r| {
            let role_str: String = r.get("role");
            Participant {
                session_id: r.get("session_id"),
                user_id: r.get::<String, _>("user_id").parse().unwrap(),
                role: match role_str.as_str() {
                    "owner" => Role::Owner,
                    "operator" => Role::Operator,
                    _ => Role::Viewer,
                },
                joined_at: r.get::<String, _>("joined_at").parse().unwrap(),
                left_at: None,
            }
        }).collect())
    }
}
```

- [ ] **Step 6: Update lib.rs**

Add `pub mod storage;` to `crates/telepair-core/src/lib.rs`.

- [ ] **Step 7: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core --test storage_test 2>&1 | cat`
Expected: all 6 tests PASS

- [ ] **Step 8: Commit**

```bash
git add migrations/ crates/telepair-core/
git commit -s -m "feat(core): add storage trait with SQLite implementation"
```

---

### Task 6: telepair-core — Auth System

**Files:**
- Create: `crates/telepair-core/src/auth.rs`
- Modify: `crates/telepair-core/src/lib.rs`
- Create: `crates/telepair-core/tests/auth_test.rs`

**Depends on:** Task 5

- [ ] **Step 1: Write auth tests**

```rust
// crates/telepair-core/tests/auth_test.rs
use telepair_core::auth::TokenAuthProvider;
use telepair_core::storage::SqliteStorage;
use std::sync::Arc;

async fn setup() -> (TokenAuthProvider, String) {
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, token) = store.create_user("test-user", false).await.unwrap();
    let auth = TokenAuthProvider::new(store);
    (auth, token)
}

#[tokio::test]
async fn valid_token_returns_user() {
    let (auth, token) = setup().await;
    let user = auth.validate(&token).await.unwrap();
    assert_eq!(user.name, "test-user");
}

#[tokio::test]
async fn invalid_token_returns_error() {
    let (auth, _) = setup().await;
    let result = auth.validate("bad-token").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn setup_initial_admin() {
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let auth = TokenAuthProvider::new(store);
    let (user, token) = auth.setup_initial_admin("admin").await.unwrap();
    assert!(user.is_admin);

    let validated = auth.validate(&token).await.unwrap();
    assert_eq!(validated.id, user.id);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core --test auth_test 2>&1 | cat`
Expected: FAIL — module `auth` not found

- [ ] **Step 3: Implement auth.rs**

```rust
// crates/telepair-core/src/auth.rs
use std::sync::Arc;

use crate::error::Result;
use crate::session::User;
use crate::storage::{SqliteStorage, Storage};

pub struct TokenAuthProvider {
    storage: Arc<SqliteStorage>,
}

impl TokenAuthProvider {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    pub async fn validate(&self, token: &str) -> Result<User> {
        self.storage.validate_token(token).await
    }

    pub async fn create_user(&self, name: &str) -> Result<(User, String)> {
        self.storage.create_user(name, false).await
    }

    pub async fn setup_initial_admin(&self, name: &str) -> Result<(User, String)> {
        self.storage.create_user(name, true).await
    }
}
```

- [ ] **Step 4: Update lib.rs**

Add `pub mod auth;` to `crates/telepair-core/src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-core --test auth_test 2>&1 | cat`
Expected: all 3 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/telepair-core/
git commit -s -m "feat(core): add token-based auth provider"
```

---

### Task 7: telepair-agent — PTY Manager

**Files:**
- Create: `crates/telepair-agent/src/pty.rs`
- Modify: `crates/telepair-agent/src/lib.rs`
- Create: `crates/telepair-agent/tests/pty_test.rs`

**Depends on:** Tasks 2, 3

Note: `portable-pty` requires `unsafe` internally but our code uses it through its safe API. The `#![deny(unsafe_code)]` in telepair-agent is fine since we don't write unsafe ourselves.

- [ ] **Step 1: Write PTY tests**

```rust
// crates/telepair-agent/tests/pty_test.rs
use telepair_agent::pty::PtyManager;
use tokio::time::{timeout, Duration};

#[tokio::test]
async fn spawn_shell_and_read_output() {
    let mut pty = PtyManager::spawn_shell(80, 24).unwrap();

    // Write a command
    pty.write(b"echo HELLO_TELEPAIR\n").await.unwrap();

    // Read output until we see our marker
    let output = timeout(Duration::from_secs(5), async {
        let mut all_output = Vec::new();
        loop {
            if let Some(data) = pty.read().await {
                all_output.extend_from_slice(&data);
                let text = String::from_utf8_lossy(&all_output);
                if text.contains("HELLO_TELEPAIR") {
                    return text.to_string();
                }
            }
        }
    })
    .await
    .expect("timed out waiting for output");

    assert!(output.contains("HELLO_TELEPAIR"));
    pty.shutdown();
}

#[tokio::test]
async fn spawn_command() {
    let mut pty = PtyManager::spawn_command("echo", &["PTY_TEST"], 80, 24).unwrap();

    let output = timeout(Duration::from_secs(3), async {
        let mut all = Vec::new();
        loop {
            match pty.read().await {
                Some(data) => {
                    all.extend_from_slice(&data);
                    let text = String::from_utf8_lossy(&all);
                    if text.contains("PTY_TEST") {
                        return text.to_string();
                    }
                }
                None => return String::from_utf8_lossy(&all).to_string(),
            }
        }
    })
    .await
    .expect("timed out");

    assert!(output.contains("PTY_TEST"));
}

#[tokio::test]
async fn resize_pty() {
    let mut pty = PtyManager::spawn_shell(80, 24).unwrap();
    // Should not panic
    pty.resize(120, 40).unwrap();
    pty.shutdown();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-agent --test pty_test 2>&1 | cat`
Expected: FAIL — module `pty` not found

- [ ] **Step 3: Implement pty.rs**

```rust
// crates/telepair-agent/src/pty.rs
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem, MasterPty, Child};
use std::io::{Read, Write};
use tokio::sync::mpsc;
use tokio::task;

pub struct PtyManager {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
}

impl PtyManager {
    pub fn spawn_shell(cols: u16, rows: u16) -> std::io::Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        Self::spawn_command(&shell, &[], cols, rows)
    }

    pub fn spawn_command(command: &str, args: &[&str], cols: u16, rows: u16) -> std::io::Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(*arg);
        }

        let child = pair.slave
            .spawn_command(cmd)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        // Drop the slave side — we only use master
        drop(pair.slave);

        let writer = pair.master
            .take_writer()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let mut reader = pair.master
            .try_clone_reader()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(256);

        // Spawn blocking reader thread
        task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self { master: pair.master, child, output_rx, writer })
    }

    pub async fn read(&mut self) -> Option<Vec<u8>> {
        self.output_rx.recv().await
    }

    pub async fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.master
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn is_alive(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}
```

- [ ] **Step 4: Update lib.rs**

```rust
// crates/telepair-agent/src/lib.rs
#![deny(unsafe_code)]

pub mod pty;
```

- [ ] **Step 5: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-agent --test pty_test 2>&1 | cat`
Expected: all 3 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/telepair-agent/
git commit -s -m "feat(agent): add PTY manager with spawn, IO, and resize"
```

---

### Task 8: telepair-agent — Virtual Target Engine

**Files:**
- Create: `crates/telepair-agent/src/virtual_target.rs`
- Modify: `crates/telepair-agent/src/lib.rs`
- Create: `crates/telepair-agent/tests/virtual_target_test.rs`

**Depends on:** Tasks 2, 3

- [ ] **Step 1: Write virtual target tests**

```rust
// crates/telepair-agent/tests/virtual_target_test.rs
use telepair_agent::virtual_target::TargetEngine;
use telepair_core::target::TargetConfig;

const TEST_CONFIG: &str = r#"
targets:
  - name: test-echo
    display: "Test Echo"
    command: echo
    args: ["hello", "world"]
    tags: [test]
    required_role: viewer

  - name: local-shell
    display: "Local Shell"
    type: local
"#;

#[test]
fn load_targets_from_yaml() {
    let engine = TargetEngine::from_yaml(TEST_CONFIG).unwrap();
    let targets = engine.list_targets();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].name, "test-echo");
    assert_eq!(targets[1].name, "local-shell");
}

#[test]
fn resolve_virtual_target() {
    let engine = TargetEngine::from_yaml(TEST_CONFIG).unwrap();
    let (cmd, args) = engine.resolve("test-echo").unwrap();
    assert_eq!(cmd, "echo");
    assert_eq!(args, vec!["hello", "world"]);
}

#[test]
fn resolve_local_shell() {
    let engine = TargetEngine::from_yaml(TEST_CONFIG).unwrap();
    let (cmd, args) = engine.resolve("local-shell").unwrap();
    // Should resolve to $SHELL or /bin/sh
    assert!(!cmd.is_empty());
    assert!(args.is_empty());
}

#[test]
fn unknown_target_returns_none() {
    let engine = TargetEngine::from_yaml(TEST_CONFIG).unwrap();
    assert!(engine.resolve("nonexistent").is_none());
}

#[test]
fn default_local_shell_always_present() {
    let engine = TargetEngine::empty();
    let targets = engine.list_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "local-shell");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-agent --test virtual_target_test 2>&1 | cat`
Expected: FAIL

- [ ] **Step 3: Implement virtual_target.rs**

```rust
// crates/telepair-agent/src/virtual_target.rs
use telepair_core::target::{substitute_env_vars, Target, TargetConfig, TargetKind};

pub struct TargetEngine {
    targets: Vec<Target>,
}

impl TargetEngine {
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        let mut config: TargetConfig = serde_yaml::from_str(yaml)?;
        // Ensure local-shell exists
        if !config.targets.iter().any(|t| t.kind == TargetKind::Local) {
            config.targets.push(default_local_shell());
        }
        Ok(Self { targets: config.targets })
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::from_yaml(&content)?)
    }

    pub fn empty() -> Self {
        Self { targets: vec![default_local_shell()] }
    }

    pub fn list_targets(&self) -> &[Target] {
        &self.targets
    }

    /// Resolve a target name to (command, args) with env substitution applied.
    pub fn resolve(&self, name: &str) -> Option<(String, Vec<String>)> {
        let target = self.targets.iter().find(|t| t.name == name)?;
        match target.kind {
            TargetKind::Local => {
                let shell = target.shell.as_deref()
                    .map(|s| substitute_env_vars(s))
                    .unwrap_or_else(|| {
                        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
                    });
                Some((shell, vec![]))
            }
            TargetKind::Virtual => {
                let cmd = substitute_env_vars(target.command.as_deref()?);
                let args: Vec<String> = target.args.iter()
                    .map(|a| substitute_env_vars(a))
                    .collect();
                // Set env vars (side effect for PTY spawn)
                for (k, v) in &target.env {
                    std::env::set_var(k, substitute_env_vars(v));
                }
                Some((cmd, args))
            }
        }
    }
}

fn default_local_shell() -> Target {
    Target {
        name: "local-shell".into(),
        display: "Local Shell".into(),
        kind: TargetKind::Local,
        command: None,
        args: vec![],
        env: Default::default(),
        tags: vec![],
        required_role: None,
        shell: None,
    }
}
```

- [ ] **Step 4: Update lib.rs**

Add `pub mod virtual_target;` to `crates/telepair-agent/src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-agent --test virtual_target_test 2>&1 | cat`
Expected: all 5 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/telepair-agent/
git commit -s -m "feat(agent): add virtual target engine with YAML config"
```

---

### Task 9: telepair-control — Business Logic Services

**Files:**
- Create: `crates/telepair-control/src/session_service.rs`
- Create: `crates/telepair-control/src/target_service.rs`
- Modify: `crates/telepair-control/src/lib.rs`
- Create: `crates/telepair-control/tests/session_service_test.rs`

**Depends on:** Tasks 5, 6, 8

- [ ] **Step 1: Write session service tests**

```rust
// crates/telepair-control/tests/session_service_test.rs
use std::sync::Arc;
use telepair_control::session_service::SessionService;
use telepair_core::session::InputMode;
use telepair_core::storage::SqliteStorage;

async fn setup() -> (SessionService, String) {
    let store = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, token) = store.create_user("owner", false).await.unwrap();
    let svc = SessionService::new(store);
    (svc, token)
}

#[tokio::test]
async fn create_session_adds_owner_as_participant() {
    let (svc, token) = setup().await;
    let user = svc.storage().validate_token(&token).await.unwrap();
    let session = svc.create_session(user.id, "local-shell", InputMode::Serialized).await.unwrap();

    let participants = svc.storage().list_participants(&session.id).await.unwrap();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0].role, telepair_core::permission::Role::Owner);
}

#[tokio::test]
async fn close_session_updates_status() {
    let (svc, token) = setup().await;
    let user = svc.storage().validate_token(&token).await.unwrap();
    let session = svc.create_session(user.id, "shell", InputMode::Serialized).await.unwrap();

    svc.close_session(&session.id).await.unwrap();
    let fetched = svc.storage().get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(fetched.status, telepair_core::session::SessionStatus::Closed);
}

#[tokio::test]
async fn list_active_sessions() {
    let (svc, token) = setup().await;
    let user = svc.storage().validate_token(&token).await.unwrap();
    svc.create_session(user.id, "s1", InputMode::Serialized).await.unwrap();
    svc.create_session(user.id, "s2", InputMode::Multiplexed).await.unwrap();

    let sessions = svc.list_active_sessions().await.unwrap();
    assert_eq!(sessions.len(), 2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-control --test session_service_test 2>&1 | cat`
Expected: FAIL

- [ ] **Step 3: Implement session_service.rs**

```rust
// crates/telepair-control/src/session_service.rs
use std::sync::Arc;
use uuid::Uuid;

use telepair_core::error::Result;
use telepair_core::permission::Role;
use telepair_core::session::{InputMode, Session};
use telepair_core::storage::{SqliteStorage, Storage};

pub struct SessionService {
    storage: Arc<SqliteStorage>,
}

impl SessionService {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    pub fn storage(&self) -> &SqliteStorage {
        &self.storage
    }

    pub async fn create_session(
        &self,
        owner_id: Uuid,
        target_name: &str,
        input_mode: InputMode,
    ) -> Result<Session> {
        let session = self.storage.create_session(owner_id, target_name, input_mode).await?;
        self.storage.add_participant(&session.id, owner_id, Role::Owner).await?;
        Ok(session)
    }

    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        self.storage.close_session(session_id).await
    }

    pub async fn list_active_sessions(&self) -> Result<Vec<Session>> {
        self.storage.list_active_sessions().await
    }
}
```

- [ ] **Step 4: Implement target_service.rs**

```rust
// crates/telepair-control/src/target_service.rs
use telepair_core::target::Target;
use telepair_agent::virtual_target::TargetEngine;

pub struct TargetService {
    engine: TargetEngine,
}

impl TargetService {
    pub fn new(engine: TargetEngine) -> Self {
        Self { engine }
    }

    pub fn list_targets(&self) -> &[Target] {
        self.engine.list_targets()
    }

    pub fn resolve(&self, name: &str) -> Option<(String, Vec<String>)> {
        self.engine.resolve(name)
    }
}
```

- [ ] **Step 5: Update lib.rs**

```rust
// crates/telepair-control/src/lib.rs
#![deny(unsafe_code)]

pub mod session_service;
pub mod target_service;
```

Also add `telepair-agent = { workspace = true }` to `crates/telepair-control/Cargo.toml` dependencies (needed for TargetService).

- [ ] **Step 6: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-control 2>&1 | cat`
Expected: all 3 tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/telepair-control/
git commit -s -m "feat(control): add session and target services"
```

---

### Task 10: telepair-gateway — HTTP REST Routes

**Files:**
- Create: `crates/telepair-gateway/src/http.rs`
- Create: `crates/telepair-gateway/src/state.rs`
- Modify: `crates/telepair-gateway/src/lib.rs`
- Create: `crates/telepair-gateway/tests/http_test.rs`

**Depends on:** Tasks 4, 9

- [ ] **Step 1: Write HTTP route tests**

```rust
// crates/telepair-gateway/tests/http_test.rs
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for oneshot
use telepair_gateway::build_router;
use telepair_gateway::state::AppState;

async fn setup() -> (axum::Router, String) {
    let state = AppState::new_test().await;
    let token = state.create_test_user("tester").await;
    let router = build_router(state);
    (router, token)
}

#[tokio::test]
async fn health_check() {
    let (app, _) = setup().await;
    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn list_targets_requires_auth() {
    let (app, _) = setup().await;
    let resp = app
        .oneshot(Request::get("/api/targets").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_targets_with_auth() {
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/targets")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn create_session_and_list() {
    let (app, token) = setup().await;
    // Create session
    let resp = app.clone()
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"local-shell"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // List sessions
    let resp = app
        .oneshot(
            Request::get("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
```

Note: Add `tower = { version = "0.5", features = ["util"] }` to `[workspace.dependencies]` in root `Cargo.toml`, and `tower = { workspace = true }` to `[dev-dependencies]` in `crates/telepair-gateway/Cargo.toml` (needed for `ServiceExt::oneshot` in tests).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-gateway --test http_test 2>&1 | cat`
Expected: FAIL

- [ ] **Step 3: Implement state.rs (shared application state)**

```rust
// crates/telepair-gateway/src/state.rs
use std::sync::Arc;
use telepair_agent::virtual_target::TargetEngine;
use telepair_control::session_service::SessionService;
use telepair_control::target_service::TargetService;
use telepair_core::auth::TokenAuthProvider;
use telepair_core::storage::SqliteStorage;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<TokenAuthProvider>,
    pub sessions: Arc<SessionService>,
    pub targets: Arc<TargetService>,
    pub storage: Arc<SqliteStorage>,
}

impl AppState {
    pub async fn new(storage: Arc<SqliteStorage>, engine: TargetEngine) -> Self {
        let auth = Arc::new(TokenAuthProvider::new(storage.clone()));
        let sessions = Arc::new(SessionService::new(storage.clone()));
        let targets = Arc::new(TargetService::new(engine));
        Self { auth, sessions, targets, storage }
    }

    pub async fn new_test() -> Self {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let engine = TargetEngine::empty();
        Self::new(storage, engine).await
    }

    pub async fn create_test_user(&self, name: &str) -> String {
        let (_, token) = self.storage.create_user(name, false).await.unwrap();
        token
    }
}
```

- [ ] **Step 4: Implement http.rs**

```rust
// crates/telepair-gateway/src/http.rs
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use telepair_core::session::{InputMode, Session, User};
use telepair_core::target::Target;

use crate::state::AppState;

// --- Auth extraction ---

pub async fn extract_user(state: &AppState, headers: &HeaderMap) -> Result<User, StatusCode> {
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    state.auth.validate(token).await.map_err(|_| StatusCode::UNAUTHORIZED)
}

// --- Handlers ---

pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

pub async fn list_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    let _user = extract_user(&state, &headers).await?;

    #[derive(Serialize)]
    struct TargetInfo { name: String, display: String, tags: Vec<String> }

    let targets: Vec<TargetInfo> = state.targets.list_targets().iter().map(|t| {
        TargetInfo { name: t.name.clone(), display: t.display.clone(), tags: t.tags.clone() }
    }).collect();

    Ok(Json(targets))
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub target_name: String,
    #[serde(default)]
    pub input_mode: Option<String>,
}

pub async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateSessionRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = extract_user(&state, &headers).await?;

    // Verify target exists
    if state.targets.resolve(&body.target_name).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    let mode = match body.input_mode.as_deref() {
        Some("multiplexed") => InputMode::Multiplexed,
        _ => InputMode::Serialized,
    };

    let session = state.sessions
        .create_session(user.id, &body.target_name, mode)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::CREATED, Json(session)))
}

pub async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    let _user = extract_user(&state, &headers).await?;
    let sessions = state.sessions
        .list_active_sessions()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(sessions))
}
```

- [ ] **Step 5: Update lib.rs with router builder**

```rust
// crates/telepair-gateway/src/lib.rs
#![deny(unsafe_code)]

pub mod http;
pub mod state;

use axum::{routing::{get, post}, Router};
use state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(http::health))
        .route("/api/targets", get(http::list_targets))
        .route("/api/sessions", post(http::create_session).get(http::list_sessions))
        .with_state(state)
}
```

- [ ] **Step 6: Run tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test -p telepair-gateway --test http_test 2>&1 | cat`
Expected: all 4 tests PASS

- [ ] **Step 7: Commit**

```bash
git add crates/telepair-gateway/
git commit -s -m "feat(gateway): add REST API routes for health, targets, and sessions"
```

---

### Task 11: telepair-gateway — WebSocket & Session Hub

**Files:**
- Create: `crates/telepair-gateway/src/ws.rs`
- Create: `crates/telepair-gateway/src/session_hub.rs`
- Modify: `crates/telepair-gateway/src/lib.rs`

**Depends on:** Task 10

This is the most complex component — it bridges WebSocket clients to PTY processes. For v1 (Plan 1), this handles single-user terminal sessions. Multi-user collaboration is Plan 3.

- [ ] **Step 1: Implement session_hub.rs**

The session hub manages active terminal sessions: spawns PTY, bridges I/O, tracks connected clients.

```rust
// crates/telepair-gateway/src/session_hub.rs
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

use telepair_agent::pty::PtyManager;

/// A running terminal session with PTY and broadcast channel.
struct LiveSession {
    /// Send terminal input to PTY
    input_tx: mpsc::Sender<Vec<u8>>,
    /// Subscribe to PTY output
    output_tx: broadcast::Sender<Vec<u8>>,
    /// Send resize commands
    resize_tx: mpsc::Sender<(u16, u16)>,
}

pub struct SessionHub {
    sessions: Arc<RwLock<HashMap<String, LiveSession>>>,
}

impl SessionHub {
    pub fn new() -> Self {
        Self { sessions: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// Spawn a PTY for a session. Returns channels for I/O.
    pub async fn start_session(
        &self,
        session_id: &str,
        command: &str,
        args: &[String],
        cols: u16,
        rows: u16,
    ) -> Result<(mpsc::Sender<Vec<u8>>, broadcast::Receiver<Vec<u8>>, mpsc::Sender<(u16, u16)>), String> {
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let mut pty = PtyManager::spawn_command(command, &args_ref, cols, rows)
            .map_err(|e| e.to_string())?;

        let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(256);
        let (output_tx, output_rx) = broadcast::channel::<Vec<u8>>(256);
        let (resize_tx, mut resize_rx) = mpsc::channel::<(u16, u16)>(16);

        let output_tx_clone = output_tx.clone();
        let session_id_owned = session_id.to_string();
        let sessions = self.sessions.clone();

        // PTY I/O loop
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // PTY output -> broadcast to clients
                    data = pty.read() => {
                        match data {
                            Some(bytes) => {
                                let _ = output_tx_clone.send(bytes);
                            }
                            None => {
                                // PTY closed
                                tracing::info!(session = %session_id_owned, "PTY process exited");
                                break;
                            }
                        }
                    }
                    // Client input -> PTY
                    Some(input) = input_rx.recv() => {
                        if pty.write(&input).await.is_err() {
                            break;
                        }
                    }
                    // Resize
                    Some((cols, rows)) = resize_rx.recv() => {
                        let _ = pty.resize(cols, rows);
                    }
                }
            }
            // Cleanup
            sessions.write().await.remove(&session_id_owned);
        });

        let live = LiveSession {
            input_tx: input_tx.clone(),
            output_tx: output_tx.clone(),
            resize_tx: resize_tx.clone(),
        };
        self.sessions.write().await.insert(session_id.to_string(), live);

        Ok((input_tx, output_rx, resize_tx))
    }

    /// Join an existing live session (get broadcast receiver + input sender).
    pub async fn join_session(
        &self,
        session_id: &str,
    ) -> Option<(mpsc::Sender<Vec<u8>>, broadcast::Receiver<Vec<u8>>, mpsc::Sender<(u16, u16)>)> {
        let sessions = self.sessions.read().await;
        let live = sessions.get(session_id)?;
        Some((live.input_tx.clone(), live.output_tx.subscribe(), live.resize_tx.clone()))
    }

    pub async fn is_live(&self, session_id: &str) -> bool {
        self.sessions.read().await.contains_key(session_id)
    }
}
```

- [ ] **Step 2: Implement ws.rs**

```rust
// crates/telepair-gateway/src/ws.rs
use axum::{
    extract::{ws::{Message, WebSocket}, State, WebSocketUpgrade, Path},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};

use telepair_core::protocol::{ClientMessage, ServerMessage};

use crate::state::AppState;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, session_id, state))
}

async fn handle_socket(socket: WebSocket, session_id: String, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Wait for SessionJoin message with auth token
    let user = match ws_rx.next().await {
        Some(Ok(Message::Text(text))) => {
            match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::SessionJoin { token, .. }) => {
                    match state.auth.validate(&token).await {
                        Ok(user) => user,
                        Err(_) => {
                            let err = ServerMessage::Error {
                                code: "AUTH_FAILED".into(),
                                message: "invalid token".into(),
                            };
                            let _ = ws_tx.send(Message::Text(serde_json::to_string(&err).unwrap().into())).await;
                            return;
                        }
                    }
                }
                _ => return,
            }
        }
        _ => return,
    };

    // Check if session exists in DB
    let session = match state.sessions.storage().get_session(&session_id).await {
        Ok(Some(s)) => s,
        _ => {
            let err = ServerMessage::Error {
                code: "SESSION_NOT_FOUND".into(),
                message: format!("session {session_id} not found"),
            };
            let _ = ws_tx.send(Message::Text(serde_json::to_string(&err).unwrap().into())).await;
            return;
        }
    };

    // Start or join the live PTY session
    let hub = &state.hub;
    let (input_tx, mut output_rx, resize_tx) = if hub.is_live(&session_id).await {
        match hub.join_session(&session_id).await {
            Some(channels) => channels,
            None => return,
        }
    } else {
        // Resolve target and spawn PTY
        let (cmd, args) = match state.targets.resolve(&session.target_name) {
            Some(resolved) => resolved,
            None => return,
        };
        match hub.start_session(&session_id, &cmd, &args, 80, 24).await {
            Ok(channels) => channels,
            Err(_) => return,
        }
    };

    // Send session state
    let state_msg = ServerMessage::SessionState {
        session: session.clone(),
        participants: vec![],
        your_role: telepair_core::permission::Role::Owner,
    };
    let _ = ws_tx.send(Message::Text(serde_json::to_string(&state_msg).unwrap().into())).await;

    // Spawn output forwarder: PTY output -> WebSocket
    let mut ws_tx_clone = ws_tx;
    let output_handle = tokio::spawn(async move {
        while let Ok(data) = output_rx.recv().await {
            let msg = ServerMessage::TermOutput { data };
            let json = serde_json::to_string(&msg).unwrap();
            if ws_tx_clone.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Input loop: WebSocket -> PTY
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                    match client_msg {
                        ClientMessage::TermInput { data } => {
                            let _ = input_tx.send(data).await;
                        }
                        ClientMessage::TermResize { cols, rows } => {
                            let _ = resize_tx.send((cols, rows)).await;
                        }
                        _ => {}
                    }
                }
            }
            Message::Binary(data) => {
                // Binary frame: direct PTY input
                let _ = input_tx.send(data.to_vec()).await;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    output_handle.abort();
    tracing::info!(user = %user.name, session = %session_id, "WebSocket disconnected");
}
```

- [ ] **Step 3: Update state.rs to include SessionHub**

Add the hub field:

```rust
// Add to state.rs imports:
use crate::session_hub::SessionHub;

// Add to AppState struct:
pub hub: Arc<SessionHub>,

// Update AppState::new():
let hub = Arc::new(SessionHub::new());
Self { auth, sessions, targets, storage, hub }

// Update AppState::new_test():
// Same — include hub
```

- [ ] **Step 4: Update lib.rs with WS route**

```rust
// crates/telepair-gateway/src/lib.rs
#![deny(unsafe_code)]

pub mod http;
pub mod session_hub;
pub mod state;
pub mod ws;

use axum::{routing::{get, post}, Router};
use state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(http::health))
        .route("/api/targets", get(http::list_targets))
        .route("/api/sessions", post(http::create_session).get(http::list_sessions))
        .route("/ws/session/{session_id}", get(ws::ws_handler))
        .with_state(state)
}
```

- [ ] **Step 5: Verify all existing tests still pass**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --workspace 2>&1 | cat`
Expected: all tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/telepair-gateway/
git commit -s -m "feat(gateway): add WebSocket handler and session hub for terminal I/O"
```

---

### Task 12: telepair-cli — Entry Point & Standalone Wiring

**Files:**
- Modify: `crates/telepair-cli/src/main.rs`
- Modify: `crates/telepair-cli/Cargo.toml`

**Depends on:** Tasks 10, 11

- [ ] **Step 1: Implement main.rs with composable roles**

```rust
// crates/telepair-cli/src/main.rs
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::auth::TokenAuthProvider;
use telepair_core::storage::SqliteStorage;
use telepair_gateway::state::AppState;

#[derive(Parser)]
#[command(name = "telepair", version, about = "Web terminal collaboration tool")]
struct Cli {
    /// Enable the agent role (PTY management, virtual targets)
    #[arg(long)]
    agent: bool,

    /// Enable the control role (auth, sessions, storage)
    #[arg(long)]
    control: bool,

    /// Enable the gateway role (HTTP/WS endpoints)
    #[arg(long)]
    gateway: bool,

    /// Server bind address
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    /// Server port
    #[arg(long, default_value_t = 7700)]
    port: u16,

    /// Path to config file
    #[arg(long)]
    config: Option<PathBuf>,

    /// Path to targets config file
    #[arg(long)]
    targets: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();

    // No flags = all roles enabled
    let (agent, control, gateway) = if !cli.agent && !cli.control && !cli.gateway {
        (true, true, true)
    } else {
        (cli.agent, cli.control, cli.gateway)
    };

    tracing::info!(
        agent = agent, control = control, gateway = gateway,
        "starting telepair"
    );

    // Ensure data directory exists
    let data_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".telepair");
    std::fs::create_dir_all(&data_dir)?;

    // Initialize storage (needed by control)
    let db_path = data_dir.join("telepair.db");
    let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
    let storage = Arc::new(SqliteStorage::new(&db_url).await?);

    // Initialize target engine (needed by agent)
    let engine = match &cli.targets {
        Some(path) => TargetEngine::from_file(path)
            .unwrap_or_else(|e| {
                tracing::warn!("failed to load targets from {}: {e}, using defaults", path.display());
                TargetEngine::empty()
            }),
        None => {
            let targets_path = data_dir.join("targets.yaml");
            if targets_path.exists() {
                TargetEngine::from_file(&targets_path).unwrap_or_else(|_| TargetEngine::empty())
            } else {
                TargetEngine::empty()
            }
        }
    };

    // Auto-create admin user on first run
    let auth = TokenAuthProvider::new(storage.clone());
    if storage.get_user_by_name("admin").await?.is_none() {
        let (_, token) = auth.setup_initial_admin("admin").await?;
        tracing::info!("=== First run: admin user created ===");
        tracing::info!("Admin token: {token}");
        tracing::info!("Save this token — it won't be shown again!");
    }

    if gateway {
        let state = AppState::new(storage, engine).await;
        let router = telepair_gateway::build_router(state);
        let addr = format!("{}:{}", cli.host, cli.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tracing::info!("telepair listening on http://{addr}");
        axum::serve(listener, router).await?;
    } else {
        tracing::info!("no gateway role — running headless");
        // In a future cluster mode, agent/control-only instances would
        // connect to a remote gateway here. For now, just wait.
        tokio::signal::ctrl_c().await?;
    }

    Ok(())
}
```

- [ ] **Step 2: Add missing dependencies to telepair-cli/Cargo.toml**

Add to `[dependencies]`:
```toml
anyhow = "1"
dirs = "6"
```

Also add `anyhow = "1"` and `dirs = "6"` to `[workspace.dependencies]` in the root `Cargo.toml`.

- [ ] **Step 3: Build and verify**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo build --workspace 2>&1 | cat`
Expected: compilation succeeds

- [ ] **Step 4: Run all tests**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo test --workspace 2>&1 | cat`
Expected: all tests PASS

- [ ] **Step 5: Manual smoke test**

```bash
# Start telepair in background
cd /Users/liys/workspace/github.com/telepair/telepair
cargo run -- --port 7711 &
TELEPAIR_PID=$!
sleep 2

# Check health endpoint
curl -s http://localhost:7711/api/health | cat
# Expected: {"status":"ok"}

# Get admin token from logs (or grep from stderr)
# Note: the token was printed in the startup logs

# Kill the server
kill $TELEPAIR_PID
```

- [ ] **Step 6: Run clippy**

Run: `cd /Users/liys/workspace/github.com/telepair/telepair && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | cat`
Expected: no warnings

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -s -m "feat(cli): add entry point with composable roles and standalone wiring"
```

---

## Summary

After completing all 12 tasks, you will have:

1. **A working `telepair` binary** that starts a server on port 7700
2. **REST API** at `/api/health`, `/api/targets`, `/api/sessions`
3. **WebSocket endpoint** at `/ws/session/{id}` for terminal I/O
4. **SQLite storage** for users, sessions, participants
5. **Token auth** with auto-admin creation on first run
6. **PTY management** that spawns shell processes
7. **Virtual target engine** that resolves YAML config to commands
8. **~25 tests** covering core types, storage, auth, HTTP routes

**What's NOT included (deferred to later plans):**
- Plan 2: SolidJS frontend
- Plan 3: Multi-user collaboration (cursors, chat, permissions enforcement, WebRTC)
