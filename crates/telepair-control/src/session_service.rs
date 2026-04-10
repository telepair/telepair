use std::sync::Arc;
use uuid::Uuid;

use telepair_core::error::{Error, Result};
use telepair_core::permission::Role;
use telepair_core::session::{InputMode, Participant, Session, SessionStatus, User};
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
}

impl SessionService {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    // -- Session lifecycle ---------------------------------------------------

    pub async fn create_session(
        &self,
        owner_id: Uuid,
        target_name: &str,
        input_mode: InputMode,
    ) -> Result<Session> {
        // Atomic: session row + owner participant row land together or
        // not at all. See `Storage::create_session_with_owner`.
        self.storage
            .create_session_with_owner(owner_id, target_name, input_mode)
            .await
    }

    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        self.storage.close_session(session_id).await
    }

    pub async fn list_sessions_for_user(&self, user_id: Uuid) -> Result<Vec<Session>> {
        self.storage.list_sessions_for_user(user_id).await
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
