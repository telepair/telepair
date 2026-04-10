use axum::{
    Json,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use telepair_control::invite_service::{CreateInviteParams, CreateInviteResult, RedeemResult};
use telepair_core::error::Error;
use telepair_core::permission::Role;
use telepair_core::session::{CloseReason, InputMode, SessionListFilter, SessionStatus, User};

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

/// `GET /api/auth/whoami`
///
/// Returns the authenticated caller's identity. Used by the frontend
/// auth store to cache `currentUserId` so the dashboard can decide
/// per-row whether the caller owns the session — closed rows on
/// non-owned sessions stay inert (the audit dialog is owner-only and
/// would otherwise produce a deterministic 403). 401 on missing or
/// invalid bearer; never returns 403, since "I am a guest" is still a
/// valid identity to surface.
pub async fn whoami(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    Ok(Json(serde_json::json!({
        "user_id": user.id.to_string(),
        "name": user.name,
        "is_admin": user.is_admin,
        "is_guest": user.is_guest(),
    })))
}

pub async fn list_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    // Guests are scoped to a single session and have no dashboard —
    // they must never see a target list at all. (Separate finding
    // from the info-leak fix below: before this the handler didn't
    // even check authentication scope.)
    require_unscoped(&user)?;

    #[derive(Serialize)]
    struct TargetInfo {
        name: String,
        display: String,
        tags: Vec<String>,
    }

    // Info-leak fix: `admin_only` targets must not be enumerable by
    // non-admin callers. Before this filter, a regular user could
    // still `GET /api/targets` and read the full set of admin-only
    // target names / display strings / tags — names in the wild
    // often encode environment info (e.g. `prod-payments-db`), so
    // leaking the list is itself the problem, not just "users see a
    // button they can't click". `create_session` still enforces the
    // same rule, so this is a defence-in-depth narrowing of the
    // response, not the sole gate.
    //
    // `load()` is wait-free — the hot-reload admin endpoint may be
    // concurrently installing a new engine, but this reader walks a
    // consistent snapshot for the duration of the call.
    let is_admin = user.is_admin;
    let engine = state.targets.load();
    let targets: Vec<TargetInfo> = engine
        .list_targets()
        .iter()
        .filter(|t| is_admin || !t.admin_only)
        .map(|t| TargetInfo {
            name: t.name.clone(),
            display: t.display.clone(),
            tags: t.tags.clone(),
        })
        .collect();

    Ok(Json(targets))
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
    // Hold the ArcSwap guard only as long as needed to read the two
    // fields we care about; clone them out so a concurrent reload
    // doesn't extend the guard's lifetime through the rest of the
    // handler.
    let (target_exists, admin_only) = {
        let engine = state.targets.load();
        match engine.find(&body.target_name) {
            Some(t) => (true, t.admin_only),
            None => (false, false),
        }
    };
    if !target_exists {
        return Err(ApiError(StatusCode::NOT_FOUND));
    }

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
        .create_session(&user, &body.target_name, mode)
        .await?;

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
    let visible = state
        .sessions
        .list_sessions_for_user(user.id, query.into_filter())
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

    Ok((StatusCode::CREATED, Json(invite_response(&result))))
}

fn invite_response(r: &CreateInviteResult) -> serde_json::Value {
    serde_json::json!({
        "token": r.token,
        "role": r.role,
        "max_uses": r.max_uses,
        "expires_at": r.expires_at,
        "session_id": r.session_id,
    })
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
    // We need the User, not just the session, so the audit emit in
    // `close_session` can attribute the close to the actor. Auth and
    // the session lookup run concurrently — saves one DB-query's
    // worth of latency on the happy path without rewriting the
    // ownership check into an opaque helper.
    let (user_res, session_res) = tokio::join!(
        extract_user(&state, &headers),
        state.sessions.get_session(&session_id),
    );
    let user = user_res?;
    let session = session_res?.ok_or(ApiError(StatusCode::NOT_FOUND))?;
    if session.owner_id != user.id {
        return Err(ApiError(StatusCode::FORBIDDEN));
    }
    // Owner-initiated close → CloseReason::Owner. The reaper and the
    // boot-time cleanup paths stamp their own reasons when they call
    // close_session; the history view reads that column to pick the
    // right chip ("Owner closed" / "Timed out" / "Server restart").
    state
        .sessions
        .close_session(&session_id, CloseReason::Owner, Some(&user))
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
    // participant upsert — lives inside `InviteService::redeem`. The
    // HTTP layer translates the `RedeemResult` into the wire shape.
    let result = state.invites.redeem(existing_user, &body.token).await?;
    Ok(Json(redeem_response(&result)))
}

fn redeem_response(r: &RedeemResult) -> serde_json::Value {
    serde_json::json!({
        "session_id": r.session_id,
        "role": r.role,
        "token": r.issued_token,
    })
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
    #[serde(rename = "type")]
    kind: String,
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
                    set: std::env::var(k).is_ok(),
                })
                .collect();
            env.sort_by(|a, b| a.key.cmp(&b.key));
            AdminTargetInfo {
                name: t.name.clone(),
                display: t.display.clone(),
                kind: match t.kind {
                    telepair_core::target::TargetKind::Virtual => "virtual".into(),
                    telepair_core::target::TargetKind::Local => "local".into(),
                },
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
