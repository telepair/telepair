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
    /// Nanoid of the `user_targets` row backing this session, if the
    /// session was launched from a user-owned target rather than a
    /// global (`targets.yaml`) target. The WS PTY spawn path reads
    /// this when `TargetEngine::resolve` returns `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_target_id: Option<String>,
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

/// Admin-approval state for an account. Tracked separately from
/// `session_enabled` so the admin UI can tell "waiting for approval"
/// apart from "approved but currently disabled". Introduced in v0.1.4
/// to replace the old `verified = FALSE` proxy for "pending", which
/// never matched real post-OTP rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalState {
    /// Admin-created accounts, invite-minted guests, legacy rows, and
    /// email signups that an admin has explicitly approved.
    Approved,
    /// Email signup that passed OTP verification and is waiting for an
    /// admin to flip `session_enabled = TRUE`.
    Pending,
}

impl ApprovalState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalState::Approved => "approved",
            ApprovalState::Pending => "pending",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "approved" => Some(ApprovalState::Approved),
            "pending" => Some(ApprovalState::Pending),
            _ => None,
        }
    }
}

fn default_approval_state() -> ApprovalState {
    ApprovalState::Approved
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
    /// Populated for email-registered users. `None` for token-only
    /// accounts (admin created at first run, invite-minted guests).
    /// Never serialized — not sent over the API.
    #[serde(skip)]
    pub email: Option<String>,
    /// Whether this account may create new sessions or attach to
    /// existing ones. Defaults to TRUE for admin-created accounts and
    /// invite-minted guests; the email-registration path explicitly
    /// inserts FALSE so a self-served signup is inert until an admin
    /// approves it on the user management page. The HTTP
    /// `POST /api/sessions` handler and the WS attach handshake both
    /// gate on this bit; without it, anyone with SMTP enabled could
    /// turn a public signup into shell access against the gateway
    /// host (the v0.1.2 critical adversarial finding).
    #[serde(default = "default_session_enabled")]
    pub session_enabled: bool,
    /// Admin-approval bucket. Required for the admin UI to display
    /// "Pending approval" as a first-class status separate from
    /// "Disabled". See [`ApprovalState`] for the transition rules.
    #[serde(default = "default_approval_state")]
    pub approval_state: ApprovalState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_session_enabled() -> bool {
    true
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

impl InviteToken {
    /// First 4 hex chars of `token_sha256`. Used as a short, stable,
    /// low-entropy label in audit detail blobs and the management UI.
    /// The full token is never reconstructable from this prefix; the
    /// length was cut from 8 to 4 after a QA review flagged the 8-char
    /// form as enough to partially correlate invite rows across a
    /// compromised audit log. 4 chars (16 bits) keep the label useful
    /// for humans reading audit rows for a single session while
    /// reducing cross-event correlation signal for an attacker who
    /// obtained audit logs without DB access.
    pub fn token_prefix(&self) -> &str {
        let n = self.token_sha256.len().min(4);
        // `token_sha256` is hex-encoded SHA-256, so byte indices and
        // char indices coincide — no need to walk chars.
        &self.token_sha256[..n]
    }
}

/// Who is redeeming an invite, as seen by
/// [`crate::storage::Storage::redeem_invite`]. Keeping the "existing
/// authenticated user" and "fresh scoped guest" paths in one enum
/// lets the storage layer stay honest about the fact that these two
/// flows share a single atomic transaction — previously they lived
/// in the service layer as three separate write calls with a
/// TOCTOU window and no rollback story.
#[derive(Debug, Clone, Copy)]
pub enum RedeemIdentity<'a> {
    /// Authenticated caller. `Storage::redeem_invite` assumes the row
    /// exists and looks up the current `users.name` inside the same
    /// transaction so the audit row reflects the authoritative
    /// display name even if the caller passed a stale copy.
    Existing(Uuid),
    /// Anonymous caller. `Storage::redeem_invite` INSERTs a scoped
    /// guest row with `scoped_session_id = <invite.session_id>` and
    /// returns the raw bearer token exactly once (the DB only stores
    /// its SHA-256 digest). On a UNIQUE(name) collision the caller
    /// should retry with a new name; the rolled-back transaction
    /// guarantees the invite's `used_count` is not drained.
    NewGuest { name: &'a str },
}

/// A virtual target owned by an individual user, stored in `user_targets`.
/// These are created via the Web UI and merged with global targets from
/// `targets.yaml` at list time. The `source` field in the API response
/// distinguishes them (`"user"` vs `"global"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTarget {
    pub id: String,
    pub user_id: Uuid,
    pub name: String,
    pub display: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Params for creating a user-owned target.
#[derive(Debug, Clone)]
pub struct CreateUserTargetParams {
    pub user_id: Uuid,
    pub name: String,
    pub display: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub tags: Vec<String>,
}

/// Result of a [`crate::storage::Storage::verify_pending_registration`]
/// call. Folds the OTP check, the locked-row signal, and the
/// "consume + materialize" success step into a single atomic outcome
/// so the auth service does not have to coordinate three separate
/// transactions.
///
/// `Expired` deliberately collapses both "no pending row exists" and
/// "row exists but OTP has elapsed" — the public API must not let an
/// unauthenticated caller distinguish "this email never registered"
/// from "this email registered and the OTP timed out". The internal
/// audit log still records the precise reason via
/// `auth.login_failed`-style detail blobs.
#[derive(Debug)]
pub enum PendingVerifyResult {
    /// Code matched. The pending row was deleted, the user row was
    /// inserted with `verified = TRUE` and `session_enabled = FALSE`,
    /// and `raw_token` is the freshly minted bearer the auth service
    /// must return to the client exactly once.
    Success { user: User, raw_token: String },
    /// Code did not match. `remaining` counts down from 4 to 0; on
    /// the next failure the row transitions to `Locked`.
    Failure { remaining: u32 },
    /// Five consecutive wrong codes on the same pending row — the
    /// row is now locked. The user must re-register from scratch
    /// (which deletes the locked row via the upsert path).
    Locked,
    /// No matching pending row, or the OTP has expired.
    Expired,
}

/// Result of a `Storage::record_login_failure` call. Mirrors the OTP
/// 5-strike pattern but operates on the `users` row directly: there is
/// only ever one counter per user, not one per row.
#[derive(Debug, PartialEq, Eq)]
pub enum LoginFailureOutcome {
    /// Counter incremented but the threshold has not been reached.
    /// `remaining` counts down to zero — when it hits zero on the next
    /// failure, the row transitions to `Locked`.
    Recorded { remaining: u32 },
    /// Threshold reached: the row now has `login_locked_until` set
    /// and subsequent login attempts are rejected until that time.
    Locked { until: DateTime<Utc> },
}

/// Return value of [`crate::storage::Storage::redeem_invite`]. Carries
/// the consumed invite row (post-increment), the resolved user, and —
/// for the `NewGuest` path — the raw bearer the caller must return
/// to the client exactly once.
#[derive(Debug, Clone)]
pub struct RedeemOutcome {
    /// The invite row. For a fresh join this is the post-increment
    /// state (`used_count += 1`); for the idempotent
    /// `was_already_member` short path the row is NOT bumped and
    /// this reflects the pre-call state. Callers use this for audit
    /// labelling (token prefix, role, session id) without a second
    /// round trip.
    pub invite: InviteToken,
    pub user_id: Uuid,
    pub user_name: String,
    /// `Some(raw)` iff a scoped guest was minted inside the same
    /// transaction. `None` when an authenticated caller joined under
    /// their existing identity.
    pub issued_token: Option<String>,
    /// `true` iff the caller was already an active participant of
    /// the invite's session when `redeem_invite` ran — the
    /// Existing-identity branch took its idempotent short path,
    /// leaving `used_count` untouched. Service-layer callers use
    /// this flag to skip audit writes so a race-losing double-click
    /// doesn't log a ghost `InviteRedeemed` + `ParticipantJoined`
    /// pair. Always `false` for the `NewGuest` path, which cannot
    /// race with itself.
    pub was_already_member: bool,
}
