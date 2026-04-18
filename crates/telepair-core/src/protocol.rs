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
    /// Transient storage failure during the WS handshake. Clients
    /// should retry — this is NOT a permission or invite problem.
    /// Pairs with [`crate::protocol::CLOSE_CODE_TRANSIENT`] so the
    /// close frame actually conveys "retry me".
    pub const STORAGE_ERROR: &str = "STORAGE_ERROR";
    /// The caller's `session_enabled` bit is FALSE — their account
    /// was registered through the public email-signup flow and has
    /// not been approved by an admin yet. The token is still valid
    /// for reads (whoami, history), but session create / attach is
    /// closed. Pairs with the HTTP 403 that `POST /api/sessions`
    /// returns for the same reason. Terminal close, not transient —
    /// the client must route the user to a "pending approval" UI
    /// rather than retry.
    pub const SESSION_DISABLED: &str = "SESSION_DISABLED";
}

/// Terminal WebSocket close code — the client treats this as a
/// permanent refusal and surfaces an error state without reconnecting.
/// Auth / permission / not-found / target-missing all use this.
/// `web/src/lib/ws.ts` switches on `event.code` and MUST stay in sync.
pub const CLOSE_CODE_TERMINAL: u16 = 4001;

/// Transient WebSocket close code — the client is expected to retry
/// (e.g. a one-off storage hiccup during the handshake). Distinct from
/// `CLOSE_CODE_TERMINAL` so the frontend can actually tell the two
/// apart via `event.code`: the preceding `ServerMessage::Error` JSON
/// frame can get dropped if the socket tears down mid-write, so the
/// close code must stand on its own as the retry signal.
///
/// Value sits inside the private-use range (4000-4999) reserved for
/// applications, chosen to be visually distinct from 4001 rather than
/// for any IANA meaning.
pub const CLOSE_CODE_TRANSIENT: u16 = 4503;

/// Map a protocol-level error code to the WebSocket close code that
/// accompanies it. The close code is the ONLY signal the client has
/// for deciding "retry vs give up" — the preceding JSON error frame
/// may be dropped if the socket dies mid-write, so this mapping is
/// the protocol's single source of truth.
///
/// Default is terminal; transient codes must be opted into explicitly
/// so a new error code never silently becomes "silently retry" (which
/// would invite reconnect storms from revoked tokens, etc.).
pub fn close_code_for(error_code: &str) -> u16 {
    match error_code {
        error_codes::STORAGE_ERROR => CLOSE_CODE_TRANSIENT,
        _ => CLOSE_CODE_TERMINAL,
    }
}

/// Stable `reason` codes carried by `ServerMessage::InputDenied`. The
/// frontend switches on these to render localized guidance; keep in sync
/// with `web/src/lib/protocol.ts::InputDeniedReason`.
pub mod input_denied {
    /// Connection's role is `Viewer` — read-only by design.
    pub const VIEWER: &str = "VIEWER";
    /// Session is in `Serialized` mode and caller is not the owner.
    /// Operators can still resize and chat; only keystrokes are dropped.
    pub const SERIALIZED_NOT_OWNER: &str = "SERIALIZED_NOT_OWNER";
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
    ChatMessage {
        text: String,
    },
}

/// Why a participant is being force-disconnected. Carried on
/// [`ServerMessage::PeerEvicted`] so the client can split the UX
/// between "your account was just disabled" (terminal, go to
/// pending-approval) and "your bearer token rotated" (recoverable,
/// go to login). Also shapes the close-frame reason string the
/// server attaches when it tears down the evicted socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvictReason {
    /// Admin flipped `session_enabled = FALSE` (or an equivalent
    /// permanent revoke). Token is dead on every endpoint; the UI
    /// should route the user to the pending-approval page and other
    /// participants should see a "removed by an admin" notice.
    AccountDisabled,
    /// The user's own password change atomically rotated their
    /// bearer token. The session must drop because the old token is
    /// no longer accepted, but the account is in good standing —
    /// the UI should route to login/re-auth and other participants
    /// should see a neutral "re-authentication required" notice,
    /// not an admin action.
    TokenRotated,
}

/// Recording status embedded in `SessionState` so late joiners know
/// immediately whether a recording is active when they connect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingStatusInfo {
    pub recording_id: String,
    pub started_at: String,
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
        /// Bounded replay for late joiners, oldest-first. Ephemeral —
        /// dies with the session; see `session_hub::CHAT_HISTORY_CAP`.
        #[serde(default)]
        chat_history: Vec<ChatEntry>,
        /// Present when a recording is active at join time.
        #[serde(default)]
        recording: Option<RecordingStatusInfo>,
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
    /// Broadcast when a participant is force-removed from a session
    /// *without* them choosing to leave. Distinct from `PeerLeft`
    /// because the meaning for the client UI is different: other
    /// participants render a "was removed" notice and the evicted
    /// user's own WS handler treats this as a close signal (their
    /// session tab must drop immediately so the session can no
    /// longer be driven from their browser).
    ///
    /// `reason` separates the two triggers so the client can split
    /// the UX cleanly: `AccountDisabled` routes the user to the
    /// pending-approval page (their token is now invalid for every
    /// endpoint), while `TokenRotated` routes them to re-login
    /// (their account is fine, only the bearer they were holding is
    /// dead after they rotated their own password). Collaborators
    /// see a different chat string in each case so the same "was
    /// removed" affordance doesn't misattribute a routine password
    /// change as an admin action.
    PeerEvicted {
        user_id: Uuid,
        reason: EvictReason,
    },
    /// Broadcast when the owner changes another participant's role via
    /// `PUT /api/sessions/:id/participants/:user_id/role`. Every
    /// connected client receives this so participant lists update in
    /// lockstep. The WS handler also intercepts messages targeting the
    /// current connection to re-evaluate input permissions without a
    /// reconnect.
    PeerRoleChanged {
        user_id: Uuid,
        new_role: Role,
    },
    PeerChat {
        user_id: Uuid,
        name: String,
        text: String,
        ts: String,
    },
    /// Sent **only to the originating connection** (never broadcast) the
    /// first time a binary input frame is rejected for this session. The
    /// frontend uses this to surface a toast instead of leaving the user
    /// wondering why typing silently does nothing.
    InputDenied {
        /// Machine-readable reason code — see `input_denied` module.
        reason: String,
    },
    Error {
        code: String,
        message: String,
    },
    /// Broadcast to all participants when a recording starts on this session.
    RecordingStarted {
        recording_id: String,
    },
    /// Broadcast to all participants when a recording stops on this session.
    RecordingStopped {
        recording_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub user_id: Uuid,
    pub name: String,
    pub role: Role,
    pub color: String,
}

/// A single entry in the bounded chat backlog delivered inside
/// `ServerMessage::SessionState` and broadcast live as
/// `ServerMessage::PeerChat`. Fields mirror `PeerChat` 1:1 so the
/// frontend can reuse the same renderer for replayed and live messages
/// — one code path, one formatting story.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatEntry {
    pub user_id: Uuid,
    pub name: String,
    pub text: String,
    /// RFC 3339 timestamp captured at broadcast time (not at replay).
    pub ts: String,
}

impl From<ChatEntry> for ServerMessage {
    fn from(e: ChatEntry) -> Self {
        ServerMessage::PeerChat {
            user_id: e.user_id,
            name: e.name,
            text: e.text,
            ts: e.ts,
        }
    }
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}
