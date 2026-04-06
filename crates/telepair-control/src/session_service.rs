use std::sync::Arc;
use uuid::Uuid;

use telepair_core::error::Result;
use telepair_core::session::{InputMode, Session};
use telepair_core::storage::{SqliteStorage, Storage};

pub struct SessionService {
    storage: Arc<SqliteStorage>,
}

impl SessionService {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    pub fn storage(&self) -> &SqliteStorage {
        &self.storage
    }

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

    pub async fn list_active_sessions(&self) -> Result<Vec<Session>> {
        self.storage.list_active_sessions().await
    }

    pub async fn list_sessions_for_user(&self, user_id: Uuid) -> Result<Vec<Session>> {
        self.storage.list_sessions_for_user(user_id).await
    }
}
