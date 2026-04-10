use crate::permission::Role;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputMode {
    Serialized,
    Multiplexed,
}

impl InputMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Serialized => "serialized",
            Self::Multiplexed => "multiplexed",
        }
    }
}

impl std::str::FromStr for InputMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "serialized" => Ok(Self::Serialized),
            "multiplexed" => Ok(Self::Multiplexed),
            _ => Err(format!("unknown input mode: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Closed,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Closed => "closed",
        }
    }
}

impl std::str::FromStr for SessionStatus {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "closed" => Ok(Self::Closed),
            _ => Err(format!("unknown session status: {s}")),
        }
    }
}

/// Why a session transitioned from `active` to `closed`. Stored in the
/// `sessions.closed_reason` column as a lowercase discriminant and
/// surfaced to the history UI so users can distinguish "I closed it"
/// from "the reaper timed it out" from "the server was restarted".
///
/// Nullable on the row because v0.1.0 closed rows predate the column;
/// they read back as `None` and render as "unknown" in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloseReason {
    /// Owner clicked Close in the UI (or hit the DELETE endpoint).
    Owner,
    /// Idle reaper ran past the grace window and reclaimed the PTY.
    Reaper,
    /// Boot-time cleanup closed an orphaned `active` row left over
    /// from an unclean shutdown.
    Startup,
    /// Unexpected server-side error tore the session down.
    Error,
}

impl CloseReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Reaper => "reaper",
            Self::Startup => "startup",
            Self::Error => "error",
        }
    }
}

impl std::str::FromStr for CloseReason {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "owner" => Ok(Self::Owner),
            "reaper" => Ok(Self::Reaper),
            "startup" => Ok(Self::Startup),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown close reason: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub owner_id: Uuid,
    pub target_name: String,
    pub input_mode: InputMode,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    /// Why the session was closed. Populated by the service layer on
    /// the close path; `None` for still-active rows and for legacy
    /// v0.1.0 closed rows created before the column existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_reason: Option<CloseReason>,
}

/// Filter criteria for `Storage::list_sessions_for_user` and the
/// `GET /api/sessions` endpoint it backs. Built up from query-string
/// params in the HTTP layer; `Default` = "every session visible to
/// the caller, no bounds", which is what the history view wants.
///
/// This exists so the history view (closed + active, optionally
/// scoped to a target, paginated) and the legacy "what am I in right
/// now" call (active-only) can share one storage method.
#[derive(Debug, Clone, Default)]
pub struct SessionListFilter {
    /// `None` = include both statuses. `Some(Active)` reproduces the
    /// pre-0.1.1 default behaviour; `Some(Closed)` powers the history
    /// tab.
    pub status: Option<SessionStatus>,
    /// `Some("name")` = only sessions whose `target_name` matches,
    /// used by the "jump from admin targets list" deep link.
    pub target_name: Option<String>,
    /// `None` = no `LIMIT` clause. Storage still caps the query plan
    /// with `ORDER BY created_at DESC`, so callers that don't care
    /// get the newest first.
    pub limit: Option<i64>,
    /// `0` = no `OFFSET`.
    pub offset: i64,
}

impl SessionListFilter {
    /// Shorthand for the pre-0.1.1 behaviour: active sessions only,
    /// all targets, no pagination. Kept as a helper so the "current
    /// sessions" call sites stay readable.
    pub fn active_only() -> Self {
        Self {
            status: Some(SessionStatus::Active),
            ..Self::default()
        }
    }
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
    /// `Some(session_id)` → this user is an invite-minted guest whose
    /// credentials are scoped to exactly one session. The HTTP layer
    /// rejects every account-level route for scoped users, and the
    /// WebSocket handshake rejects connections whose path does not
    /// match. `None` → a real account created through the admin
    /// path; full access subject to the usual role checks.
    ///
    /// This is the load-bearing fix for the "invite link grants a
    /// full non-admin account" authorization bug — without it, a
    /// redeemed viewer invite would let the holder list targets and
    /// spawn new sessions behind the scenes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped_session_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    /// `true` iff the user's credentials are scoped to a single
    /// session (i.e. an invite-minted guest). Route handlers use this
    /// to gate account-level endpoints; WS uses it to match the
    /// path session id.
    pub fn is_guest(&self) -> bool {
        self.scoped_session_id.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteToken {
    /// Hex-encoded SHA-256 digest of the raw token. Doubles as the
    /// primary key in `invite_tokens` — the raw token is only ever
    /// returned to the caller at creation time.
    pub token_sha256: String,
    pub session_id: String,
    pub role: Role,
    pub max_uses: i32,
    pub used_count: i32,
    pub expires_at: Option<DateTime<Utc>>,
    /// Wall-clock time the invite row was created. Nullable because
    /// v0.1.0 rows pre-date the `created_at` column — the upgrade
    /// path adds the column nullable so old rows read as `None`.
    /// New inserts always populate it via `now_rfc3339()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}
