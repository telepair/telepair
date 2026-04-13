use axum::{
    Json,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use telepair_control::invite_service::CreateInviteParams;
use telepair_control::user_target_service::{CreateTargetParams, UpdateTargetParams};
use telepair_core::error::Error;
use telepair_core::permission::Role;
use telepair_core::session::{InputMode, SessionListFilter, SessionStatus, User};
use telepair_core::target::TargetKind;

use crate::state::AppState;

/// Handler-level error wrapper. `?` on any `Result<_, core::Error>`
/// lifts via `From`, so `InvalidInput` never leaks out as 500 and auth
/// failures always surface as 401/403. `StatusCode` also lifts in, for
/// the handful of sites that short-circuit with a hard-coded status
/// (e.g. `return Err(StatusCode::BAD_REQUEST.into())` on body validation).
pub struct ApiError(StatusCode);

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

impl From<StatusCode> for ApiError {
    fn from(s: StatusCode) -> Self {
        Self(s)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}

// --- Auth extraction ---

pub async fn extract_user(state: &AppState, headers: &HeaderMap) -> Result<User, ApiError> {
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError(StatusCode::UNAUTHORIZED))?;

    Ok(state.auth.validate(token).await?)
}

/// Reject invite-minted guests on account-level routes. A scoped
/// guest token is only valid for its bound session — it must not be
/// usable to enumerate targets, spin up new sessions, or otherwise
/// behave like a real account. 403 (not 401) because the caller is
/// authenticated, they just don't have the scope for this route.
fn require_unscoped(user: &User) -> Result<(), ApiError> {
    if user.is_guest() {
        return Err(ApiError(StatusCode::FORBIDDEN));
    }
    Ok(())
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
/// Returns 503 if SMTP is not configured, 409 if email is taken.
pub async fn register(
    State(state): State<AppState>,
    body: Result<Json<RegisterRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(body) = body.map_err(|_| ApiError(StatusCode::BAD_REQUEST))?;
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
    let Json(body) = body.map_err(|_| ApiError(StatusCode::BAD_REQUEST))?;
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
    let Json(body) = body.map_err(|_| ApiError(StatusCode::BAD_REQUEST))?;
    let token = if let Some(t) = body.token {
        // Validate existing bearer token (admin / guest path).
        state.auth.validate(&t).await?;
        t
    } else if let (Some(email), Some(password)) = (body.email, body.password) {
        state.auth_service.login(&email, &password).await?
    } else {
        return Err(ApiError(StatusCode::BAD_REQUEST));
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
        name: user.name,
    }))
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

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub target_name: String,
    /// Strict parse: unknown values are rejected by axum's JSON extractor
    /// with a 400 so typos are loud. Omitted field defaults to
    /// `InputMode::Multiplexed` below — the collaborative default so
    /// invited operators can actually type, which is the whole point of
    /// "Google Docs for terminals". Owners who want a solo shell with
    /// shoulder-surfing viewers can still opt into `serialized`.
    #[serde(default)]
    pub input_mode: Option<InputMode>,
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

    // Axum's default JSON rejection is 422; we want 400 so an unknown
    // `input_mode` value reads as "client sent garbage" instead of
    // "server doesn't know what to do with it".
    let Json(body) = body.map_err(|_| ApiError(StatusCode::BAD_REQUEST))?;

    // Verify target exists and enforce admin-only restriction.
    // Global targets (from targets.yaml) take priority; user-owned targets
    // are checked as a fallback when the global engine misses.
    let (admin_only, user_target_id) = {
        let engine = state.targets.load();
        match engine.find(&body.target_name) {
            Some(t) => (t.admin_only, None),
            None => {
                // Check caller's user-owned targets
                let user_ts = state.user_targets.list(user.id).await?;
                match user_ts.into_iter().find(|ut| ut.name == body.target_name) {
                    Some(ut) => (false, Some(ut.id)),
                    None => {
                        return Err(ApiError(StatusCode::NOT_FOUND));
                    }
                }
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
                .with_detail(serde_json::json!({ "target_name": body.target_name })),
            )
            .await;
        return Err(ApiError(StatusCode::FORBIDDEN));
    }

    let mode = body.input_mode.unwrap_or(InputMode::Multiplexed);

    let session = state
        .sessions
        .create_session_with_user_target(
            &user,
            &body.target_name,
            mode,
            user_target_id.as_deref(),
        )
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
    state
        .hub
        .reserve_target(&session.id, &body.target_name)
        .await;

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
    /// Optional TTL in minutes — mutually exclusive with `expires_at`.
    /// The UI uses this because it's easier than picking an absolute
    /// wall-clock time in a form; the backend resolves it to an absolute
    /// `DateTime<Utc>` before hitting storage so the DB only ever sees
    /// concrete timestamps.
    #[serde(default)]
    pub expires_in_minutes: Option<i64>,
    /// Optional absolute expiry. If both `expires_in_minutes` and
    /// `expires_at` are set, this wins — callers shouldn't pass both
    /// but if they do we prefer the one with less ambiguity.
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

    // Axum's default JSON rejection is 422; every other handler in this
    // file remaps to 400 so clients get a consistent "you sent garbage"
    // signal regardless of which field was wrong.
    let Json(body) = body.map_err(|_| ApiError(StatusCode::BAD_REQUEST))?;

    // Everything else — ownership, alive gate, role/max_uses/TTL
    // validation, token mint — lives inside `InviteService::create`.
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
        Err(ApiError(StatusCode::UNAUTHORIZED)) => None,
        Err(other) => return Err(other),
    };

    // Keep the JSON rejection semantics consistent across the handlers:
    // a malformed body is a 400, not a 422. This matters for the
    // frontend's error-handling code which branches on "bad request"
    // (show form error) vs "server error" (show toast + retry) — the
    // old 422 made bogus redeems look like a server crash.
    let Json(body) = body.map_err(|_| ApiError(StatusCode::BAD_REQUEST))?;

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
/// Hard-deletes the invite row. Owner-only; the path-parameter session
/// id must match what the invite points at (mismatch surfaces as 404 so
/// a caller can't probe for invites belonging to other sessions).
/// Double-revoke returns 404 — the UI treats that as "already gone" and
/// refreshes its list.
pub async fn revoke_session_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, token_sha256)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
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
        return Err(ApiError(StatusCode::FORBIDDEN));
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
pub async fn reload_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    require_admin(&user)?;

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

    // Parse in a blocking context so a pathologically large yaml
    // doesn't stall the tokio worker. `TargetEngine::from_file` reads
    // the file synchronously; spawn_blocking keeps the runtime healthy.
    let path_for_blocking = path.clone();
    let parse_result = tokio::task::spawn_blocking(move || {
        telepair_agent::virtual_target::TargetEngine::from_file(&path_for_blocking)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "targets reload: spawn_blocking join error");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR)
    })?;

    let new_engine = match parse_result {
        Ok(engine) => engine,
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
    let Json(body) = body.map_err(|_| ApiError(StatusCode::BAD_REQUEST))?;
    let name = body.name.ok_or(ApiError(StatusCode::BAD_REQUEST))?;
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
    let Json(body) = body.map_err(|_| ApiError(StatusCode::BAD_REQUEST))?;
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
