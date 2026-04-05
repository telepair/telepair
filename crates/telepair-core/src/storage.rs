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
    async fn get_user(&self, id: Uuid) -> Result<Option<User>>;
    async fn get_user_by_name(&self, name: &str) -> Result<Option<User>>;
    async fn validate_token(&self, token: &str) -> Result<User>;

    // Sessions
    async fn create_session(
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
    async fn add_participant(
        &self,
        session_id: &str,
        user_id: Uuid,
        role: Role,
    ) -> Result<Participant>;
    async fn upsert_participant(
        &self,
        session_id: &str,
        user_id: Uuid,
        role: Role,
    ) -> Result<Participant>;
    async fn remove_participant(&self, session_id: &str, user_id: Uuid) -> Result<()>;
    async fn list_participants(&self, session_id: &str) -> Result<Vec<Participant>>;

    // Invite Tokens
    async fn create_invite(
        &self,
        session_id: &str,
        role: Role,
        max_uses: i32,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(InviteToken, String)>;
    async fn validate_invite(&self, token: &str) -> Result<InviteToken>;
    async fn consume_invite(&self, token: &str) -> Result<InviteToken>;
}
