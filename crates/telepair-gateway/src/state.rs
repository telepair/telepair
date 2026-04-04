use std::sync::Arc;

use telepair_agent::virtual_target::TargetEngine;
use telepair_control::session_service::SessionService;
use telepair_control::target_service::TargetService;
use telepair_core::auth::TokenAuthProvider;
use telepair_core::storage::{SqliteStorage, Storage};

use crate::session_hub::SessionHub;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<TokenAuthProvider>,
    pub sessions: Arc<SessionService>,
    pub targets: Arc<TargetService>,
    pub hub: Arc<SessionHub>,
}

impl AppState {
    pub async fn new(storage: Arc<SqliteStorage>, engine: TargetEngine) -> Self {
        let auth = Arc::new(TokenAuthProvider::new(storage.clone()));
        let sessions = Arc::new(SessionService::new(storage.clone()));
        let targets = Arc::new(TargetService::new(engine));
        let hub = Arc::new(SessionHub::new());
        Self {
            auth,
            sessions,
            targets,
            hub,
        }
    }

    pub async fn new_test() -> Self {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let engine = TargetEngine::empty();
        Self::new(storage, engine).await
    }

    pub async fn create_test_user(&self, name: &str) -> String {
        let (_, token) = self.sessions.storage().create_user(name, false).await.unwrap();
        token
    }
}
