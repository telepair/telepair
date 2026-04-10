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
        // Atomic: session row + owner participant row land together or
        // not at all. See `Storage::create_session_with_owner`.
        let session = self
            .storage
            .create_session_with_owner(owner.id, target_name, input_mode)
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
    pub async fn list_sessions_for_user(
        &self,
        user_id: Uuid,
        filter: SessionListFilter,
    ) -> Result<Vec<Session>> {
        self.storage.list_sessions_for_user(user_id, filter).await
    }

    /// Count how many currently-active sessions exist per target
    /// name. Backs the admin targets page ("N active sessions on
    /// this target" deep link) without forcing the HTTP handler to
    /// reach into raw storage — that path is reserved for
    /// bootstrap and test fixtures only. Targets with zero rows
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
