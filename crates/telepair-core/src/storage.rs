pub mod sqlite;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::Result;
use crate::permission::Role;
use crate::session::{InputMode, InviteToken, Participant, Session, User};

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
    async fn create_scoped_guest(
        &self,
        name: &str,
        session_id: &str,
    ) -> Result<(User, String)>;
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
    async fn close_session(&self, id: &str) -> Result<()>;
    async fn list_active_sessions(&self) -> Result<Vec<Session>>;
    async fn list_sessions_for_user(&self, user_id: Uuid) -> Result<Vec<Session>>;
    async fn close_stale_sessions(&self) -> Result<u64>;

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
}
