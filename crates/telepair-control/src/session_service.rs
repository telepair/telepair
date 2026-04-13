use std::sync::Arc;
use uuid::Uuid;

use telepair_core::audit::{AuditEvent, AuditEventType, AuditSink};
use telepair_core::error::{Error, Result};
use telepair_core::permission::Role;
use telepair_core::session::{
    CloseReason, InputMode, Participant, Session, SessionListFilter, SessionStatus, User,
};
use telepair_core::storage::{SqliteStorage, Storage};

/// Session-layer business rules: create, close, lookup, ownership
/// checks, and participant queries. The HTTP/WS layers route every
/// session-related call through this service so the gateway stays
/// pure transport + auth, with business policy living in one place.
///
/// **No `storage()` escape hatch.** Previous revisions exposed the
/// underlying `SqliteStorage` so handlers could reach around the
/// service for one-off queries; the result was that invariants like
/// "owner check must run before returning a session" were duplicated
/// inline in every handler. Every query that mattered moved onto
/// this struct; production code (`src/`) must not reach the storage
/// directly.
pub struct SessionService {
    storage: Arc<SqliteStorage>,
    audit: Arc<AuditSink>,
}

impl SessionService {
    pub fn new(storage: Arc<SqliteStorage>, audit: Arc<AuditSink>) -> Self {
        Self { storage, audit }
    }

    // -- Session lifecycle ---------------------------------------------------

    pub async fn create_session(
        &self,
        owner: &User,
        target_name: &str,
        input_mode: InputMode,
    ) -> Result<Session> {
        self.create_session_with_user_target(owner, target_name, input_mode, None)
            .await
    }

    /// Like `create_session` but also records a `user_target_id` on the
    /// session row so the WS PTY spawn path can look up the user-owned
    /// target config if `TargetEngine::resolve` misses.
    pub async fn create_session_with_user_target(
        &self,
        owner: &User,
        target_name: &str,
        input_mode: InputMode,
        user_target_id: Option<&str>,
    ) -> Result<Session> {
        // Atomic: session row + owner participant row land together or
        // not at all. See `Storage::create_session_with_owner`.
        let session = self
            .storage
            .create_session_with_owner(owner.id, target_name, input_mode, user_target_id)
            .await?;

        // Audit the lifecycle event after the storage write has
        // succeeded — no point recording a session that never
        // existed. `record` is log-and-swallow, so a transient audit
        // outage does not fail the user-visible create call. We also
        // emit an implicit `participant.joined` for the owner so the
        // timeline reads naturally ("alice created session sess-X /
        // alice joined as Owner") without requiring a second explicit
        // upsert call.
        self.audit
            .record(
                AuditEvent::new(AuditEventType::SessionCreated)
                    .with_actor(owner.id, owner.name.clone())
                    .with_session(session.id.clone())
                    .with_detail(serde_json::json!({
                        "target_name": target_name,
                        "input_mode": input_mode.as_str(),
                    })),
            )
            .await;
        self.audit
            .record(
                AuditEvent::new(AuditEventType::ParticipantJoined)
                    .with_actor(owner.id, owner.name.clone())
                    .with_session(session.id.clone())
                    .with_detail(serde_json::json!({ "role": Role::Owner.as_str() })),
            )
            .await;
        Ok(session)
    }

    /// Close a session, stamping the given `reason` into the history
    /// row so the UI can show "owner closed" vs "reaper timed out"
    /// vs "server restart". Every close path in the codebase routes
    /// through this method — inline `.storage().close_session(...)`
    /// calls would lose the reason and silently backfill `None`.
    ///
    /// `actor` is the user who triggered the close when there is one
    /// (owner clicking "end session"), or `None` for server-initiated
    /// paths (reaper, startup sweep). The audit event keeps the
    /// distinction so the history view can render "Owner alice ended"
    /// vs "Timed out after N minutes".
    pub async fn close_session(
        &self,
        session_id: &str,
        reason: CloseReason,
        actor: Option<&User>,
    ) -> Result<()> {
        // Snapshot the pre-close row so the audit `duration_s` can
        // use the authoritative `created_at` from the DB rather than
        // whatever the caller might cache. `get_session` is cheap and
        // runs against the same connection pool as the close.
        let pre = self.storage.get_session(session_id).await?;
        self.storage.close_session(session_id, reason).await?;

        // Best-effort audit after the transaction committed.
        let mut event = AuditEvent::new(AuditEventType::SessionClosed).with_session(session_id);
        if let Some(user) = actor {
            event = event.with_actor(user.id, user.name.clone());
        }
        let duration_s = pre
            .as_ref()
            .map(|s| (chrono::Utc::now() - s.created_at).num_seconds());
        event = event.with_detail(serde_json::json!({
            "reason": reason.as_str(),
            "duration_s": duration_s,
        }));
        self.audit.record(event).await;
        Ok(())
    }

    /// Owner-initiated close, atomic with the ownership check. The
    /// HTTP `DELETE /api/sessions/:id` handler used to inline the
    /// "fetch session, compare owner_id, then close" sequence — which
    /// duplicated the policy already encoded in [`Self::require_owner`]
    /// and silently drifted the moment any other call site needed the
    /// same shape. Routing the handler through this wrapper means
    /// "who can close a session" lives in exactly one place; if the
    /// rule ever widens (e.g. admins can yank other users' sessions)
    /// the change happens here, not scattered across the gateway.
    ///
    /// Always uses `CloseReason::Owner` and passes the actor through
    /// to the audit emit so the timeline reads "alice ended session
    /// X" instead of an actorless `session.closed`. Reaper / startup
    /// callers stay on the bare [`Self::close_session`] entry point
    /// because they have no actor and stamp their own reason.
    pub async fn close_session_as_owner(&self, user: &User, session_id: &str) -> Result<()> {
        // `require_owner` already returns the right error variants:
        // missing → SessionNotFound (404), wrong owner → PermissionDenied
        // (403). We discard the returned `Session` because the inner
        // `close_session` re-fetches it for the audit `duration_s`
        // calculation; doing one extra read costs nothing meaningful
        // on a path that fires once per session lifetime, and avoids
        // a parallel mutable-borrow detour through the storage layer.
        self.require_owner(user, session_id).await?;
        self.close_session(session_id, CloseReason::Owner, Some(user))
            .await
    }

    /// Bulk-close every still-active session, used by the boot-time
    /// startup cleanup. Always passes through as `CloseReason::Startup`
    /// at the CLI call site; exposed on the service so test fixtures
    /// can reach it without talking to storage directly. Emits one
    /// synthetic `session.closed` audit row carrying the bulk count
    /// rather than one-per-session — the history timeline would
    /// otherwise be flooded on every restart against a DB that had
    /// been running for weeks.
    pub async fn close_stale_sessions(&self, reason: CloseReason) -> Result<u64> {
        let closed = self.storage.close_stale_sessions(reason).await?;
        if closed > 0 {
            self.audit
                .record(AuditEvent::new(AuditEventType::SessionClosed).with_detail(
                    serde_json::json!({
                        "reason": reason.as_str(),
                        "bulk": true,
                        "count": closed,
                    }),
                ))
                .await;
        }
        Ok(closed)
    }

    /// List sessions visible to `user_id`, filtered by `filter`.
    /// Default filter = every session the user owned or joined,
    /// active or closed, newest first — the shape the Dashboard
    /// Sessions tab wants. Callers that specifically need "what am
    /// I in right now" should pass [`SessionListFilter::active_only`].
    ///
    /// This is the **user-scoped** variant; it never returns rows
    /// the user did not own or participate in. For the admin-scoped
    /// "see every session in the system" view, route through
    /// [`Self::list_sessions_visible_to`] so the admin branch is
    /// picked based on `User::is_admin`.
    pub async fn list_sessions_for_user(
        &self,
        user_id: Uuid,
        filter: SessionListFilter,
    ) -> Result<Vec<Session>> {
        self.storage.list_sessions_for_user(user_id, filter).await
    }

    /// List sessions visible to `user`, dispatching on admin status:
    ///
    /// - `user.is_admin == true` → every session in the system,
    ///   filtered only by `filter`. This is what backs the admin
    ///   targets deep-link ("N active sessions on target X") and
    ///   the Dashboard Sessions tab for admins, so the count shown
    ///   on the admin card and the list shown after the deep-link
    ///   always match.
    /// - `user.is_admin == false` → the user-scoped variant, i.e.
    ///   only rows they owned or participated in.
    ///
    /// Guest/scoped tokens are still non-admin by construction
    /// (see `User::is_guest`), so a guest calling this method sees
    /// only their own session. The gateway continues to enforce
    /// `require_unscoped` separately on handlers that must reject
    /// guests entirely; this method is safe to expose to everyone.
    pub async fn list_sessions_visible_to(
        &self,
        user: &User,
        filter: SessionListFilter,
    ) -> Result<Vec<Session>> {
        if user.is_admin {
            self.storage.list_all_sessions(filter).await
        } else {
            self.storage.list_sessions_for_user(user.id, filter).await
        }
    }

    /// Count how many sessions are marked `status='active'` in the
    /// DB, grouped by `target_name`. Backs the admin targets page
    /// ("N active sessions on this target" deep link) because the
    /// deep link opens the sessions-list view, which also reads
    /// from the DB — so the display count must match what the
    /// linked list will show.
    ///
    /// **Do not use this for safety-gating reloads or anything
    /// else that must answer "is there a live PTY right now".**
    /// The DB row can briefly remain `active` after its PTY has
    /// exited (reaper hasn't run yet, or the cleanup branch of the
    /// PTY loop hasn't landed its close) and the hot-reload guard
    /// used to trip on that window. For the "live shell exists"
    /// question, walk the hub via
    /// `SessionHub::count_live_sessions_per_target` instead —
    /// defined in the gateway crate, so this control-layer method
    /// can't name it as an intra-doc link. Targets with zero rows
    /// are omitted from the map; callers look up absent targets
    /// as zero.
    pub async fn active_session_counts_per_target(
        &self,
    ) -> Result<std::collections::HashMap<String, u32>> {
        self.storage.count_active_sessions_per_target().await
    }

    // -- Session lookup ------------------------------------------------------

    /// Fetch a session by id. Returns `None` if the row does not exist;
    /// callers that want a hard-fail can use [`get_session_required`].
    pub async fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
        self.storage.get_session(session_id).await
    }

    /// Fetch a session by id or return [`Error::SessionNotFound`].
    pub async fn get_session_required(&self, session_id: &str) -> Result<Session> {
        self.storage
            .get_session(session_id)
            .await?
            .ok_or_else(|| Error::SessionNotFound(session_id.to_string()))
    }

    /// Fetch a session and verify the caller owns it. Collapses the
    /// previous `get_session + not_found + owner_check` trio into one
    /// call with consistent error variants:
    ///
    /// - missing session → `Error::SessionNotFound` (→ 404)
    /// - wrong owner     → `Error::PermissionDenied` (→ 403)
    ///
    /// Handlers should prefer this over hand-rolling the sequence so
    /// the "owner check" invariant is written exactly once.
    pub async fn require_owner(&self, user: &User, session_id: &str) -> Result<Session> {
        let session = self.get_session_required(session_id).await?;
        if session.owner_id != user.id {
            return Err(Error::PermissionDenied(format!(
                "user {} does not own session {session_id}",
                user.id
            )));
        }
        Ok(session)
    }

    /// Like [`require_owner`] but additionally rejects sessions that
    /// are no longer `Active` with [`Error::SessionClosed`] (→ 410 Gone).
    /// Used by endpoints that must refuse to mutate closed sessions
    /// (e.g. minting new invites against a dead session).
    pub async fn require_active_owned(&self, user: &User, session_id: &str) -> Result<Session> {
        let session = self.require_owner(user, session_id).await?;
        if session.status != SessionStatus::Active {
            return Err(Error::SessionClosed(session_id.to_string()));
        }
        Ok(session)
    }

    // -- Participants --------------------------------------------------------

    pub async fn list_participants(&self, session_id: &str) -> Result<Vec<Participant>> {
        self.storage.list_participants(session_id).await
    }

    /// Atomic "is this user currently a participant of an active
    /// session" lookup. Returns `Some(role)` iff the user has an
    /// active (non-`left_at`) participant row AND the session is
    /// still `Active`, both as-of the same DB snapshot. Backs the
    /// invite-redeem existing-member short path so that branch is
    /// free of the TOCTOU race the two-query version had.
    ///
    /// See [`Storage::find_active_participant_role`] for the exact
    /// guarantee and its boundary (it does not prevent a concurrent
    /// close from landing *after* the query; callers that need
    /// fully-atomic redemption must wrap their write in a single
    /// transaction, as `Storage::redeem_invite` already does).
    pub async fn find_active_participant_role(
        &self,
        session_id: &str,
        user_id: Uuid,
    ) -> Result<Option<Role>> {
        self.storage
            .find_active_participant_role(session_id, user_id)
            .await
    }

    /// Register (or reuse) a participant row for `user_id` in the given
    /// session. Delegates to the storage layer's idempotent upsert so
    /// a re-issued join never races against the existing row.
    pub async fn upsert_participant(
        &self,
        session_id: &str,
        user_id: Uuid,
        role: Role,
    ) -> Result<Participant> {
        self.storage
            .upsert_participant(session_id, user_id, role)
            .await
    }
}
