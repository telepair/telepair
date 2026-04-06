use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use telepair_core::permission::Role;
use telepair_core::session::{InputMode, Session, User};
use telepair_core::storage::Storage;

use crate::state::AppState;

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

    // Verify target exists and check required_role
    let target = state
        .targets
        .find(&body.target_name)
        .ok_or(StatusCode::NOT_FOUND)?;

    if let Some(required) = &target.required_role {
        if !user.is_admin && *required != Role::Viewer {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    let mode = match body.input_mode.as_deref() {
        Some("multiplexed") => InputMode::Multiplexed,
        _ => InputMode::Serialized,
    };

    let session = state
        .sessions
        .create_session(user.id, &body.target_name, mode)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    state.hub.stop_session(&session_id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn redeem_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RedeemInviteRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = extract_user(&state, &headers).await?;

    // Consume atomically first — validates expiry, max_uses, and increments used_count.
    // If this fails, no participant is added.
    let invite = state
        .sessions
        .storage()
        .consume_invite(&body.token)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Only now upsert participant — invite was valid and consumed
    state
        .sessions
        .storage()
        .upsert_participant(&invite.session_id, user.id, invite.role)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "session_id": invite.session_id,
        "role": invite.role.as_str(),
    })))
}
