pub mod sqlite;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::audit::{AuditEvent, AuditFilter};
use crate::error::Result;
use crate::permission::Role;
use crate::session::{
    CloseReason, InputMode, InviteToken, Participant, Session, SessionListFilter, User,
};

pub use sqlite::SqliteStorage;

#[allow(async_fn_in_trait)] // We only use SqliteStorage concretely, not dyn Storage
pub trait Storage: Send + Sync {
    // Users
    async fn create_user(&self, name: &str, is_admin: bool) -> Result<(User, String)>;
    /// Create a non-admin user whose credentials are bound to a single
    /// session. The resulting `User.scoped_session_id` will be
    /// `Some(session_id)`, which the HTTP and WS layers use to reject
    /// account-level access and cross-session WS joins. Backs the
    /// invite-redeem guest flow.
    async fn create_scoped_guest(&self, name: &str, session_id: &str) -> Result<(User, String)>;
    async fn get_user_by_name(&self, name: &str) -> Result<Option<User>>;
    async fn validate_token(&self, token: &str) -> Result<User>;

    // Sessions
    /// Atomically create a session row and insert the owner as its first
    /// participant. This must be a single transaction — a crash or error
    /// between the two statements would otherwise leave the session
    /// without its owner participant, making the owner appear unable to
    /// join their own session.
    async fn create_session_with_owner(
        &self,
        owner_id: Uuid,
        target_name: &str,
        input_mode: InputMode,
    ) -> Result<Session>;
    async fn get_session(&self, id: &str) -> Result<Option<Session>>;
    /// Close a session and stamp `reason` into the `closed_reason`
    /// column. Callers pass the semantic reason they're closing for
    /// (owner click, reaper timeout, …) so the history view can
    /// render a meaningful chip without grepping logs.
    async fn close_session(&self, id: &str, reason: CloseReason) -> Result<()>;
    /// List sessions the user owns or has participated in, filtered
    /// by `filter`. Unlike the pre-0.1.1 version, this no longer
    /// hides sessions the user has left — history needs to show
    /// "you were in this closed session" rows too. Use
    /// `SessionListFilter::active_only()` to reproduce the old
    /// "currently in" behaviour.
    async fn list_sessions_for_user(
        &self,
        user_id: Uuid,
        filter: SessionListFilter,
    ) -> Result<Vec<Session>>;
    /// Close every still-active session in one transaction. Used by
    /// the startup cleanup path after an unclean shutdown; passes
    /// `CloseReason::Startup` so the history view shows the right
    /// chip on rows the server torn down.
    async fn close_stale_sessions(&self, reason: CloseReason) -> Result<u64>;
    /// Count how many currently-active sessions exist per target
    /// name. Returns a map keyed by `Session.target_name` with `0`
    /// rows omitted — callers looking up an unmentioned target
    /// should treat the absence as zero. Backs the admin targets
    /// page so each card can show "N active sessions" without
    /// round-tripping the session list. Cheap: one indexed GROUP BY.
    async fn count_active_sessions_per_target(&self) -> Result<HashMap<String, u32>>;

    // Participants
    async fn upsert_participant(
        &self,
        session_id: &str,
        user_id: Uuid,
        role: Role,
    ) -> Result<Participant>;
    async fn list_participants(&self, session_id: &str) -> Result<Vec<Participant>>;

    // Invite Tokens
    async fn create_invite(
        &self,
        session_id: &str,
        role: Role,
        max_uses: i32,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(InviteToken, String)>;
    /// Look up an invite by its raw token without decrementing any
    /// counters. Useful for pre-validation (e.g. "is the target
    /// session still active?") before calling `consume_invite`.
    /// Does NOT check expiry / max_uses — callers that want to
    /// enforce those should call `consume_invite`.
    async fn find_invite(&self, token: &str) -> Result<InviteToken>;
    async fn consume_invite(&self, token: &str) -> Result<InviteToken>;
    /// List every invite row for a session, newest-first. Returned
    /// rows still carry `token_sha256` (not the raw token) — the
    /// raw bearer is only ever visible at mint time.
    async fn list_invites_for_session(&self, session_id: &str) -> Result<Vec<InviteToken>>;
    /// Hard-delete an invite row by its SHA-256 PK. Returns
    /// `Error::InvalidInput` if the row does not exist so callers
    /// can distinguish "already gone" (→ 404) from "real error".
    async fn revoke_invite(&self, token_sha256: &str) -> Result<()>;

    // Audit log
    /// Append a single [`AuditEvent`] to the `audit_events` table.
    /// Returns the autoincrement id of the new row. Callers should
    /// normally go through [`crate::audit::AuditSink::record`],
    /// which wraps this with "log-and-swallow" semantics — this
    /// method exists as the low-level primitive for tests and for
    /// the sink wrapper.
    async fn insert_audit_event(&self, event: &AuditEvent) -> Result<i64>;
    /// Query `audit_events` with the given filter. Rows are sorted
    /// newest-first. An unset `filter.limit` falls back to `100`.
    async fn list_audit_events(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>>;
}
