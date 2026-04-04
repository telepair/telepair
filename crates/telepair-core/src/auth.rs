use std::sync::Arc;

use crate::error::Result;
use crate::session::User;
use crate::storage::{SqliteStorage, Storage};

pub struct TokenAuthProvider {
    storage: Arc<SqliteStorage>,
}

impl TokenAuthProvider {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    pub async fn validate(&self, token: &str) -> Result<User> {
        self.storage.validate_token(token).await
    }

    pub async fn create_user(&self, name: &str) -> Result<(User, String)> {
        self.storage.create_user(name, false).await
    }

    pub async fn setup_initial_admin(&self, name: &str) -> Result<(User, String)> {
        self.storage.create_user(name, true).await
    }
}
