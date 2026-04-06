use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
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
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
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
    let _user = extract_user(&state, &headers).await?;

    #[derive(Serialize)]
    struct TargetInfo {
        name: String,
        display: String,
        tags: Vec<String>,
    }

    let targets: Vec<TargetInfo> = state
        .targets
        .list_targets()
        .iter()
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
    #[serde(default)]
    pub input_mode: Option<String>,
}

pub async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateSessionRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = extract_user(&state, &headers).await?;

    // Verify target exists and enforce admin-only restriction
    let target = state
        .targets
        .find(&body.target_name)
        .ok_or(StatusCode::NOT_FOUND)?;

    if target.admin_only && !user.is_admin {
        return Err(StatusCode::FORBIDDEN);
    }

    // Strict parse: unknown values used to silently collapse to
    // Serialized, which masked client bugs and could surprise the
    // caller with the wrong input semantics. Return 400 instead so
    // typos are loud.
    let mode = match body.input_mode.as_deref() {
        None => InputMode::Serialized,
        Some(raw) => raw
            .parse::<InputMode>()
            .map_err(|_| StatusCode::BAD_REQUEST)?,
    };

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
}

fn default_max_uses() -> i32 {
    1
}

pub async fn create_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<CreateInviteRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    extract_user_and_owned_session(&state, &headers, &session_id).await?;

    // Only operator and viewer roles can be invited
    if body.role == Role::Owner {
        return Err(StatusCode::BAD_REQUEST);
    }

    let role = body.role;

    let (invite, raw_token) = state
        .sessions
        .storage()
        .create_invite(&session_id, role, body.max_uses, None)
        .await
        .map_err(|e| status_for(&e))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "token": raw_token,
            "role": invite.role.as_str(),
            "max_uses": invite.max_uses,
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
    Json(body): Json<RedeemInviteRequest>,
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

    // Look up the invite first (no state change) so we can validate
    // the session is still alive before burning a use on a closed
    // session. Without this check, redeeming against a closed session
    // would decrement `max_uses` and insert a ghost participant — the
    // invite counter drains and nothing useful happens.
    let storage = state.sessions.storage();
    let preview = storage
        .find_invite(&body.token)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
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

    // Now consume atomically. This validates expiry, max_uses, and
    // increments used_count in one transaction. A session close that
    // races in between the check above and this call is possible but
    // harmless — the participant insert below just won't be visible
    // because the stopped session's in-memory hub entry is gone.
    let invite = storage
        .consume_invite(&body.token)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Decide which user joins: reuse the authenticated caller, or
    // mint a fresh guest. Guests are only created **after** the
    // invite was successfully consumed, so a rejected invite can
    // never leave an orphan user behind.
    let (user, issued_token) = match existing_user {
        Some(u) => (u, None),
        None => {
            let (guest, raw_token) = state
                .auth
                .create_guest()
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
        "role": invite.role.as_str(),
        "token": issued_token,
    })))
}
