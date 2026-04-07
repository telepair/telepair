use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::permission::Role;
use crate::session::Session;

/// Stable error codes carried by `ServerMessage::Error`. The frontend
/// switches on these strings to render localized messages and decide
/// whether to force re-login — a typo on either side silently degrades
/// UX, so the Rust and TS sides MUST stay in sync via constants, not
/// scattered string literals. Mirror table lives in `web/src/lib/protocol.ts`.
pub mod error_codes {
    pub const AUTH_FAILED: &str = "AUTH_FAILED";
    pub const AUTH_TIMEOUT: &str = "AUTH_TIMEOUT";
    pub const EXPECTED_JOIN: &str = "EXPECTED_JOIN";
    pub const SESSION_NOT_FOUND: &str = "SESSION_NOT_FOUND";
    pub const SESSION_CLOSED: &str = "SESSION_CLOSED";
    pub const NOT_PARTICIPANT: &str = "NOT_PARTICIPANT";
    pub const TARGET_NOT_FOUND: &str = "TARGET_NOT_FOUND";
    pub const PTY_ERROR: &str = "PTY_ERROR";
}

// --- Client -> Server ---
//
// Terminal input (`TermInput`) is NOT a JSON message — it is sent as a binary
// WebSocket frame carrying raw bytes. The handler at `ws.rs::handle_socket`
// reads `Message::Binary(data)` and forwards it straight to the PTY. JSON
// messages below are used for everything else (session control, resize,
// chat, cursor).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    SessionJoin {
        session_id: String,
        token: String,
        #[serde(default = "default_cols")]
        cols: u16,
        #[serde(default = "default_rows")]
        rows: u16,
    },
    TermResize {
        cols: u16,
        rows: u16,
    },
    CursorMove {
        x: u16,
        y: u16,
    },
    ChatMessage {
        text: String,
    },
}

// --- Server -> Client ---
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    SessionState {
        session: Session,
        participants: Vec<ParticipantInfo>,
        your_role: Role,
        your_user_id: Uuid,
    },
    PeerJoined {
        user_id: Uuid,
        name: String,
        role: Role,
        color: String,
    },
    PeerLeft {
        user_id: Uuid,
    },
    PeerCursor {
        user_id: Uuid,
        x: u16,
        y: u16,
    },
    PeerChat {
        user_id: Uuid,
        name: String,
        text: String,
        ts: String,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub user_id: Uuid,
    pub name: String,
    pub role: Role,
    pub color: String,
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}
