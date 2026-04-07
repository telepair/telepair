use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use telepair_core::error::Error;
use telepair_core::permission::Role;
use telepair_core::session::{InputMode, Session, User};
use telepair_core::storage::Storage;

use crate::state::AppState;

/// Lift a `core::Error` into the right HTTP status. Handlers should use
/// this instead of hand-written `map_err(|_| StatusCode::X)` so that
/// `InvalidInput` never leaks out as 500 and authorization failures
/// always come back as 401 / 403, even when the underlying call site
/// only knows it got "some error".
fn status_for(err: &Error) -> StatusCode {
    StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

// --- Auth extraction ---

pub async fn extract_user(state: &AppState, headers: &HeaderMap) -> Result<User, StatusCode> {
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    state
        .auth
        .validate(token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)
}

/// Reject invite-minted guests on account-level routes. A scoped
/// guest token is only valid for its bound session — it must not be
/// usable to enumerate targets, spin up new sessions, or otherwise
/// behave like a real account. 403 (not 401) because the caller is
/// authenticated, they just don't have the scope for this route.
fn require_unscoped(user: &User) -> Result<(), StatusCode> {
    if user.is_guest() {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Run auth extraction and session lookup concurrently, then verify the
/// authenticated user owns the session. Shaves one DB-query latency off
/// each authenticated single-session endpoint.
async fn extract_user_and_owned_session(
    state: &AppState,
    headers: &HeaderMap,
    session_id: &str,
) -> Result<(User, Session), StatusCode> {
    let (user_res, session_res) = tokio::join!(
        extract_user(state, headers),
        state.sessions.storage().get_session(session_id),
    );
    let user = user_res?;
    let session = session_res
        .map_err(|e| status_for(&e))?
        .ok_or(StatusCode::NOT_FOUND)?;
    if session.owner_id != user.id {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok((user, session))
}

// --- Handlers ---

pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

pub async fn list_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
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
) -> Result<impl IntoResponse, StatusCode> {
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
    let Json(body) = body.map_err(|_| StatusCode::BAD_REQUEST)?;

    // Verify target exists and enforce admin-only restriction
    let target = state
        .targets
        .find(&body.target_name)
        .ok_or(StatusCode::NOT_FOUND)?;

    if target.admin_only && !user.is_admin {
        return Err(StatusCode::FORBIDDEN);
    }

    let mode = body.input_mode.unwrap_or(InputMode::Multiplexed);

    let session = state
        .sessions
        .create_session(user.id, &body.target_name, mode)
        .await
        .map_err(|e| status_for(&e))?;

    Ok((StatusCode::CREATED, Json(session)))
}

pub async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    let user = extract_user(&state, &headers).await?;
    let visible = state
        .sessions
        .list_sessions_for_user(user.id)
        .await
        .map_err(|e| status_for(&e))?;

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

/// Hard cap on invite `max_uses`. An invite that can be redeemed 10k
/// times is not an invite, it's a public URL — reject those at the
/// HTTP layer so a typo in the UI can't produce one by accident.
const MAX_INVITE_USES: i32 = 100;

/// Hard cap on invite TTL. Week-long invites are already pushing it;
/// month-long invites are a credential-handling mistake waiting to
/// happen. Any request asking for longer is clamped to this value.
const MAX_INVITE_TTL_MINUTES: i64 = 7 * 24 * 60;

pub async fn create_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Result<Json<CreateInviteRequest>, JsonRejection>,
) -> Result<impl IntoResponse, StatusCode> {
    extract_user_and_owned_session(&state, &headers, &session_id).await?;

    // Axum's default JSON rejection is 422; every other handler in this
    // file remaps to 400 so clients get a consistent "you sent garbage"
    // signal regardless of which field was wrong.
    let Json(body) = body.map_err(|_| StatusCode::BAD_REQUEST)?;

    // Only operator and viewer roles can be invited
    if body.role == Role::Owner {
        return Err(StatusCode::BAD_REQUEST);
    }

    if body.max_uses < 1 || body.max_uses > MAX_INVITE_USES {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Resolve expiry: absolute timestamp wins over TTL, both are
    // optional, negative/huge TTLs are clamped rather than rejected so
    // a slider or number-field overshoot produces a useful invite.
    let expires_at = match (body.expires_at, body.expires_in_minutes) {
        (Some(at), _) => {
            if at <= Utc::now() {
                return Err(StatusCode::BAD_REQUEST);
            }
            Some(at)
        }
        (None, Some(minutes)) if minutes > 0 => {
            let clamped = minutes.min(MAX_INVITE_TTL_MINUTES);
            Some(Utc::now() + Duration::minutes(clamped))
        }
        (None, Some(_)) => {
            // Zero or negative TTL is not a thing — reject loudly rather
            // than silently creating a "never-expires" invite.
            return Err(StatusCode::BAD_REQUEST);
        }
        (None, None) => None,
    };

    let role = body.role;

    let (invite, raw_token) = state
        .sessions
        .storage()
        .create_invite(&session_id, role, body.max_uses, expires_at)
        .await
        .map_err(|e| status_for(&e))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "token": raw_token,
            "role": invite.role,
            "max_uses": invite.max_uses,
            "expires_at": invite.expires_at,
            "session_id": session_id,
        })),
    ))
}

#[derive(Deserialize)]
pub struct RedeemInviteRequest {
    pub token: String,
}

pub async fn close_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    extract_user_and_owned_session(&state, &headers, &session_id).await?;
    state
        .sessions
        .close_session(&session_id)
        .await
        .map_err(|e| status_for(&e))?;
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
) -> Result<impl IntoResponse, StatusCode> {
    // Best-effort auth: a bearer token is no longer required. We try
    // to validate it so a logged-in user reuses their identity, but
    // a missing/invalid token falls through to the guest path instead
    // of failing the whole request.
    let existing_user = match extract_user(&state, &headers).await {
        Ok(u) => Some(u),
        Err(StatusCode::UNAUTHORIZED) => None,
        Err(status) => return Err(status),
    };

    // Keep the JSON rejection semantics consistent across the handlers:
    // a malformed body is a 400, not a 422. This matters for the
    // frontend's error-handling code which branches on "bad request"
    // (show form error) vs "server error" (show toast + retry) — the
    // old 422 made bogus redeems look like a server crash.
    let Json(body) = body.map_err(|_| StatusCode::BAD_REQUEST)?;

    // Look up the invite first (no state change) so we can validate
    // the session is still alive before burning a use on a closed
    // session. Without this check, redeeming against a closed session
    // would decrement `max_uses` and insert a ghost participant — the
    // invite counter drains and nothing useful happens.
    let storage = state.sessions.storage();
    let preview = storage
        .find_invite(&body.token)
        .await
        .map_err(|e| status_for(&e))?;

    // Scoped-guest cross-session redeem: a guest whose token was
    // issued for session A must not be able to redeem an invite for
    // session B and pivot their identity. If the authenticated caller
    // is scoped, the invite's target session must match their scope
    // — otherwise reject outright with 403. (They can still reach the
    // intended session by opening the invite link in a fresh tab
    // without credentials, which mints a new, correctly-scoped
    // guest.) Must fire before `consume_invite` so we don't burn a
    // use on a rejected attempt.
    if let Some(ref user) = existing_user
        && let Some(ref scope) = user.scoped_session_id
        && preview.session_id != *scope
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let session = storage
        .get_session(&preview.session_id)
        .await
        .map_err(|e| status_for(&e))?
        .ok_or(StatusCode::NOT_FOUND)?;
    if session.status != telepair_core::session::SessionStatus::Active {
        // Gone / closed — fail without consuming the invite so the
        // operator can still retract it or reuse uses on a new session.
        return Err(StatusCode::GONE);
    }

    // Finding #8: if the authenticated caller is already a member of
    // this session (owner or any role), treat the redeem as a no-op
    // that **does not** consume the invite. Before this check, an
    // owner clicking their own share link to sanity-check it would
    // silently burn one of the `max_uses` and often drop the invite
    // to zero remaining uses before any guest saw it.
    if let Some(ref user) = existing_user {
        let participants = storage
            .list_participants(&preview.session_id)
            .await
            .map_err(|e| status_for(&e))?;
        if let Some(existing) = participants.iter().find(|p| p.user_id == user.id) {
            return Ok(Json(serde_json::json!({
                "session_id": preview.session_id,
                "role": existing.role,
                "token": serde_json::Value::Null,
            })));
        }
    }

    // Now consume atomically. This validates expiry, max_uses, and
    // increments used_count in one transaction. A session close that
    // races in between the check above and this call is possible but
    // harmless — the participant insert below just won't be visible
    // because the stopped session's in-memory hub entry is gone.
    let invite = storage
        .consume_invite(&body.token)
        .await
        .map_err(|e| status_for(&e))?;

    // Decide which user joins: reuse the authenticated caller, or
    // mint a fresh guest. Guests are only created **after** the
    // invite was successfully consumed, so a rejected invite can
    // never leave an orphan user behind. The guest's credentials are
    // scoped to `invite.session_id` so the resulting bearer token
    // cannot be used to hit account-level routes or connect to any
    // other session's WS endpoint.
    let (user, issued_token) = match existing_user {
        Some(u) => (u, None),
        None => {
            let (guest, raw_token) = state
                .auth
                .create_guest(&invite.session_id)
                .await
                .map_err(|e| status_for(&e))?;
            (guest, Some(raw_token))
        }
    };

    storage
        .upsert_participant(&invite.session_id, user.id, invite.role)
        .await
        .map_err(|e| status_for(&e))?;

    Ok(Json(serde_json::json!({
        "session_id": invite.session_id,
        "role": invite.role,
        "token": issued_token,
    })))
}
