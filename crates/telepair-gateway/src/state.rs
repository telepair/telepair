use std::sync::Arc;

use telepair_agent::virtual_target::TargetEngine;
use telepair_control::session_service::SessionService;
use telepair_control::target_service::TargetService;
use telepair_core::auth::TokenAuthProvider;
use telepair_core::storage::{SqliteStorage, Storage};

use crate::session_hub::{ReaperConfig, SessionHub};

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
        let hub = Arc::new(SessionHub::new(storage.clone()));
        // Production: start the idle-session reaper so orphaned PTYs
        // don't leak when all clients disconnect. The JoinHandle is
        // intentionally detached — the task lives for the process
        // lifetime. Tests that want to exercise reaping behaviour use
        // `new_test` and spawn their own reaper with a fast config.
        //
        // `drop` (not `let _`) to silence clippy::let_underscore_future:
        // ignoring the handle detaches the task, which is what we want.
        std::mem::drop(hub.spawn_reaper(ReaperConfig::default()));
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
        // Skip `Self::new` here — test setups don't want a background
        // reaper racing against their assertions. Tests that need the
        // reaper spawn it explicitly.
        let auth = Arc::new(TokenAuthProvider::new(storage.clone()));
        let sessions = Arc::new(SessionService::new(storage.clone()));
        let targets = Arc::new(TargetService::new(engine));
        let hub = Arc::new(SessionHub::new(storage.clone()));
        Self {
            auth,
            sessions,
            targets,
            hub,
        }
    }

    pub async fn create_test_user(&self, name: &str) -> String {
        let (_, token) = self
            .sessions
            .storage()
            .create_user(name, false)
            .await
            .unwrap();
        token
    }
}
