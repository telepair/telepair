pub mod sqlite;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::audit::{AuditEvent, AuditFilter};
use crate::error::Result;
use crate::permission::Role;
use crate::session::{
    CloseReason, InputMode, InviteToken, Participant, RedeemIdentity, RedeemOutcome, Session,
    SessionListFilter, User,
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
    /// List every session in the system, filtered only by `filter`,
    /// with no ownership / participant scoping. Callers MUST enforce
    /// the admin gate themselves — this trait method does not know
    /// who is asking. `SessionService::list_sessions_visible_to`
    /// branches on `User::is_admin` and is the only intended caller
    /// on the production path.
    async fn list_all_sessions(&self, filter: SessionListFilter) -> Result<Vec<Session>>;
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
    /// Atomic existing-participant lookup, scoped to **active** sessions.
    /// Returns `Some(role)` iff *all* of the following hold in the same
    /// MVCC snapshot: the user has a participant row in the session,
    /// the row's `left_at IS NULL`, and the session's `status = 'active'`.
    /// Returns `None` if the row doesn't exist, the user has already
    /// left, or the session is closed.
    ///
    /// Backs the invite-redeem existing-member short path so a no-op
    /// "owner verifies their own share link" cannot return success
    /// against a session that was closed between the pre-check and the
    /// participant lookup. The two-query alternative
    /// (`get_session` + `list_participants`) was vulnerable to a TOCTOU
    /// race where a concurrent `close_session` could commit between
    /// the two reads, producing a "you're a member" reply against a
    /// dead session. A single SELECT-JOIN closes that gap.
    async fn find_active_participant_role(
        &self,
        session_id: &str,
        user_id: Uuid,
    ) -> Result<Option<Role>>;

    // Invite Tokens
    async fn create_invite(
        &self,
        session_id: &str,
        role: Role,
        max_uses: i32,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(InviteToken, String)>;
    /// Look up an invite by its raw token without decrementing any
    /// counters. Used by the service layer as a fast-fail preview
    /// before committing to a `redeem_invite` transaction — the
    /// authoritative expiry / max_uses / session-active gates live
    /// inside that transaction's WHERE clause, so this method is a
    /// read-only helper that does NOT check any of them.
    async fn find_invite(&self, token: &str) -> Result<InviteToken>;
    /// Look up an invite directly by its SHA-256 PK. Used by the
    /// revoke path where the caller already has the digest (it's the
    /// URL parameter) and doesn't want the `find_invite` pre-hash
    /// step. Returns `Ok(None)` on miss so the caller can distinguish
    /// "not in session" from a real DB error.
    async fn find_invite_by_sha256(&self, token_sha256: &str) -> Result<Option<InviteToken>>;
    /// Atomically consume an invite AND install the redeemer as a
    /// session participant in one transaction. Supersedes the v0.1.1
    /// split-step sequence (pre-check session active → increment
    /// `used_count` → insert guest user → upsert participant), which
    /// had two failure modes:
    ///
    /// 1. Partial failure: a transient error between the counter
    ///    increment and `upsert_participant` left `used_count`
    ///    drained with no participant row (invite silently burned).
    /// 2. TOCTOU on session close: the pre-check "session still
    ///    active" ran outside any transaction, so a concurrent
    ///    `close_session` could commit between the check and the
    ///    participant write — leaving a participant row pointing
    ///    at a closed session.
    ///
    /// This method fixes both by folding the session-status check
    /// into the invite `UPDATE` (`EXISTS (SELECT … WHERE status =
    /// 'active')`) and wrapping everything in a single transaction.
    /// Concrete errors:
    /// - [`Error::InvalidInput`] — token not found, expired, or
    ///   exhausted (`used_count >= max_uses`).
    /// - [`Error::SessionClosed`] — the invite's session was closed
    ///   before (or during) redemption.
    /// - [`Error::SessionNotFound`] — the invite points at a
    ///   deleted session row (should only happen in test fixtures).
    /// - [`Error::Storage`] — underlying driver errors, including
    ///   the UNIQUE(name) violation the caller retries on for the
    ///   `NewGuest` path.
    async fn redeem_invite(
        &self,
        token: &str,
        identity: RedeemIdentity<'_>,
    ) -> Result<RedeemOutcome>;
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
