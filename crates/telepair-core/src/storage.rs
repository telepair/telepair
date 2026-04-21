pub mod sqlite;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::audit::{AuditEvent, AuditFilter};
use crate::error::Result;
use crate::permission::Role;
use crate::recording::{RecordingRow, RecordingShareRow};
use crate::session::{
    CloseReason, CreateUserTargetParams, InputMode, InviteToken, LoginFailureOutcome, Participant,
    PendingVerifyResult, RedeemIdentity, RedeemOutcome, Session, SessionListFilter, User,
    UserTarget,
};

pub use sqlite::SqliteStorage;

/// Filter for the admin user listing endpoint.
pub struct AccountFilter {
    /// Fuzzy match on name or email.
    pub query: Option<String>,
    /// Filter by account status.
    pub status: Option<AccountStatus>,
    pub limit: i64,
    pub offset: i64,
}

/// Account status filter for admin user management. The three buckets
/// map to a combination of `approval_state` + `session_enabled` so
/// "waiting for admin approval" is distinct from "admin explicitly
/// disabled this account":
#[derive(Debug, Clone, Copy)]
pub enum AccountStatus {
    /// `approval_state = 'approved' AND session_enabled = TRUE`
    Enabled,
    /// `approval_state = 'approved' AND session_enabled = FALSE`
    Disabled,
    /// `approval_state = 'pending'` — self-served signup that passed
    /// OTP verification but has not been approved by an admin yet.
    Pending,
}

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
    ///
    /// `user_target_id` is `Some(nanoid)` when the session was launched
    /// from a user-owned target rather than a global (targets.yaml) one.
    /// The WS PTY spawn path reads this when `TargetEngine::resolve`
    /// returns `None`.
    async fn create_session_with_owner(
        &self,
        owner_id: Uuid,
        target_name: &str,
        input_mode: InputMode,
        user_target_id: Option<&str>,
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
    /// Hard-delete an invite row by its SHA-256 PK.
    ///
    /// Returns `Ok(true)` when a row was actually deleted and
    /// `Ok(false)` when the row was already gone (or never existed).
    /// Callers use the boolean to decide whether the observable state
    /// of the system changed — the `InviteService::revoke` audit path
    /// only emits on `true` so concurrent double-clicks or retries
    /// don't write phantom audit events.
    async fn revoke_invite(&self, token_sha256: &str) -> Result<bool>;

    // Auth — email registration
    //
    // The pending-row design (v0.1.2): a self-served signup writes a
    // single row to `pending_registrations` carrying the display name,
    // password hash, and OTP. The row carries no authority — it has
    // no `users` entry and no token — so a re-register from the same
    // address can overwrite it freely without opening the
    // pre-verification takeover window the old `users.verified=FALSE`
    // path enabled. The successful OTP verify is the *only* moment a
    // `users` row materializes, and it happens in the same transaction
    // that consumes the pending row.
    //
    // Listing users + flipping `session_enabled` powers the admin
    // approval flow: a freshly verified self-signup is inert (no
    // session create / attach) until an admin enables the row.

    /// Insert or refresh the pending registration for `email`. The row
    /// is keyed by lowercased email; a re-register from the same
    /// address overwrites the previous row in place, resets the OTP
    /// failure counter to zero, and refreshes `updated_at`. Carries
    /// no authority of its own — there is no `users` row and no
    /// token until [`Self::verify_pending_registration`] consumes it.
    async fn upsert_pending_registration(
        &self,
        email: &str,
        display_name: &str,
        password_hash: &str,
        otp_code: &str,
        otp_expires_at: DateTime<Utc>,
    ) -> Result<()>;

    /// Returns the `updated_at` of the pending row for `email`, if
    /// any. Used by the auth service's 60-second OTP rate limit.
    async fn latest_pending_registration_at(&self, email: &str) -> Result<Option<DateTime<Utc>>>;

    /// Delete the pending row for `email` only if its OTP matches
    /// `otp_code`. Used by the SMTP-failure rollback path so a user
    /// whose code was never delivered is not stranded behind the rate
    /// limit on a row that no one can verify. The compare-and-delete
    /// prevents a concurrent registration from losing its valid OTP
    /// when an earlier request's SMTP send fails and rolls back.
    async fn delete_pending_registration(&self, email: &str, otp_code: &str) -> Result<()>;

    /// Atomically verify a pending registration's OTP. On
    /// [`PendingVerifyResult::Success`] the pending row is consumed
    /// AND a fresh `users` row is inserted (with `session_enabled =
    /// FALSE`, awaiting admin approval) AND a bearer token is minted,
    /// all in a single transaction. On code mismatch the failure
    /// counter advances (Failure → Locked at 5). The pending row is
    /// gated by `otp_expires_at > now` and `otp_failure_count < 5`;
    /// "no eligible row" collapses to `Expired`, identical to a
    /// missing row, so the public API cannot be used to enumerate
    /// pending addresses.
    async fn verify_pending_registration(
        &self,
        email: &str,
        code: &str,
    ) -> Result<PendingVerifyResult>;

    /// Hard-delete every pending row whose `updated_at < cutoff`.
    /// Returns the number of rows removed. Used by the (future)
    /// background sweeper to keep the table from accreting abandoned
    /// signups.
    async fn sweep_pending_registrations(&self, cutoff: DateTime<Utc>) -> Result<u64>;

    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>>;

    /// Materialize a real `users` row directly from the admin CLI,
    /// bypassing the OTP / SMTP flow. Used by
    /// `telepair admin users create` so a single-node install without
    /// SMTP can still onboard regular users. The row is written as
    /// `verified = TRUE`, `approval_state = 'approved'` (no admin
    /// review needed — the admin is the one running the CLI), with
    /// `session_enabled` under caller control. Returns the full
    /// `User` row plus the fresh raw bearer token.
    ///
    /// Errors collapse via [`Error::Conflict`] on either a display
    /// name collision or a duplicate email — the CLI surfaces the
    /// message to the operator verbatim.
    async fn admin_create_password_user(
        &self,
        email: &str,
        name: &str,
        password_hash: &str,
        is_admin: bool,
        session_enabled: bool,
    ) -> Result<(User, String)>;

    // `get_password_hash` is not on the trait — SqliteStorage exposes it
    // directly so auth-service code can read credentials without making
    // the method part of the public Storage contract.

    /// Generate a fresh nanoid token for an already-verified user and
    /// overwrite `token_sha256`. Previous tokens are immediately invalid.
    async fn refresh_user_token(&self, user_id: Uuid) -> Result<String>;

    // ── Admin user management (v0.1.2) ────────────────────────────────
    //
    // The admin approval flow needs a list of users to render and a
    // way to flip `session_enabled` per row. Both endpoints are
    // admin-gated in the HTTP layer; the storage primitives stay
    // agnostic so tests can drive them directly.

    /// List every account row, newest-first. Excludes invite-minted
    /// scoped guests (`scoped_session_id IS NOT NULL`) since those are
    /// session-local and not user-actionable on the admin page.
    async fn list_accounts(&self) -> Result<Vec<User>>;

    /// List non-guest accounts with optional filtering by name/email
    /// and status. Returns (matching rows, total count before pagination).
    async fn list_accounts_filtered(&self, filter: &AccountFilter) -> Result<(Vec<User>, i64)>;

    /// Look up a user row by id. Returns `Ok(None)` on miss so the
    /// admin enable/disable handler can render a 404 instead of a 500.
    async fn find_user_by_id(&self, user_id: Uuid) -> Result<Option<User>>;

    /// Flip `session_enabled` on a user row. Returns the post-update
    /// `User` so the caller can echo it into an audit row and the API
    /// response. Returns `Error::InvalidInput` if the row does not
    /// exist — the admin handler maps that to a 404.
    async fn set_session_enabled(&self, user_id: Uuid, enabled: bool) -> Result<User>;

    /// Returns the active password-login lockout for `user_id`, or
    /// `None` if the user is not currently locked out. The "lazy
    /// clear" semantics are deliberate: when a row's
    /// `login_locked_until` has fallen into the past this method
    /// resets `login_failed_count` and `login_locked_until` to their
    /// idle state in the same call, so the next failed attempt starts
    /// a fresh 5-strike window instead of immediately re-locking on
    /// the stale counter. The returned `Some(time)` is always strictly
    /// in the future.
    async fn check_login_lockout(&self, user_id: Uuid) -> Result<Option<DateTime<Utc>>>;

    /// Atomically increment `login_failed_count`. When the post-bump
    /// count crosses the 5-strike threshold the row is transitioned
    /// to `Locked` with `login_locked_until = now + lockout_duration`.
    /// Mirrors the OTP failure-counter semantics so the rate limit
    /// behaves consistently across both auth paths.
    async fn record_login_failure(
        &self,
        user_id: Uuid,
        lockout_duration: chrono::Duration,
    ) -> Result<LoginFailureOutcome>;

    /// Reset `login_failed_count` to zero and clear
    /// `login_locked_until`. Called on successful login so a single
    /// good password wipes out any prior bad attempts. Idempotent on
    /// rows that are already in the idle state.
    async fn clear_login_failures(&self, user_id: Uuid) -> Result<()>;

    // User-owned targets
    async fn create_user_target(&self, params: CreateUserTargetParams) -> Result<UserTarget>;
    async fn list_user_targets(&self, user_id: Uuid) -> Result<Vec<UserTarget>>;
    #[allow(clippy::too_many_arguments)]
    async fn update_user_target(
        &self,
        id: &str,
        user_id: Uuid,
        display: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        tags: &[String],
    ) -> Result<UserTarget>;
    /// Delete a user target. Returns `Error::PermissionDenied` if
    /// `user_id` does not own the target.
    async fn delete_user_target(&self, id: &str, user_id: Uuid) -> Result<()>;
    async fn find_user_target_by_id(&self, id: &str) -> Result<Option<UserTarget>>;

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

    // ── Recordings ────────────────────────────────────────────────────

    /// Create a new recording row in `recording` status. The caller
    /// supplies `id` so that the recording id, the on-disk filename,
    /// and the DB primary key are all derived from a single source —
    /// preventing the cleaner / downloader from looking up a file
    /// under one id while the row was inserted under another.
    /// `expires_at` is an optional RFC3339 string; `None` means the
    /// recording never expires (permanent).
    #[allow(clippy::too_many_arguments)] // Atomic insert, no natural sub-grouping.
    async fn create_recording(
        &self,
        id: &str,
        session_id: &str,
        created_by: Uuid,
        width: i64,
        height: i64,
        file_path: &str,
        expires_at: Option<&str>,
    ) -> Result<RecordingRow>;

    /// Look up a recording by id. Returns `Ok(None)` on miss.
    async fn get_recording(&self, id: &str) -> Result<Option<RecordingRow>>;

    /// Transition a recording to `completed` status with final
    /// duration, event count, and file size.
    async fn complete_recording(
        &self,
        id: &str,
        duration_ms: i64,
        event_count: i64,
        file_size: i64,
    ) -> Result<()>;

    /// Transition a recording to `failed` status.
    async fn fail_recording(&self, id: &str) -> Result<()>;

    /// Find the active (`status = 'recording'`) recording for a
    /// session, if any. At most one recording per session can be
    /// active at a time — the caller is responsible for enforcing
    /// this invariant at creation time.
    async fn find_active_recording(&self, session_id: &str) -> Result<Option<RecordingRow>>;

    /// List all recordings created by a specific user, newest-first.
    async fn list_recordings_for_user(&self, user_id: Uuid) -> Result<Vec<RecordingRow>>;

    /// List every recording in the system, newest-first. Admin-only
    /// gate is the caller's responsibility.
    async fn list_all_recordings(&self) -> Result<Vec<RecordingRow>>;

    /// List recordings whose `expires_at` has passed (< now), up to
    /// `limit` rows. Used by the TTL cleaner to find candidates for
    /// deletion.
    async fn list_expired_recordings(&self, limit: i64) -> Result<Vec<RecordingRow>>;

    /// Hard-delete a recording row. Cascade deletes will remove any
    /// associated `recording_shares` rows.
    async fn delete_recording(&self, id: &str) -> Result<()>;

    /// Clear `expires_at` so the recording never expires.
    async fn set_recording_permanent(&self, id: &str) -> Result<()>;

    /// Set or update `expires_at` to the given RFC3339 timestamp.
    async fn set_recording_expiry(&self, id: &str, expires_at: &str) -> Result<()>;

    // ── Recording shares ──────────────────────────────────────────────

    /// Create a share token row for a recording. `token_sha256` is the
    /// hex-encoded SHA-256 of the raw token (the raw token is only
    /// visible at mint time). `max_uses` of 0 means unlimited.
    async fn create_recording_share(
        &self,
        recording_id: &str,
        token_sha256: &str,
        max_uses: i64,
        expires_at: Option<&str>,
    ) -> Result<RecordingShareRow>;

    /// Read-only validation for a recording share token. Applies the
    /// same ownership / expiry / remaining-uses predicates as
    /// [`Self::consume_recording_share`] but does NOT increment
    /// `used_count`.
    ///
    /// Used by the recording download path to fail invalid credentials
    /// with 401 before touching the filesystem, while still delaying
    /// the actual counter burn until after the `.cast` read succeeds.
    async fn peek_recording_share(
        &self,
        token_sha256: &str,
        expected_recording_id: &str,
    ) -> Result<Option<RecordingShareRow>>;

    /// Atomically validate and consume a share token. Single SQL
    /// statement that increments `used_count` only when ALL of the
    /// following hold:
    /// - the token row exists,
    /// - it belongs to `expected_recording_id`,
    /// - it has not expired (`expires_at IS NULL OR > now`),
    /// - it has remaining uses (`max_uses = 0 OR used_count < max_uses`).
    ///
    /// Returns the post-increment row on success, `None` otherwise.
    /// Doing this in one statement closes two earlier holes:
    /// 1. TOCTOU race where two concurrent requests both pass an
    ///    application-level `used_count < max_uses` check before
    ///    either UPDATE landed and exhausted the limit;
    /// 2. caller incrementing the counter before validating that the
    ///    token belonged to the requested recording — letting any
    ///    holder of a share burn another recording's quota with a
    ///    bogus URL.
    async fn consume_recording_share(
        &self,
        token_sha256: &str,
        expected_recording_id: &str,
    ) -> Result<Option<RecordingShareRow>>;

    /// List all share tokens for a recording, newest-first.
    async fn list_recording_shares(&self, recording_id: &str) -> Result<Vec<RecordingShareRow>>;

    /// Hard-delete a share token row scoped to `recording_id`. The
    /// URL path (not the digest alone) is the authoritative scope:
    /// without the recording_id filter, any owner who happens to know
    /// a share digest — which is trivially computable from the raw
    /// token the share link already embeds — could revoke shares
    /// belonging to other recordings by hitting the delete endpoint
    /// with their own `recording_id` in the URL. Returns `true` if a
    /// matching row was deleted, `false` otherwise; callers map the
    /// `false` case to 404.
    async fn delete_recording_share(&self, recording_id: &str, token_sha256: &str) -> Result<bool>;
}
