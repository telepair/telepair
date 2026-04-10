use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use telepair_control::invite_service::{CreateInviteParams, CreateInviteResult, RedeemResult};
use telepair_core::error::Error;
use telepair_core::permission::Role;
use telepair_core::session::{InputMode, Session, User};

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

/// Run auth extraction and session lookup concurrently, then verify the
/// authenticated user owns the session. Shaves one DB-query latency off
/// each authenticated single-session endpoint.
///
/// Both halves go through `SessionService` so the ownership rule lives
/// exactly once, in one place: [`SessionService::require_owner`] emits
/// the right 404/403 distinction so the HTTP layer stays transport-only.
async fn extract_owned_session(
    state: &AppState,
    headers: &HeaderMap,
    session_id: &str,
) -> Result<Session, ApiError> {
    let (user_res, session_res) = tokio::join!(
        extract_user(state, headers),
        state.sessions.get_session(session_id),
    );
    let user = user_res?;
    // `session_res` is `Result<Option<Session>>`; surface any storage
    // error through `?` (→ 500) and collapse `None` to 404.
    let session = session_res?.ok_or(ApiError(StatusCode::NOT_FOUND))?;
    if session.owner_id != user.id {
        return Err(ApiError(StatusCode::FORBIDDEN));
    }
    Ok(session)
}

// --- Handlers ---

pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
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
    let is_admin = user.is_admin;
    let targets: Vec<TargetInfo> = state
        .targets
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

    // Verify target exists and enforce admin-only restriction
    let target = state
        .targets
        .find(&body.target_name)
        .ok_or(ApiError(StatusCode::NOT_FOUND))?;

    if target.admin_only && !user.is_admin {
        return Err(ApiError(StatusCode::FORBIDDEN));
    }

    let mode = body.input_mode.unwrap_or(InputMode::Multiplexed);

    let session = state
        .sessions
        .create_session(user.id, &body.target_name, mode)
        .await?;

    Ok((StatusCode::CREATED, Json(session)))
}

pub async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = extract_user(&state, &headers).await?;
    let visible = state.sessions.list_sessions_for_user(user.id).await?;

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
    extract_owned_session(&state, &headers, &session_id).await?;
    state.sessions.close_session(&session_id).await?;
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
