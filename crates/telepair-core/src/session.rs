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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}
