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

use crate::rate_limit::{DEFAULT_REGISTER_MIN_INTERVAL, RegisterRateLimiter};
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
    /// Records the instant the server started, for uptime reporting.
    pub startup: std::time::Instant,
    /// Resolved data directory path (e.g. `~/.telepair`).
    pub data_dir: PathBuf,
    /// Whether SMTP was configured at startup.
    pub smtp_configured: bool,
    /// Per-IP rate limiter for `POST /api/auth/register`. `None` in
    /// test fixtures (so unit tests that fire many requests back to
    /// back aren't throttled) and in any wiring that cannot see the
    /// caller's `SocketAddr` (tower oneshot, reverse proxies that
    /// don't forward ConnectInfo). Production sets this via
    /// [`AppState::new`].
    pub register_rl: Option<Arc<RegisterRateLimiter>>,
    /// When `true`, the rate-limit gate on `POST /api/auth/register`
    /// reads the caller's real IP from `X-Real-IP` (preferred; set by
    /// nginx from `$remote_addr`, non-forgeable in the documented
    /// single-hop deployment) or the rightmost `X-Forwarded-For`
    /// entry (where `$proxy_add_x_forwarded_for` appends the real
    /// peer after whatever the client may have prepended). Must ONLY
    /// be enabled when telepair sits behind a reverse proxy that
    /// rewrites these headers on every inbound request — any
    /// deployment that accepts traffic directly from untrusted
    /// clients with this flag on lets attackers forge their source
    /// IP and bypass the throttle entirely. Off by default so a
    /// misconfigured operator fails closed to "per-socket IP", which
    /// is the safe default.
    pub trust_forwarded_headers: bool,
}

impl AppState {
    pub async fn new(
        storage: Arc<SqliteStorage>,
        engine: TargetEngine,
        targets_path: Option<PathBuf>,
        smtp: Option<Arc<SmtpConfig>>,
        data_dir: PathBuf,
    ) -> Self {
        let smtp_configured = smtp.is_some();
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
        let auth_service = Arc::new(AuthService::new(storage.clone(), smtp, audit.clone()));
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
        let register_rl = Arc::new(RegisterRateLimiter::new(DEFAULT_REGISTER_MIN_INTERVAL));
        // Detached sweep — drops stale entries so the map can't grow
        // unbounded under signup churn. Cadence matches the throttle
        // window; `ReaperConfig`-style tunables aren't warranted for
        // a single-purpose limiter this small.
        let sweep = Arc::clone(&register_rl);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(DEFAULT_REGISTER_MIN_INTERVAL);
            // First tick fires immediately; skip it so startup doesn't
            // purge an empty map just to touch the lock.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                sweep.purge_expired();
            }
        });
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
            startup: std::time::Instant::now(),
            data_dir,
            smtp_configured,
            register_rl: Some(register_rl),
            trust_forwarded_headers: false,
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
        let auth_service = Arc::new(AuthService::new(storage.clone(), None, audit.clone()));
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
            startup: std::time::Instant::now(),
            data_dir: PathBuf::from("/tmp/telepair-test"),
            smtp_configured: false,
            // Tests that want to exercise the limiter opt-in by
            // swapping this to `Some(...)` on the returned AppState.
            register_rl: None,
            trust_forwarded_headers: false,
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
