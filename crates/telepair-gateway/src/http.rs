use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use telepair_core::session::{InputMode, User};
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

    // Verify target exists
    if state.targets.resolve(&body.target_name).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Check required_role if set on the target
    if let Some(target) = state
        .targets
        .list_targets()
        .iter()
        .find(|t| t.name == body.target_name)
    {
        if let Some(required) = &target.required_role {
            // Admin users are always allowed
            if !user.is_admin {
                // Non-admin users are only allowed if required_role is Viewer
                if *required != telepair_core::permission::Role::Viewer {
                    return Err(StatusCode::FORBIDDEN);
                }
            }
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
    let _user = extract_user(&state, &headers).await?;
    let sessions = state
        .sessions
        .list_active_sessions()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(sessions))
}

// --- Invite handlers ---

#[derive(Deserialize)]
pub struct CreateInviteRequest {
    pub role: String,
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
    let user = extract_user(&state, &headers).await?;

    let session = state
        .sessions
        .storage()
        .get_session(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if session.owner_id != user.id {
        return Err(StatusCode::FORBIDDEN);
    }

    let role = match body.role.as_str() {
        "operator" => telepair_core::permission::Role::Operator,
        "viewer" => telepair_core::permission::Role::Viewer,
        _ => return Err(StatusCode::BAD_REQUEST),
    };

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
    let user = extract_user(&state, &headers).await?;
    let session = state
        .sessions
        .storage()
        .get_session(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if session.owner_id != user.id {
        return Err(StatusCode::FORBIDDEN);
    }
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

    // Validate first (does not consume)
    let invite = state
        .sessions
        .storage()
        .validate_invite(&body.token)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    // Upsert participant (idempotent — safe if user already has a row)
    state
        .sessions
        .storage()
        .upsert_participant(&invite.session_id, user.id, invite.role)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Then consume the invite (increment used_count)
    state
        .sessions
        .storage()
        .consume_invite(&body.token)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    Ok(Json(serde_json::json!({
        "session_id": invite.session_id,
        "role": invite.role.as_str(),
    })))
}
