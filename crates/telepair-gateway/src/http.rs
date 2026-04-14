use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};

use axum::{
    Json,
    extract::{ConnectInfo, FromRequestParts, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use telepair_control::invite_service::CreateInviteParams;
use telepair_control::user_target_service::{CreateTargetParams, UpdateTargetParams};
use telepair_core::error::Error;
use telepair_core::permission::Role;
use telepair_core::session::{InputMode, SessionListFilter, SessionStatus, User};
use telepair_core::storage::{AccountFilter, AccountStatus};
use telepair_core::target::TargetKind;

use uuid::Uuid;

use crate::state::AppState;

/// Handler-level error wrapper. `?` on any `Result<_, core::Error>`
/// lifts via `From`, so `InvalidInput` never leaks out as 500 and auth
/// failures always surface as 401/403. `StatusCode` also lifts in, for
/// the handful of sites that short-circuit with a hard-coded status
/// (e.g. `return Err(StatusCode::BAD_REQUEST.into())` on body validation).
///
/// The response body is a minimal JSON object `{"error": "..."}` when a
/// message is available, or the canonical status reason otherwise.
pub struct ApiError {
    pub(crate) status: StatusCode,
    message: Option<String>,
}

impl ApiError {
    fn bare(status: StatusCode) -> Self {
        Self {
            status,
            message: None,
        }
    }

    /// Build an `ApiError` with a custom 4xx message. 5xx callers
    /// should stick to `bare` — the `IntoResponse` impl redacts their
    /// body either way, so providing a string here would just be
    /// leaked into logs.
    pub(crate) fn with_message(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: Some(message.into()),
        }
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self {
            status: StatusCode::from_u16(e.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            message: Some(e.to_string()),
        }
    }
}

impl From<StatusCode> for ApiError {
    fn from(s: StatusCode) -> Self {
        Self::bare(s)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // 5xx bodies never echo `Error::Display` — `Storage`/`Io`/`Yaml`
        // can contain SQL fragments, file paths, or other server-side
        // detail that must not leak to clients. Client errors (4xx) keep
        // their message so frontend toasts stay actionable.
        let canonical = || {
            self.status
                .canonical_reason()
                .unwrap_or("error")
                .to_string()
        };
        let msg = if self.status.is_server_error() {
            canonical()
        } else {
            self.message.unwrap_or_else(canonical)
        };
        (self.status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

/// Extractor that yields the client's `SocketAddr` when axum was
/// booted with `into_make_service_with_connect_info::<SocketAddr>`
/// and `None` otherwise. Axum's built-in `ConnectInfo<T>` extractor
/// is not `Option`-wrappable (there is no `OptionalFromRequestParts`
/// impl for it in 0.8), so we read the extension directly —
/// mirroring what `ConnectInfo::from_request_parts` does internally
/// but returning `None` instead of a 500 on absence. This lets
/// tower `oneshot`-style tests keep calling the handler without
/// installing a fake ConnectInfo.
pub struct OptionalClientAddr(pub Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for OptionalClientAddr {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(OptionalClientAddr(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ci| ci.0),
        ))
    }
}

/// Resolve the client IP the rate-limit gate should key on. When
/// `trust_forwarded_headers` is off (the default) we always use the
/// socket peer: a deployment that accepts direct traffic cannot trust
/// `X-Forwarded-For` — any client can forge it and reset the bucket.
///
/// When the flag is on, we prefer `X-Real-IP`: the nginx deployment
/// telepair documents (see `docs/deployment.md`) sets it from
/// `$remote_addr`, i.e. the direct TCP peer at the proxy. That value
/// is authoritative in a single-hop setup and cannot be spoofed by
/// the client. We fall back to the *rightmost* `X-Forwarded-For`
/// segment: the documented nginx snippet uses
/// `$proxy_add_x_forwarded_for`, which **appends** the real peer to
/// any `X-Forwarded-For` the client sent, so only the last entry is
/// trustworthy — reading the leftmost would hand attackers a way to
/// reset their bucket by forging a client-side XFF header.
///
/// If both headers are absent or unparseable we drop back to the
/// socket peer rather than skip the gate — a misconfigured proxy
/// that strips these headers must still cost the attacker one bucket
/// per proxy pod, not zero. Multi-hop chains remain out of scope for
/// v0.1.5 and would need an operator-configured trusted-proxy CIDR
/// list to walk the XFF chain safely.
fn resolve_client_ip(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> Option<IpAddr> {
    if state.trust_forwarded_headers {
        if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok())
            && let Ok(ip) = real.trim().parse::<IpAddr>()
        {
            return Some(ip);
        }
        if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
            && let Some(last) = xff.rsplit(',').next()
            && let Ok(ip) = last.trim().parse::<IpAddr>()
        {
            return Some(ip);
        }
    }
    peer.map(|a| a.ip())
}

// --- Auth extraction ---

pub async fn extract_user(state: &AppState, headers: &HeaderMap) -> Result<User, ApiError> {
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::bare(StatusCode::UNAUTHORIZED))?;

    Ok(state.auth.validate(token).await?)
}

/// Reject invite-minted guests on account-level routes. A scoped
/// guest token is only valid for its bound session — it must not be
/// usable to enumerate targets, spin up new sessions, or otherwise
/// behave like a real account. 403 (not 401) because the caller is
/// authenticated, they just don't have the scope for this route.
fn require_unscoped(user: &User) -> Result<(), ApiError> {
    if user.is_guest() {
        return Err(ApiError::bare(StatusCode::FORBIDDEN));
    }
    Ok(())
}

/// Emit an `auth.session_access_denied` audit row. Shared by the
/// HTTP `require_session_enabled` gate and the WS attach gate so
/// both attach surfaces produce the same audit shape and cannot
/// drift independently.
pub(crate) async fn audit_session_access_denied(
    state: &AppState,
    user: &User,
    path: &str,
    session_id: Option<&str>,
) {
    let mut event = telepair_core::audit::AuditEvent::new(
        telepair_core::audit::AuditEventType::AuthSessionAccessDenied,
    )
    .with_actor(user.id, user.name.clone())
    .with_detail(serde_json::json!({ "path": path }));
    if let Some(sid) = session_id {
        event = event.with_session(sid.to_string());
    }
    state.audit.record(event).await;
}

/// Gate every session-mutating handler on the user's `session_enabled`
/// bit. Admins bypass so bootstrap cannot lock itself out. Rejections
/// are audited via [`audit_session_access_denied`]; pass `Some(id)`
/// when the request is bound to a specific session so the audit row
/// carries the `session_id` column (invite mint/revoke/redeem,
/// participant-role updates). The bare "POST /api/sessions" path
/// leaves it `None` because the session does not exist yet.
async fn require_session_enabled(
    state: &AppState,
    user: &User,
    path: &str,
    session_id: Option<&str>,
) -> Result<(), ApiError> {
    if user.session_enabled || user.is_admin {
        return Ok(());
    }
    audit_session_access_denied(state, user, path, session_id).await;
    Err(ApiError::bare(StatusCode::FORBIDDEN))
}

// --- Handlers ---

pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

// ── Email registration ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub display_name: String,
}

/// `POST /api/auth/register` — create unverified account and send OTP.
/// Returns 503 if SMTP is not configured. 429 when the source IP is
/// within the register rate-limit window (see [`crate::rate_limit`]
/// for why this layer exists alongside the per-email throttle inside
/// `AuthService`). Always returns 201 on success (enumeration safety:
/// callers cannot distinguish "sent" from "already registered").
///
/// `ConnectInfo` is `Option`-wrapped so tower `oneshot` test harnesses
/// — which never populate connect info — still reach the handler.
/// Production (`axum::serve(..).into_make_service_with_connect_info`)
/// always provides it.
pub async fn register(
    State(state): State<AppState>,
    OptionalClientAddr(client_addr): OptionalClientAddr,
    headers: HeaderMap,
    body: Result<Json<RegisterRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(body) = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;

    // Only enforce the IP throttle when we actually know the caller's
    // address. If either piece is missing we skip the gate rather than
    // reject the request — a production deployment that forgot to wire
    // ConnectInfo would lock out every signup, and the per-email
    // limiter inside `AuthService::register` still covers the most
    // common "same user, mashing the button" shape.
    //
    // When `trust_forwarded_headers` is on, the key IP is pulled from
    // `X-Forwarded-For` / `X-Real-IP` instead of the socket peer —
    // otherwise the recommended "nginx in front of telepair" shape
    // collapses every caller onto 127.0.0.1 and the whole fleet shares
    // one bucket (see `resolve_client_ip`).
    let client_ip = resolve_client_ip(&state, &headers, client_addr);
    if let (Some(limiter), Some(ip)) = (state.register_rl.as_ref(), client_ip) {
        use crate::rate_limit::RateLimitDecision;
        if let RateLimitDecision::Throttled { retry_after } = limiter.check(ip) {
            // Round up to the next 10-second bucket instead of leaking
            // exact seconds. The precise remainder lets an attacker
            // infer how recent their last probe was (a low-resolution
            // timing oracle); 10s granularity is fine for the UX — a
            // human retrying a form doesn't need second-accurate
            // feedback — and clamped to at least 10 so the bucket is
            // always meaningful.
            let raw = retry_after.as_secs().max(1);
            let secs = raw.div_ceil(10) * 10;
            return Err(ApiError::with_message(
                StatusCode::TOO_MANY_REQUESTS,
                format!("Too many registrations from this address. Try again in {secs}s."),
            ));
        }
    }

    state
        .auth_service
        .register(&body.email, &body.password, &body.display_name)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"message": "Verification code sent to your email."})),
    ))
}

#[derive(Deserialize)]
pub struct VerifyOtpRequest {
    pub email: String,
    pub code: String,
}

/// `POST /api/auth/verify` — submit OTP code; returns bearer token on success.
pub async fn verify_otp(
    State(state): State<AppState>,
    body: Result<Json<VerifyOtpRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(body) = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;
    let token = state
        .auth_service
        .verify_otp(&body.email, &body.code)
        .await?;
    Ok(Json(serde_json::json!({"token": token})))
}

/// `POST /api/auth/login` — unified login accepting `{token}` (existing
/// admin path) or `{email, password}` (email-registered users).
#[derive(Deserialize)]
pub struct LoginRequest {
    pub token: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
}

pub async fn login(
    State(state): State<AppState>,
    body: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(body) = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;
    let token = if let Some(t) = body.token {
        // Validate existing bearer token (admin / guest path).
        state.auth.validate(&t).await?;
        t
    } else if let (Some(email), Some(password)) = (body.email, body.password) {
        state.auth_service.login(&email, &password).await?
    } else {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    };
    Ok(Json(serde_json::json!({"token": token})))
}

/// `GET /api/auth/whoami`
///
/// Returns the authenticated caller's identity. Used by the frontend
/// auth store to cache `currentUserId` so the dashboard can decide
/// per-row whether the caller owns the session — closed rows on
/// non-owned sessions stay inert (the audit dialog is owner-only and
/// would otherwise produce a deterministic 403). 401 on missing or
/// invalid bearer; never returns 403, since "I am a guest" is still a
/// valid identity to surface.
#[derive(Serialize)]
struct WhoamiResponse {
    user_id: String,
    name: String,
    is_admin: bool,
    is_guest: bool,
    /// Mirrors `User.session_enabled`. The Dashboard renders a
    /// "pending admin approval" banner and hides the session-create
    /// form when this is FALSE — surfacing the bit up front means
    /// the user learns about the gate on page load instead of
    /// hitting a mystery 403 when they click Create.
    session_enabled: bool,
}

pub async fn whoami(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    Ok(Json(WhoamiResponse {
        user_id: user.id.to_string(),
        is_admin: user.is_admin,
        is_guest: user.is_guest(),
        session_enabled: user.session_enabled,
        name: user.name,
    }))
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// `POST /api/auth/change-password` — authenticated password update.
pub async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<ChangePasswordRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(body) = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;
    let user = extract_user(&state, &headers).await?;
    let new_token = state
        .auth_service
        .change_password(&user, &body.current_password, &body.new_password)
        .await?;
    Ok(Json(serde_json::json!({ "token": new_token })))
}

pub async fn list_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    // Guests are scoped to a single session and have no dashboard —
    // they must never see a target list at all.
    require_unscoped(&user)?;

    #[derive(Serialize)]
    struct TargetInfo {
        name: String,
        display: String,
        tags: Vec<String>,
        /// "global" for targets from targets.yaml; "user" for user-owned targets.
        source: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        admin_only: bool,
    }

    // Info-leak fix: `admin_only` targets must not be enumerable by
    // non-admin callers. `load()` is wait-free — concurrent reloads
    // don't extend the guard's lifetime.
    let is_admin = user.is_admin;
    let engine = state.targets.load();
    let mut entries: Vec<TargetInfo> = engine
        .list_targets()
        .iter()
        .filter(|t| is_admin || !t.admin_only)
        .map(|t| TargetInfo {
            name: t.name.clone(),
            display: t.display.clone(),
            tags: t.tags.clone(),
            source: "global",
            id: None,
            admin_only: t.admin_only,
        })
        .collect();

    // Append user-owned targets (always visible to the owner, never admin_only)
    let user_targets = state.user_targets.list(user.id).await?;
    for ut in user_targets {
        entries.push(TargetInfo {
            name: ut.name,
            display: ut.display,
            tags: ut.tags,
            source: "user",
            id: Some(ut.id),
            admin_only: false,
        });
    }

    Ok(Json(entries))
}

/// Request body for `POST /api/sessions`.
///
/// Callers MUST set exactly one of `target_id` (user-owned target,
/// addressed by its stable nanoid) or `target_name` (global target from
/// `targets.yaml`, addressed by its name). Sending both, neither, or an
/// empty string for either is a 400 — see `pick_target_selector` for the
/// classifier and the rationale below.
///
/// Why two fields instead of one polymorphic string: the namespaces
/// overlap. A global `vps` target and a user's own `vps` target are
/// distinct rows that need to round-trip independently from create to WS
/// attach. Before this split, the handler resolved global-first /
/// user-target-fallback by name, so a user could never launch their own
/// `vps` while a global `vps` existed — and worse, a global target added
/// *after* the session was created could shadow the user's target on the
/// next WS attach.
#[derive(Deserialize)]
pub struct CreateSessionRequest {
    /// Global target name from `targets.yaml`. Mutually exclusive with `target_id`.
    #[serde(default)]
    pub target_name: Option<String>,
    /// User-owned target nanoid (`UserTarget.id`). Mutually exclusive with `target_name`.
    #[serde(default)]
    pub target_id: Option<String>,
    /// Omitted field defaults to `InputMode::Multiplexed` below — the
    /// collaborative default so invited operators can actually type,
    /// which is the whole point of "Google Docs for terminals". Owners
    /// who want a solo shell with shoulder-surfing viewers can still
    /// opt into `serialized`.
    #[serde(default)]
    pub input_mode: Option<InputMode>,
}

/// Which kind of target a `CreateSessionRequest` is asking for.
enum TargetSelector {
    /// Global (`targets.yaml`) target, looked up in the in-memory engine by name.
    Global(String),
    /// User-owned target, looked up in storage by stable nanoid.
    User(String),
}

/// Classify a `CreateSessionRequest` into exactly one selector or 400.
///
/// Empty strings count as "not set" so a frontend regression that posts
/// `{"target_name":"","target_id":"abc"}` still resolves the user target
/// instead of mysteriously 400ing on what looks like a populated body.
fn pick_target_selector(body: &CreateSessionRequest) -> Result<TargetSelector, ApiError> {
    let name = body.target_name.as_deref().filter(|s| !s.is_empty());
    let id = body.target_id.as_deref().filter(|s| !s.is_empty());
    match (name, id) {
        (Some(n), None) => Ok(TargetSelector::Global(n.to_string())),
        (None, Some(i)) => Ok(TargetSelector::User(i.to_string())),
        _ => Err(ApiError::bare(StatusCode::BAD_REQUEST)),
    }
}

pub async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<CreateSessionRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    // Auth first so unauthenticated callers get 401 instead of 400,
    // matching the other handlers in this file.
    let user = extract_user(&state, &headers).await?;
    // Scoped guests never create sessions — that's the entire point
    // of scoping. This 403 is the teeth of the invite fix: even if a
    // guest token is valid, this path is closed.
    require_unscoped(&user)?;
    // Self-served email signups land with `session_enabled = FALSE`
    // and are inert until an admin approves them on the user
    // management page. This is the load-bearing fix for the v0.1.2
    // adversarial finding: before this gate, anyone with SMTP
    // enabled could go signup → verify OTP → `POST /api/sessions`
    // against `local-shell` and pop a shell on the gateway host.
    require_session_enabled(&state, &user, "POST /api/sessions", None).await?;

    // Axum's default JSON rejection is 422; we want 400 so an unknown
    // `input_mode` value reads as "client sent garbage" instead of
    // "server doesn't know what to do with it".
    let Json(body) = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;
    let selector = pick_target_selector(&body)?;

    // Resolve the target without crossing namespaces. `target_name`
    // looks up the global engine; `target_id` looks up the user-owned
    // table. Neither falls back to the other — the whole point of
    // splitting the API is that a missing target is the right answer
    // when the namespace it lives in misses, not "try the other one".
    let (target_name, admin_only, user_target_id) = match selector {
        TargetSelector::Global(name) => {
            let engine = state.targets.load();
            match engine.find(&name) {
                Some(t) => (name.clone(), t.admin_only, None),
                None => return Err(ApiError::bare(StatusCode::NOT_FOUND)),
            }
        }
        TargetSelector::User(id) => {
            // `get` already filters on `target.user_id == caller.id`,
            // so non-owners receive `Ok(None)` — we map that to 404 so
            // the row's existence stays hidden from anyone who doesn't
            // own it (a 403 would let an attacker enumerate target ids).
            match state.user_targets.get(&id, user.id).await? {
                Some(ut) => (ut.name, false, Some(ut.id)),
                None => return Err(ApiError::bare(StatusCode::NOT_FOUND)),
            }
        }
    };

    if admin_only && !user.is_admin {
        // Audit the rejection so admins can see attempted lateral
        // moves in the history timeline. Best-effort; a failed audit
        // write does not change the 403 the caller sees.
        state
            .audit
            .record(
                telepair_core::audit::AuditEvent::new(
                    telepair_core::audit::AuditEventType::TargetAccessDenied,
                )
                .with_actor(user.id, user.name.clone())
                .with_detail(serde_json::json!({ "target_name": target_name })),
            )
            .await;
        return Err(ApiError::bare(StatusCode::FORBIDDEN));
    }

    let mode = body.input_mode.unwrap_or(InputMode::Multiplexed);

    let session = state
        .sessions
        .create_session_with_user_target(&user, &target_name, mode, user_target_id.as_deref())
        .await?;

    // Reserve the target slot in the hub *before* returning 201 so a
    // `targets` reload landing in the gap between this response and
    // the client's WS attach sees the target as still in use. Without
    // this, the reload guard's `count_live_sessions_per_target` walks
    // an empty hub for the brand-new session and could happily drop
    // the target — `ws::handle_socket` would then fail target
    // resolution and `cleanup_orphan_session` would stamp the row
    // `Error` before the client ever connected. The reservation is
    // upgraded to `Live` by `start_or_join` once the WS attaches, or
    // GCed after `pending_attach_ttl` if the client never shows.
    state.hub.reserve_target(&session.id, &target_name).await;

    Ok((StatusCode::CREATED, Json(session)))
}

/// Query params for `GET /api/sessions`. Everything is optional; the
/// defaults ("every session the user owned or joined, newest first")
/// are what the Dashboard Sessions tab wants.
#[derive(Deserialize, Default)]
pub struct ListSessionsQuery {
    /// `active` | `closed` | `all`. Missing or `all` = both statuses.
    /// Unknown values fall back to "all" rather than 400ing because
    /// the query string is mostly driven by UI state; a typo should
    /// not blow up the page.
    #[serde(default)]
    pub status: Option<String>,
    /// Filter to a specific target name — used by the admin page's
    /// "N active sessions" deep link. The field name must stay
    /// `target_name` because that's what the frontend API layer and
    /// `SessionListFilter` both use; renaming to `target` silently
    /// dropped the filter in v0.1.1-dev.
    #[serde(default)]
    pub target_name: Option<String>,
    /// Upper bound on rows returned. Missing = unlimited.
    #[serde(default)]
    pub limit: Option<i64>,
    /// Row offset for pagination; 0 when absent.
    #[serde(default)]
    pub offset: Option<i64>,
}

impl ListSessionsQuery {
    fn into_filter(self) -> SessionListFilter {
        let status = match self.status.as_deref() {
            Some("active") => Some(SessionStatus::Active),
            Some("closed") => Some(SessionStatus::Closed),
            _ => None, // "all", missing, typos
        };
        SessionListFilter {
            status,
            target_name: self.target_name.filter(|s| !s.is_empty()),
            // Guard against negative values; sqlx would pass them to
            // SQLite verbatim and you'd get empty results instead of
            // an obvious error. Clamp to 0/None.
            limit: self.limit.filter(|&n| n > 0),
            offset: self.offset.filter(|&n| n > 0).unwrap_or(0),
        }
    }
}

pub async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListSessionsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    // Admin-aware dispatch: admins see every session in the system
    // so the counts on the admin targets page and the deep-linked
    // session list agree; everyone else (including scoped guests)
    // sees only their own owner/participant rows. The branch lives
    // inside `SessionService::list_sessions_visible_to` so the
    // gateway doesn't spread auth decisions across layers.
    let visible = state
        .sessions
        .list_sessions_visible_to(&user, query.into_filter())
        .await?;

    Ok(Json(visible))
}

// --- Invite handlers ---

#[derive(Deserialize)]
pub struct CreateInviteRequest {
    pub role: Role,
    #[serde(default = "default_max_uses")]
    pub max_uses: i32,
    /// Optional TTL in minutes — mutually exclusive with `expires_at`
    /// and `expires_in_secs`. The UI uses this because it's easier
    /// than picking an absolute wall-clock time in a form; the backend
    /// resolves it to an absolute `DateTime<Utc>` before hitting storage
    /// so the DB only ever sees concrete timestamps.
    #[serde(default)]
    pub expires_in_minutes: Option<i64>,
    /// Optional TTL in seconds — wins over `expires_in_minutes` when
    /// both are set. Exists because sub-minute invites are convenient
    /// for tests and demos (and the QA sweep caught the gap).
    #[serde(default)]
    pub expires_in_secs: Option<i64>,
    /// Optional absolute expiry. If both a TTL field and `expires_at`
    /// are set, this wins — callers shouldn't pass both but if they
    /// do we prefer the one with less ambiguity.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

fn default_max_uses() -> i32 {
    1
}

pub async fn create_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Result<Json<CreateInviteRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    // Auth first so unauthenticated callers get 401 instead of 400.
    let user = extract_user(&state, &headers).await?;

    // A disabled owner must not keep minting invites against a session
    // that outlived the disable. Before this gate, `session_enabled =
    // FALSE` only blocked `POST /api/sessions` and WS attach, so a
    // disabled owner could still mutate membership on any surviving
    // session until it closed. Mirrors the gate in `create_session`.
    require_session_enabled(
        &state,
        &user,
        "POST /api/sessions/{id}/invites",
        Some(session_id.as_str()),
    )
    .await?;

    // Axum's default JSON rejection is 422; every other handler in this
    // file remaps to 400 so clients get a consistent "you sent garbage"
    // signal regardless of which field was wrong.
    let Json(body) = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;

    // Ownership, alive gate, TTL precedence/clamping, role/max_uses
    // validation, token mint — all live inside `InviteService::create`.
    // The HTTP layer is pure transport + serialization.
    let result = state
        .invites
        .create(
            &user,
            &session_id,
            CreateInviteParams {
                role: body.role,
                max_uses: body.max_uses,
                expires_in_minutes: body.expires_in_minutes,
                expires_in_secs: body.expires_in_secs,
                expires_at: body.expires_at,
            },
        )
        .await?;

    Ok((StatusCode::CREATED, Json(result)))
}

#[derive(Deserialize)]
pub struct RedeemInviteRequest {
    pub token: String,
}

pub async fn close_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    // The "owner can close their own session" policy lives entirely
    // inside `SessionService::close_session_as_owner`, which combines
    // the existence check, ownership check, and audit-stamped close
    // into one call. Previously this handler hand-rolled all three —
    // duplicating the rule already encoded in `require_owner` and
    // making it possible to drift one site without the other. The
    // earlier version also overlapped auth and the session fetch
    // with `tokio::join!` to save one round-trip; that micro-opt was
    // not worth keeping a second copy of the policy in the gateway,
    // since DELETE fires once per session lifetime.
    let user = extract_user(&state, &headers).await?;
    state
        .sessions
        .close_session_as_owner(&user, &session_id)
        .await?;
    state.hub.stop_session(&session_id).await;
    Ok(StatusCode::NO_CONTENT)
}

// --- Participant role management ---

#[derive(Deserialize)]
pub struct UpdateRoleRequest {
    pub role: Role,
}

/// `PUT /api/sessions/:session_id/participants/:user_id/role`
///
/// Owner-only. Changes a participant's role in a live session. The
/// owner cannot change their own role or promote anyone to owner.
/// Persists the change to the DB, updates the hub's in-memory state,
/// and broadcasts `PeerRoleChanged` to all connected clients so UIs
/// update in lockstep and the WS handler re-evaluates input
/// permissions for the affected connection.
pub async fn update_participant_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, target_user_id)): Path<(String, String)>,
    body: Result<Json<UpdateRoleRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let body = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?.0;
    let user = extract_user(&state, &headers).await?;
    require_session_enabled(
        &state,
        &user,
        "PUT /api/sessions/{id}/participants/{user_id}/role",
        Some(session_id.as_str()),
    )
    .await?;
    state
        .sessions
        .require_active_owned(&user, &session_id)
        .await?;

    let target_uid =
        Uuid::parse_str(&target_user_id).map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;

    // Cannot change own role — the owner role is immutable.
    if target_uid == user.id {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    }
    // Cannot promote to owner.
    if body.role == Role::Owner {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    }

    // Verify the target is an active participant and get old role.
    let old_role = state
        .sessions
        .find_active_participant_role(&session_id, target_uid)
        .await?
        .ok_or(ApiError::bare(StatusCode::NOT_FOUND))?;

    if old_role == body.role {
        // No-op: role already matches.
        return Ok(StatusCode::NO_CONTENT);
    }

    // Persist to DB.
    state
        .sessions
        .upsert_participant(&session_id, target_uid, body.role)
        .await?;

    // Update in-memory hub state + broadcast. The hub returns false
    // when the target is not in a live session (e.g. they disconnected
    // between the DB write and this call). The DB change is still
    // correct — the next reconnect picks up the new role — but the
    // live WS handler won't get a PeerRoleChanged broadcast, so log
    // a warning for operators.
    if !state
        .hub
        .update_participant_role(&session_id, target_uid, body.role)
        .await
    {
        tracing::warn!(
            session_id,
            %target_uid,
            new_role = body.role.as_str(),
            "hub role update missed: participant not in live session (DB persisted, will apply on reconnect)"
        );
    }

    // Audit.
    state
        .audit
        .record(
            telepair_core::audit::AuditEvent::new(
                telepair_core::audit::AuditEventType::ParticipantRoleChanged,
            )
            .with_actor(user.id, user.name.clone())
            .with_session(session_id)
            .with_detail(serde_json::json!({
                "target_user_id": target_uid.to_string(),
                "old_role": old_role.as_str(),
                "new_role": body.role.as_str(),
            })),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/invite/redeem`
///
/// Auth is **optional**. If the request carries a valid bearer token,
/// the caller is added to the session under their existing identity
/// (lets an admin test their own invite link without spawning a
/// throwaway guest account). If no token, or the token is invalid,
/// the handler mints a fresh guest user and returns its freshly
/// issued token in the response — this is the load-bearing flow that
/// makes collaborators work without any out-of-band token handoff.
///
/// Response always contains `session_id` and `role`. The `token`
/// field is present **only** when a new guest was created; an
/// already-authenticated caller keeps using the token they came in
/// with and gets `token: null`.
pub async fn redeem_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<RedeemInviteRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    // Best-effort auth: a bearer token is no longer required. We try
    // to validate it so a logged-in user reuses their identity, but
    // a missing/invalid token falls through to the guest path instead
    // of failing the whole request. Only `UNAUTHORIZED` is swallowed —
    // any other status (e.g. 500 from a DB outage) still propagates so
    // the caller gets a real error instead of a spurious guest mint.
    let existing_user = match extract_user(&state, &headers).await {
        Ok(u) => Some(u),
        Err(ApiError {
            status: StatusCode::UNAUTHORIZED,
            ..
        }) => None,
        Err(other) => return Err(other),
    };

    // An authenticated but disabled account must not be able to redeem:
    // before this gate the redeem path would happily consume a use and
    // write a participant row, with the WS attach later rejecting the
    // connection. One-shot invites were effectively griefable. The
    // guest path (existing_user = None) is unaffected — a fresh guest
    // is always minted with `session_enabled = TRUE`. `session_id` is
    // unknown here because `find_invite` hasn't run yet; the audit row
    // still carries `path` so operators can slice by surface.
    if let Some(ref user) = existing_user {
        require_session_enabled(&state, user, "POST /api/invite/redeem", None).await?;
    }

    // Keep the JSON rejection semantics consistent across the handlers:
    // a malformed body is a 400, not a 422. This matters for the
    // frontend's error-handling code which branches on "bad request"
    // (show form error) vs "server error" (show toast + retry) — the
    // old 422 made bogus redeems look like a server crash.
    let Json(body) = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;

    // Everything else — preview, scoped-guest check, closed-session
    // gate, existing-member no-op, atomic consume, guest mint,
    // participant upsert — lives inside `InviteService::redeem`.
    // `RedeemResult` derives `Serialize` with `issued_token` renamed
    // to `token`, so we can hand it straight to `Json` without a
    // per-field copy.
    let result = state.invites.redeem(existing_user, &body.token).await?;
    Ok(Json(result))
}

/// `GET /api/sessions/:id/invites`
///
/// Returns every invite ever minted for this session (active, expired,
/// exhausted — all of them), sanitized into `InviteSummary`. Owner-only:
/// a non-owner caller gets 403, and a missing session gets 404. The
/// response deliberately includes post-mortem rows so the management
/// dialog can show "these were the invites in flight when the session
/// closed" without a separate code path.
pub async fn list_session_invites(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    let rows = state.invites.list_for_session(&user, &session_id).await?;
    Ok(Json(rows))
}

/// `DELETE /api/sessions/:id/invites/:token_sha256`
///
/// Hard-deletes the invite row. Owner-only; idempotent — always returns
/// 204 on success regardless of whether the row was actually present.
/// Double-revoke, an unknown sha, and a cross-session probe (valid sha
/// pointing at a session the caller doesn't own) all collapse into the
/// same 204 shape so an attacker cannot use this surface as a yes/no
/// oracle for invite existence. The UI treats 204 as "it's gone now"
/// and refreshes its list.
pub async fn revoke_session_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, token_sha256)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    // Revoke is a session-level mutation — gate it alongside mint so a
    // disabled owner cannot still tear down invites that would let a
    // replacement operator (e.g. another admin) clean up.
    require_session_enabled(
        &state,
        &user,
        "DELETE /api/sessions/{id}/invites/{token}",
        Some(session_id.as_str()),
    )
    .await?;
    state
        .invites
        .revoke(&user, &session_id, &token_sha256)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/sessions/:id/audit`
///
/// Returns the audit events that touched this session, newest first.
/// Owner-only: a non-owner gets 403 and a missing session gets 404 —
/// same gate as `list_session_invites` since the audit timeline and
/// the invite list are part of the same "session detail" admin view.
/// Closed sessions are still readable (the whole point of a history
/// view), so this goes through `require_owner` not
/// `require_active_owned`.
///
/// No pagination surface yet — capped at 500 rows, newest first, which
/// covers every real session's footprint by at least 2 orders of
/// magnitude. When a session outgrows that we'll add `?limit/offset`.
pub async fn list_session_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    // Ownership + existence gate lives in the session service so the
    // 403/404 split stays identical to the rest of the session detail
    // surface.
    state.sessions.require_owner(&user, &session_id).await?;

    let filter = telepair_core::audit::AuditFilter {
        session_id: Some(session_id),
        limit: Some(500),
        ..Default::default()
    };
    let rows = state.audit.query(filter).await?;
    Ok(Json(rows))
}

// --- Admin target management ---

/// One target's full config as returned by `GET /api/admin/targets`.
///
/// This is the operator-facing view, which is why it carries fields
/// the public `list_targets` endpoint deliberately hides:
///
/// - `command` / `args` / `shell`: the raw strings from
///   `targets.yaml`. Env-var interpolation still happens at spawn
///   time in `TargetEngine::resolve`; the JSON here preserves the
///   literal `${VAR}` placeholders so the admin UI shows exactly
///   what's on disk.
/// - `env`: a list of key names with a `set` boolean indicating
///   whether the process env has a value for each key. **Values
///   are never serialized.** Telepair is a single-process tool that
///   already trusts whoever can write `targets.yaml`, but exposing
///   resolved secrets through an HTTP API would still widen the
///   blast radius beyond that implicit trust. Keys-only is the
///   safest readable shape.
/// - `active_sessions`: live count from the storage layer, used by
///   the admin UI to render deep-link chips into the session
///   history view filtered by this target name.
#[derive(Serialize)]
struct AdminTargetInfo {
    name: String,
    display: String,
    /// `TargetKind` already serializes as lowercase (`"virtual"` /
    /// `"local"`) via its `rename_all` attribute, so we let serde do
    /// the mapping and just rename the field to `type` on the wire.
    #[serde(rename = "type")]
    kind: TargetKind,
    command: Option<String>,
    args: Vec<String>,
    shell: Option<String>,
    tags: Vec<String>,
    admin_only: bool,
    env: Vec<AdminTargetEnvKey>,
    active_sessions: u32,
}

/// Env key presence marker. `set = true` means `std::env::var(key)`
/// would return `Ok` right now. This is a snapshot taken at request
/// time, not a persistent record.
#[derive(Serialize)]
struct AdminTargetEnvKey {
    key: String,
    set: bool,
}

/// Reject a non-admin caller with 403. 401 is handled upstream in
/// `extract_user`; this helper runs AFTER the user has been
/// identified and only checks the role. Kept as a named helper so
/// the admin handlers read as "extract, require admin, do work"
/// without the gate inlined each time.
fn require_admin(user: &User) -> Result<(), ApiError> {
    if !user.is_admin {
        return Err(ApiError::bare(StatusCode::FORBIDDEN));
    }
    Ok(())
}

/// `GET /api/admin/targets`
///
/// Admin-only full target list, including env key presence and the
/// per-target active session count. See [`AdminTargetInfo`] for the
/// security rationale — env values are never returned.
pub async fn list_admin_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_admin(&user)?;

    // Grouped SELECT on `sessions` — single indexed query. Routed
    // through `SessionService` so the HTTP layer stays free of the
    // raw `Storage` accessor that the rest of the refactor stripped
    // out.
    let counts = state
        .sessions
        .active_session_counts_per_target()
        .await
        .map_err(ApiError::from)?;

    // Snapshot read: hold the guard just long enough to clone the
    // fields out. A concurrent reload installs a new pointer
    // atomically; this reader walks the snapshot it started with.
    //
    // Snapshot the process environment once instead of calling
    // `std::env::var` per key per target — each call takes a global
    // mutex and copies the value out just to discard it. One HashMap
    // probe per key is cheaper and cannot race with a concurrent
    // `setenv` mid-response (same-request snapshot semantics).
    let env_keys: std::collections::HashSet<String> = std::env::vars().map(|(k, _)| k).collect();
    let engine = state.targets.load();
    let mut out: Vec<AdminTargetInfo> = engine
        .list_targets()
        .iter()
        .map(|t| {
            // Sort env keys for a stable UI order — the underlying
            // HashMap iteration order is undefined and would cause
            // the admin page to shuffle on every reload.
            let mut env: Vec<AdminTargetEnvKey> = t
                .env
                .keys()
                .map(|k| AdminTargetEnvKey {
                    key: k.clone(),
                    set: env_keys.contains(k),
                })
                .collect();
            env.sort_by(|a, b| a.key.cmp(&b.key));
            AdminTargetInfo {
                name: t.name.clone(),
                display: t.display.clone(),
                kind: t.kind,
                command: t.command.clone(),
                args: t.args.clone(),
                shell: t.shell.clone(),
                tags: t.tags.clone(),
                admin_only: t.admin_only,
                env,
                active_sessions: counts.get(&t.name).copied().unwrap_or(0),
            }
        })
        .collect();
    // Sort deterministically so the admin UI doesn't re-render in
    // a different order each poll.
    out.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(out))
}

/// `POST /api/admin/targets/reload`
///
/// Re-read `targets.yaml` from disk and atomically install the
/// resulting [`TargetEngine`] into [`AppState::targets`]. Admin-only.
///
/// Failure modes the admin UI needs to distinguish:
/// - 401: missing/bad bearer → login again
/// - 403: authenticated but not admin → no-op, hide the button
/// - 400 with `reason=no_targets_path`: operator never configured a
///   file, so there is nothing to re-read. The old engine (possibly
///   just the default `local-shell`) stays loaded.
/// - 400 with `reason=parse_error`: the file on disk is now
///   malformed; the old engine stays loaded and the response body
///   carries the parse error string so the admin can fix the yaml.
/// - 400 with `reason=still_referenced`: the parsed engine would
///   drop or rename at least one target that still has active
///   sessions pointing at it by name. The response body's
///   `targets` array lists `{target, active_sessions}` entries so
///   the admin UI can say "close these sessions first". The old
///   engine stays loaded. Without this guard a hot reload that
///   deletes a target would silently wedge every running session
///   on that target: the next WS reconnect resolves the missing
///   name, `cleanup_orphan_session` marks the DB row `Error`, and
///   the owner can never rejoin the still-running PTY.
/// - 200: swap succeeded; response carries the new target count
///   and the absolute path that was re-read, and an audit event
///   (`target.reloaded`) is emitted with the same payload.
#[derive(Debug, Default, Deserialize)]
pub struct ReloadTargetsBody {
    /// Hex SHA-256 of the `targets.yaml` bytes as seen by a preceding
    /// `/api/admin/targets/validate`. When present, the server reads
    /// the file a second time, hashes it, and refuses the reload with
    /// `reason: "file_changed"` if the hash has drifted — the admin
    /// approved diff A, we will not apply diff B. Omit to opt out
    /// (legacy CLI callers with no preview step).
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

pub async fn reload_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<ReloadTargetsBody>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_admin(&user)?;

    // The legacy CLI caller posts no body; only reject when a body
    // *was* provided but failed to parse. `MissingJsonContentType`
    // and `BytesRejection` both surface on no-body requests and are
    // treated as "opt out of the sha guard".
    let ReloadTargetsBody { expected_sha256 } = match body {
        Ok(Json(b)) => b,
        Err(JsonRejection::JsonDataError(_) | JsonRejection::JsonSyntaxError(_)) => {
            return Err(ApiError::bare(StatusCode::BAD_REQUEST));
        }
        Err(_) => ReloadTargetsBody::default(),
    };

    let Some(path) = state.targets_path.clone() else {
        // No configured targets file — nothing to reload. 400 so
        // the admin UI can show a clear "configure targets.yaml
        // first" message instead of a generic error toast.
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "reason": "no_targets_path",
                "message": "telepair was started without a targets.yaml; \
                            configure one and restart to enable hot-reload",
            })),
        ));
    };

    let parse_result = tokio::task::spawn_blocking({
        let path = path.clone();
        move || load_and_hash_targets(&path)
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "targets reload: spawn_blocking join error");
        ApiError::bare(StatusCode::INTERNAL_SERVER_ERROR)
    })?;

    let (new_engine, file_sha256) = match parse_result {
        Ok(Some((engine, sha))) => (engine, sha),
        Ok(None) => {
            // No-op success: the caller hit reload on a file that
            // doesn't exist yet (common on fresh installs). Keep the
            // current engine in place — swapping to an empty one would
            // silently drop targets an admin may have authored on an
            // earlier file that moved away.
            tracing::info!(
                path = %path.display(),
                actor = %user.name,
                "targets reload: file not present, no-op"
            );
            return Ok((
                StatusCode::OK,
                Json(serde_json::json!({
                    "reason": "no_targets_file",
                    "message": "no user-defined targets (targets.yaml not present); \
                                using built-in targets only",
                    "path": path.display().to_string(),
                })),
            ));
        }
        Err(err_msg) => {
            // Old engine stays loaded — `ArcSwap::store` is the only
            // site that replaces the pointer, and we haven't called
            // it yet. Surface the parse error verbatim so the admin
            // can see what line of yaml is wrong.
            tracing::warn!(
                path = %path.display(),
                error = %err_msg,
                "targets reload: parse failure, keeping previous engine"
            );
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "reason": "parse_error",
                    "message": err_msg,
                    "path": path.display().to_string(),
                })),
            ));
        }
    };

    // TOCTOU guard: if the caller previewed a specific file version
    // through validate, reject when the on-disk bytes no longer match.
    // The admin must re-run validate so the confirm dialog reflects
    // the current file. Case-insensitive because the wire value is
    // hex and some clients uppercase it.
    if let Some(expected) = expected_sha256.as_deref()
        && !expected.eq_ignore_ascii_case(&file_sha256)
    {
        tracing::warn!(
            path = %path.display(),
            actor = %user.name,
            expected = %expected,
            actual = %file_sha256,
            "targets reload: rejected — file changed since validate"
        );
        return Ok((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "reason": "file_changed",
                "message": "targets.yaml changed since validate; \
                            re-run validate before reloading",
                "expected_sha256": expected,
                "actual_sha256": file_sha256,
            })),
        ));
    }

    // Refuse to drop or rename a target that still has a **live**
    // session referencing it by name. Without this guard a reload
    // that removes `foo` from the yaml will wedge every running
    // session on `foo`: `ws::handle_socket` resolves target names
    // through the live engine, so the next reconnect would hit
    // `TARGET_NOT_FOUND` and `cleanup_orphan_session` would stamp
    // the DB row `Error`, leaving the owner unable to rejoin.
    //
    // This counts the hub, not the DB. The DB `sessions.status`
    // column can briefly report `active` for rows whose PTY has
    // already exited and are waiting for the reaper (or for the
    // cleanup branch of the PTY loop) to land the close. Blocking
    // a reload on those stale rows would prevent the admin from
    // rotating targets during the idle-grace window. The hub map,
    // on the other hand, is exactly the set of running shells: if
    // a `target_name` is present in the hub, there is a live PTY
    // that would break on reconnect without its yaml entry.
    //
    // This is a conservative gate: admins can still add targets,
    // delete unreferenced targets, or tweak `command/args/env` on
    // referenced targets — only the specific "drop or rename a
    // name still pointed at by a live session" case gets a 400.
    let live_counts = state.hub.count_live_sessions_per_target().await;
    let mut still_referenced: Vec<(String, u32)> = live_counts
        .into_iter()
        .filter(|(name, _)| new_engine.find(name).is_none())
        .collect();
    if !still_referenced.is_empty() {
        // Sort deterministically so the response body and the
        // warn log render in a stable order — HashMap iteration is
        // undefined and would shuffle between runs.
        still_referenced.sort_by(|a, b| a.0.cmp(&b.0));
        tracing::warn!(
            path = %path.display(),
            actor = %user.name,
            missing = ?still_referenced,
            "targets reload: rejected — new engine drops targets with live sessions"
        );
        let targets_json: Vec<_> = still_referenced
            .into_iter()
            .map(|(name, count)| {
                serde_json::json!({
                    "target": name,
                    "active_sessions": count,
                })
            })
            .collect();
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "reason": "still_referenced",
                "message": "refusing to drop or rename targets that still have live sessions",
                "targets": targets_json,
            })),
        ));
    }

    // Capture the count BEFORE the swap so the audit detail and the
    // HTTP response agree even if another admin races to reload.
    let new_count = new_engine.list_targets().len();
    state.targets.store(std::sync::Arc::new(new_engine));

    // Best-effort audit — a failed write logs and swallows so the
    // admin still sees the 200 for a successful swap.
    state
        .audit
        .record(
            telepair_core::audit::AuditEvent::new(
                telepair_core::audit::AuditEventType::TargetReloaded,
            )
            .with_actor(user.id, user.name.clone())
            .with_detail(serde_json::json!({
                "path": path.display().to_string(),
                "targets": new_count,
            })),
        )
        .await;

    tracing::info!(
        path = %path.display(),
        targets = new_count,
        actor = %user.name,
        "targets reloaded"
    );

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "path": path.display().to_string(),
            "targets": new_count,
        })),
    ))
}

/// `POST /api/admin/targets/validate`
///
/// Parse `targets.yaml` from disk and diff it against the in-memory engine
/// without applying any changes. Admin-only, read-only, safe to call at any
/// time. Useful for previewing what a subsequent `reload` would do.
///
/// Response shape:
/// - `valid: false` + `errors: [...]`: file is missing or unparseable; no
///   diff is produced.
/// - `valid: true` + `diff: {...}`: diff between current engine and the file
///   on disk. `blocked` lists removed targets that have active sessions —
///   a reload would fail for these, but validate does NOT block on them.
pub async fn validate_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_admin(&user)?;

    let Some(path) = state.targets_path.clone() else {
        return Ok(Json(serde_json::json!({
            "valid": false,
            "errors": ["No targets.yaml path configured. Start telepair with a targets file."],
        })));
    };

    let parse_result = tokio::task::spawn_blocking({
        let path = path.clone();
        move || load_and_hash_targets(&path)
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "targets validate: spawn_blocking join error");
        ApiError::bare(StatusCode::INTERNAL_SERVER_ERROR)
    })?;

    let (new_engine, expected_sha256) = match parse_result {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            return Ok(Json(serde_json::json!({
                "valid": false,
                "errors": [format!(
                    "targets.yaml not present at {}",
                    path.display()
                )],
            })));
        }
        Err(err) => {
            return Ok(Json(serde_json::json!({
                "valid": false,
                "errors": [err],
            })));
        }
    };

    let current_engine = state.targets.load();
    let diff = current_engine.diff(&new_engine);

    // Check for blocked removals (targets with active sessions)
    let live_counts = state.hub.count_live_sessions_per_target().await;
    let blocked: Vec<serde_json::Value> = diff
        .removed
        .iter()
        .filter_map(|name| {
            live_counts.get(name.as_str()).map(|&count| {
                serde_json::json!({
                    "target": name,
                    "active_sessions": count,
                })
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "valid": true,
        "path": path.display().to_string(),
        "total": new_engine.list_targets().len(),
        "diff": diff,
        "blocked": blocked,
        "expected_sha256": expected_sha256,
    })))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Read `targets.yaml` and parse into a `TargetEngine`, returning the hex
/// SHA-256 of the raw bytes alongside the engine. `Ok(None)` signals the
/// file is absent (fresh install — callers translate to a friendly
/// "no user-defined targets" response). Parsing from the same byte slice
/// used for the hash closes a TOCTOU window where a writer could rotate
/// the file between the hash and the parse.
fn load_and_hash_targets(
    path: &std::path::Path,
) -> Result<Option<(telepair_agent::virtual_target::TargetEngine, String)>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let sha = hex_sha256(&bytes);
    let yaml = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
    let engine = telepair_agent::virtual_target::TargetEngine::from_yaml(yaml)
        .map_err(|e| e.to_string())?;
    Ok(Some((engine, sha)))
}

// ── User-owned target CRUD ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UserTargetBody {
    pub name: Option<String>, // Only required on POST
    pub display: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// `POST /api/user-targets` — create a user-owned virtual target.
pub async fn create_user_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<UserTargetBody>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_unscoped(&user)?;
    let Json(body) = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;
    let name = body.name.ok_or(ApiError::bare(StatusCode::BAD_REQUEST))?;
    let target = state
        .user_targets
        .create(
            user.id,
            CreateTargetParams {
                name,
                display: body.display,
                command: body.command,
                args: body.args,
                env: body.env,
                tags: body.tags,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(target)))
}

/// `PUT /api/user-targets/{id}` — update a user-owned target (owner only).
pub async fn update_user_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<UserTargetBody>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_unscoped(&user)?;
    let Json(body) = body.map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;
    let target = state
        .user_targets
        .update(
            &id,
            user.id,
            UpdateTargetParams {
                display: body.display,
                command: body.command,
                args: body.args,
                env: body.env,
                tags: body.tags,
            },
        )
        .await?;
    Ok(Json(target))
}

/// `GET /api/user-targets/{id}` — fetch a single user-owned target (owner only).
pub async fn get_user_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_unscoped(&user)?;
    let target = state
        .user_targets
        .get(&id, user.id)
        .await?
        .ok_or(ApiError::bare(StatusCode::NOT_FOUND))?;
    Ok(Json(target))
}

/// `DELETE /api/user-targets/{id}` — delete a user-owned target (owner only).
pub async fn delete_user_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_unscoped(&user)?;
    state.user_targets.delete(&id, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Admin user management ────────────────────────────────────────────
//
// These endpoints back the admin Users page introduced in v0.1.2.
// They exist to let an operator flip `session_enabled` on a
// self-registered email account — the bit that the public signup
// flow forces FALSE and the `create_session` / WS attach gates
// read.
//
// Scoped guests are *not* listed by `list_accounts` — they are
// invite-minted, session-local, and disappear on close. A guest
// that somehow reaches this page would have nothing to do.

/// Wire shape for a row returned by `GET /api/admin/users`. This is
/// a subset of the internal `User` struct with the email surfaced
/// (the struct itself marks `email` as `#[serde(skip)]` because
/// every *other* endpoint must treat it as a sensitive identifier).
/// We accept that exposure here because the caller is already an
/// admin — they have full target-reload and session-close rights.
#[derive(Serialize)]
struct AdminUserInfo {
    id: String,
    name: String,
    email: Option<String>,
    is_admin: bool,
    session_enabled: bool,
    /// Admin-approval bucket. `"pending"` means the account
    /// completed OTP verification but is still waiting for an admin
    /// to flip `session_enabled = TRUE`. `"approved"` means it has
    /// been approved at some point (and may or may not currently be
    /// `session_enabled`).
    approval_state: &'static str,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<User> for AdminUserInfo {
    fn from(u: User) -> Self {
        Self {
            id: u.id.to_string(),
            name: u.name,
            email: u.email,
            is_admin: u.is_admin,
            session_enabled: u.session_enabled,
            approval_state: u.approval_state.as_str(),
            created_at: u.created_at,
            updated_at: u.updated_at,
        }
    }
}

/// Query params accepted by `GET /api/admin/users`.
#[derive(Deserialize)]
pub struct AdminUsersQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

/// `GET /api/admin/users` — admin-only. Lists every non-guest
/// account so the admin UI can render the approval page. Supports
/// optional filtering by name/email (`q`), account status, and
/// pagination (`limit`/`offset`). Returns `{ users: [...], total: N }`.
pub async fn list_admin_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminUsersQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_admin(&user)?;

    let status = query.status.as_deref().and_then(|s| match s {
        "enabled" => Some(AccountStatus::Enabled),
        "disabled" => Some(AccountStatus::Disabled),
        "pending" => Some(AccountStatus::Pending),
        _ => None,
    });

    let filter = AccountFilter {
        query: query.q.filter(|s| !s.is_empty()),
        status,
        limit: query.limit.filter(|&n| n > 0).unwrap_or(50).min(500),
        offset: query.offset.filter(|&n| n >= 0).unwrap_or(0),
    };

    let (rows, total) = state.auth_service.list_accounts_filtered(&filter).await?;
    let users: Vec<AdminUserInfo> = rows.into_iter().map(AdminUserInfo::from).collect();

    Ok(Json(serde_json::json!({
        "users": users,
        "total": total,
    })))
}

/// Shared plumbing for the enable / disable admin handlers. Auth,
/// admin gate, UUID parsing, self-mutation guard, and error
/// mapping are identical; the last step dispatches to the service
/// method for the requested value.
async fn set_user_enabled(
    state: &AppState,
    headers: &HeaderMap,
    target_id: &str,
    enabled: bool,
) -> Result<Json<AdminUserInfo>, ApiError> {
    let actor = extract_user(state, headers).await?;
    require_admin(&actor)?;

    // Parse the path param into a UUID up-front so a malformed id
    // reads as 400, not "user not found". The admin UI passes back
    // whatever `list_admin_users` returned, so this should never
    // fire in practice — but a typo in a curl probe should not
    // surface as 404.
    let target_uuid =
        uuid::Uuid::parse_str(target_id).map_err(|_| ApiError::bare(StatusCode::BAD_REQUEST))?;

    // Self-mutation guard: an admin disabling their own session
    // bit would lock themselves out of session creation on the
    // next request. The storage layer would happily honour it, so
    // the guard lives here. Enabling yourself is a no-op (admins
    // are never session-disabled in practice) but we reject it for
    // symmetry — nothing sensible calls this.
    if actor.id == target_uuid {
        // Emit the specific reason so clients can branch on it instead
        // of treating every self-mutation 400 as "malformed request".
        // Admin UIs use this to surface an explanatory toast; a bare
        // 400 with no body lets users think they mis-typed the id.
        return Err(ApiError::with_message(
            StatusCode::BAD_REQUEST,
            "cannot change your own account's session access",
        ));
    }

    let result = state
        .auth_service
        .set_session_access(actor.id, &actor.name, target_uuid, enabled)
        .await;

    let updated = result.map_err(|e| match e {
        Error::InvalidInput(_) => ApiError::bare(StatusCode::NOT_FOUND),
        other => ApiError::from(other),
    })?;

    Ok(Json(AdminUserInfo::from(updated)))
}

/// `POST /api/admin/users/{id}/enable` — admin-only. Flips
/// `session_enabled = TRUE` on the target row and audits the
/// mutation. 400 on self-mutation; 404 on unknown target id.
pub async fn enable_admin_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    set_user_enabled(&state, &headers, &id, true).await
}

/// `POST /api/admin/users/{id}/disable` — admin-only. Flips
/// `session_enabled = FALSE` on the target row and audits the
/// mutation. The target keeps their bearer token (whoami / history
/// still work); the next session create or WS attach they attempt
/// fails closed via the `session_enabled` gate.
pub async fn disable_admin_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    set_user_enabled(&state, &headers, &id, false).await
}

// --- Admin audit log ---

/// Query parameters for `GET /api/admin/audit`.
///
/// Every field is optional — the bare URL returns the latest 100 events.
/// `event_type` accepts a single dotted-lowercase type string (e.g.
/// `auth.login_failed`); invalid values are silently ignored so the UI
/// can reset a filter without crashing.
#[derive(Deserialize)]
pub struct AdminAuditQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
    /// RFC 3339 inclusive lower bound on `ts`.
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    /// RFC 3339 exclusive upper bound on `ts`.
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    /// Filter by actor UUID.
    #[serde(default)]
    pub actor_id: Option<String>,
    /// Single event type in dotted-lowercase form.
    #[serde(default)]
    pub event_type: Option<String>,
    /// Filter to events touching a specific session.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// `GET /api/admin/audit`
///
/// Global audit log, admin-only. Returns events newest-first with
/// optional filtering on time range, actor, event type, and session.
/// Default limit is 100 rows (enforced by `AuditFilter`), capped at
/// 500 to prevent accidental full-table dumps.
pub async fn list_admin_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminAuditQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_admin(&user)?;

    let actor_id = query
        .actor_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());

    let event_types: Vec<telepair_core::audit::AuditEventType> = query
        .event_type
        .as_deref()
        .and_then(|s| s.parse().ok())
        .into_iter()
        .collect();

    let limit = query
        .limit
        .filter(|&n| n > 0)
        .map(|n| n.min(500))
        .or(Some(100));

    let filter = telepair_core::audit::AuditFilter {
        since: query.since,
        until: query.until,
        actor_id,
        session_id: query.session_id.filter(|s| !s.is_empty()),
        event_types,
        limit,
        offset: query.offset.filter(|&n| n >= 0).unwrap_or(0),
    };

    let rows = state.audit.query(filter).await?;
    Ok(Json(rows))
}

// --- Admin audit export ---

#[derive(Deserialize)]
pub struct AuditExportQuery {
    pub format: Option<String>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

const EXPORT_MAX_ROWS: i64 = 10_000;

/// `GET /api/admin/audit/export` — admin-only. Exports audit logs as
/// JSON or CSV. Accepts the same filter parameters as the list endpoint.
/// Capped at 10,000 rows to prevent accidental full-table dumps.
pub async fn export_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AuditExportQuery>,
) -> Result<Response, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_admin(&user)?;

    let format = query.format.as_deref().unwrap_or("");
    if format != "json" && format != "csv" {
        return Err(ApiError::bare(StatusCode::BAD_REQUEST));
    }

    let actor_id = query
        .actor_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());

    let event_types: Vec<telepair_core::audit::AuditEventType> = query
        .event_type
        .as_deref()
        .and_then(|s| s.parse().ok())
        .into_iter()
        .collect();

    let filter = telepair_core::audit::AuditFilter {
        since: query.since,
        until: query.until,
        actor_id,
        session_id: query.session_id.filter(|s| !s.is_empty()),
        event_types,
        limit: Some(EXPORT_MAX_ROWS + 1), // fetch one extra to detect overflow
        offset: 0,
    };

    let rows = state.audit.query(filter).await?;

    if rows.len() as i64 > EXPORT_MAX_ROWS {
        return Err(ApiError::bare(StatusCode::PAYLOAD_TOO_LARGE));
    }

    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");

    if format == "csv" {
        // ~200 bytes/row is a rough upper bound for the fixed columns
        // (UUIDs + RFC3339 ts + enum + short names). `detail` can be
        // larger; `String` grows as needed, but this avoids the early
        // reallocations for the common case.
        let mut csv = String::with_capacity(64 + rows.len() * 200);
        csv.push_str("id,timestamp,event_type,actor_id,actor_name,session_id,detail\n");

        use std::fmt::Write as _;
        // RFC 4180 quoting handles commas / quotes / newlines, but does
        // NOT stop spreadsheet apps from evaluating cells that start with
        // `=`, `+`, `-`, `@`, TAB, or CR as formulas. Those prefixes are
        // an exfiltration vector when a user-controlled field (display
        // name, audit detail) lands in Excel / Numbers / Sheets. Prefix
        // a single quote to the raw cell BEFORE quoting so any leading
        // trigger character is neutralized by the spreadsheet.
        fn csv_cell(s: &str) -> String {
            let needs_guard = s
                .chars()
                .next()
                .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r'));
            let escaped = s.replace('"', "\"\"");
            if needs_guard {
                format!("\"'{escaped}\"")
            } else {
                format!("\"{escaped}\"")
            }
        }

        for row in &rows {
            let id = row.id.map(|i| i.to_string()).unwrap_or_default();
            let ts = row.ts.to_rfc3339();
            let event_type = row.event_type.as_str();
            let actor_id = row.actor_id.map(|u| u.to_string()).unwrap_or_default();
            let actor_name = csv_cell(row.actor_name.as_deref().unwrap_or(""));
            let session_id = csv_cell(row.session_id.as_deref().unwrap_or(""));
            let detail = if row.detail.is_null() {
                String::new()
            } else {
                csv_cell(&serde_json::to_string(&row.detail).unwrap_or_default())
            };
            // write! on String is infallible, but writeln/write returns fmt::Result
            let _ = writeln!(
                csv,
                "{id},{ts},{event_type},{actor_id},{actor_name},{session_id},{detail}"
            );
        }

        Ok(axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
            .header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"telepair-audit-{now}.csv\""),
            )
            .body(axum::body::Body::from(csv))
            .unwrap())
    } else {
        let json_bytes = serde_json::to_vec(&rows)
            .map_err(|_| ApiError::bare(StatusCode::INTERNAL_SERVER_ERROR))?;
        Ok(axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"telepair-audit-{now}.json\""),
            )
            .body(axum::body::Body::from(json_bytes))
            .unwrap())
    }
}

// --- Admin system info ---

/// `GET /api/admin/system` — admin-only. Returns a snapshot of
/// server-level diagnostics: version, filesystem paths, SMTP status,
/// live session count, registered user count, and uptime in seconds.
/// Intended for the admin UI's health overview and for operators who
/// want a quick sanity-check without SSH'ing into the box.
pub async fn system_info(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_admin(&user)?;

    let live_sessions = state.hub.active_count().await;
    // Use the filtered-list path with limit=0 so we get the COUNT(*)
    // back without materializing every user row into memory.
    let registered_users = state
        .auth_service
        .list_accounts_filtered(&AccountFilter {
            query: None,
            status: None,
            limit: 0,
            offset: 0,
        })
        .await
        .map(|(_, total)| total)
        .unwrap_or(0);

    let uptime = state.startup.elapsed().as_secs();
    let db_path = state.data_dir.join("telepair.db");

    Ok(Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "data_dir": state.data_dir.display().to_string(),
        "db_path": db_path.display().to_string(),
        "targets_path": state.targets_path.as_ref().map(|p| p.display().to_string()),
        "smtp_configured": state.smtp_configured,
        "live_sessions": live_sessions,
        "registered_users": registered_users,
        "uptime_seconds": uptime,
    })))
}
