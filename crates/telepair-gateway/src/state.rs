use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use telepair_agent::virtual_target::TargetEngine;
use telepair_control::auth_service::{AuthService, SmtpConfig};
use telepair_control::invite_service::InviteService;
use telepair_control::session_service::SessionService;
use telepair_control::user_target_service::UserTargetService;
use telepair_core::audit::AuditSink;
use telepair_core::auth::TokenAuthProvider;
use telepair_core::storage::{SqliteStorage, Storage};

use crate::session_hub::{ReaperConfig, SessionHub};

/// Shared state handed to every Axum handler. Holds the services
/// (auth, sessions, invites) that implement business logic, plus the
/// live [`SessionHub`] and target registry.
///
/// The `targets` field is an [`ArcSwap`] so admins can hot-reload
/// `targets.yaml` without blocking concurrent session-create and WS
/// handshake paths. Readers call `state.targets.load()` — wait-free —
/// and hold the returned guard only for the brief read; writers (the
/// `POST /api/admin/targets/reload` handler) install a fresh
/// [`TargetEngine`] via `store`.
///
/// The `storage` field is kept **only** for bootstrap + test fixture
/// seeding (see [`AppState::create_test_user`]). Production handlers
/// under `crates/telepair-gateway/src/{http,ws}.rs` must never reach
/// into `state.storage` — business rules live in the services. The
/// CI grep for `state\.storage` in those files is the enforcement
/// contract.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<TokenAuthProvider>,
    pub sessions: Arc<SessionService>,
    pub invites: Arc<InviteService>,
    pub audit: Arc<AuditSink>,
    pub targets: Arc<ArcSwap<TargetEngine>>,
    /// Absolute path the admin reload handler re-reads on demand.
    /// `None` means the operator never configured a targets file,
    /// so hot-reload has nothing to re-parse — the handler surfaces
    /// that as a 400. Stored as `Option<PathBuf>` (rather than
    /// requiring a path) because telepair boots fine without a
    /// `targets.yaml`; the default `local-shell` target still works.
    pub targets_path: Option<PathBuf>,
    pub hub: Arc<SessionHub>,
    /// Raw storage handle retained for bootstrap and test fixtures
    /// only. Do **not** read/write in HTTP/WS handlers — route
    /// through `sessions` / `invites` instead.
    pub storage: Arc<SqliteStorage>,
    /// Email registration and OTP verification service.
    pub auth_service: Arc<AuthService>,
    /// Per-user virtual target CRUD and PTY resolution.
    pub user_targets: Arc<UserTargetService>,
}

impl AppState {
    pub async fn new(
        storage: Arc<SqliteStorage>,
        engine: TargetEngine,
        targets_path: Option<PathBuf>,
        smtp: Option<Arc<SmtpConfig>>,
    ) -> Self {
        let auth = Arc::new(TokenAuthProvider::new(storage.clone()));
        let audit = Arc::new(AuditSink::new(storage.clone()));
        let sessions = Arc::new(SessionService::new(storage.clone(), audit.clone()));
        let invites = Arc::new(InviteService::new(
            storage.clone(),
            sessions.clone(),
            audit.clone(),
        ));
        let targets = Arc::new(ArcSwap::from_pointee(engine));
        let hub = Arc::new(SessionHub::new(sessions.clone()));
        let auth_service = Arc::new(AuthService::new(storage.clone(), smtp));
        let user_targets = Arc::new(UserTargetService::new(storage.clone()));
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
            invites,
            audit,
            targets,
            targets_path,
            hub,
            storage,
            auth_service,
            user_targets,
        }
    }

    pub async fn new_test() -> Self {
        let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
        let engine = TargetEngine::empty();
        // Skip `Self::new` here — test setups don't want a background
        // reaper racing against their assertions. Tests that need the
        // reaper spawn it explicitly.
        let auth = Arc::new(TokenAuthProvider::new(storage.clone()));
        let audit = Arc::new(AuditSink::new(storage.clone()));
        let sessions = Arc::new(SessionService::new(storage.clone(), audit.clone()));
        let invites = Arc::new(InviteService::new(
            storage.clone(),
            sessions.clone(),
            audit.clone(),
        ));
        let targets = Arc::new(ArcSwap::from_pointee(engine));
        let hub = Arc::new(SessionHub::new(sessions.clone()));
        let auth_service = Arc::new(AuthService::new(storage.clone(), None));
        let user_targets = Arc::new(UserTargetService::new(storage.clone()));
        Self {
            auth,
            sessions,
            invites,
            audit,
            targets,
            targets_path: None,
            hub,
            storage,
            auth_service,
            user_targets,
        }
    }

    /// Test helper: seed a non-admin user and return their raw token.
    /// Production handlers MUST NOT call this — it exists only so
    /// integration tests can skip the `POST /api/auth` round-trip
    /// when they just need an authenticated identity to attach to
    /// a request.
    pub async fn create_test_user(&self, name: &str) -> String {
        let (_, token) = self.storage.create_user(name, false).await.unwrap();
        token
    }

    /// Test helper: seed an admin user and return their raw token.
    /// Same rules as [`Self::create_test_user`] — integration tests
    /// only. Needed so tests can exercise the admin-only branches of
    /// handlers like `list_sessions` without reaching around the
    /// service layer.
    pub async fn create_test_admin(&self, name: &str) -> String {
        let (_, token) = self.storage.create_user(name, true).await.unwrap();
        token
    }
}
