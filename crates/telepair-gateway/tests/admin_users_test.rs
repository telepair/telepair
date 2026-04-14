//! Integration tests for the admin user management endpoints.
//!
//! These cover the HTTP surface that backs the Admin → Users page:
//!
//! - `GET  /api/admin/users`
//! - `POST /api/admin/users/{id}/enable`
//! - `POST /api/admin/users/{id}/disable`
//!
//! The endpoints back the approval flow for self-served email
//! signups (the v0.1.2 critical adversarial fix). A fresh signup
//! lands with `session_enabled = FALSE` and is inert until an admin
//! flips the bit through one of these routes.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::audit::{AuditEventType, AuditFilter};
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router_with_options};

/// Seed an admin, a regular user, and a "pending" user whose
/// `session_enabled` bit is FALSE — the state a fresh email signup
/// lands in before an admin clicks approve. Returns the router,
/// the three raw tokens, and the pending user's id so the test can
/// target it in path params.
async fn setup() -> (
    axum::Router,
    String,
    String,
    String,
    uuid::Uuid,
    Arc<SqliteStorage>,
) {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (admin, admin_token) = storage.create_user("admin", true).await.unwrap();
    let _ = admin;
    let (_regular, regular_token) = storage.create_user("regular", false).await.unwrap();
    let (pending, pending_token) = storage.create_user("pending", false).await.unwrap();
    // Flip the pending user's bit to FALSE to simulate a
    // post-verify state awaiting admin approval. `create_user` seeds
    // TRUE; the email signup path is what writes FALSE.
    storage
        .set_session_enabled(pending.id, false)
        .await
        .unwrap();

    let state = AppState::new(
        storage.clone(),
        TargetEngine::empty(),
        None,
        None,
        std::path::PathBuf::from("/tmp/telepair-test"),
    )
    .await;
    let router = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();
    (
        router,
        admin_token,
        regular_token,
        pending_token,
        pending.id,
        storage,
    )
}

async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

// ── list_admin_users ──────────────────────────────────────────────────

#[tokio::test]
async fn list_admin_users_unauthenticated_is_401() {
    let (app, _, _, _, _, _) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_admin_users_non_admin_is_403() {
    let (app, _, regular, _, _, _) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/users")
                .header("Authorization", format!("Bearer {regular}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_admin_users_returns_every_account_with_session_enabled_flag() {
    let (app, admin, _, _, pending_id, _) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/users")
                .header("Authorization", format!("Bearer {admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = body_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    let arr = body["users"].as_array().expect("expected users array");
    assert_eq!(body["total"].as_i64().unwrap(), 3);
    // 3 seeded accounts: admin, regular, pending. Scoped guests
    // should NOT appear — we seeded none here.
    assert_eq!(arr.len(), 3, "got {body}");

    let pending_row = arr
        .iter()
        .find(|u| u["id"] == pending_id.to_string())
        .expect("pending user missing from list");
    assert_eq!(pending_row["session_enabled"], false);
    assert_eq!(pending_row["name"], "pending");

    let admin_row = arr
        .iter()
        .find(|u| u["name"] == "admin")
        .expect("admin missing");
    assert_eq!(admin_row["is_admin"], true);
    assert_eq!(admin_row["session_enabled"], true);
}

// ── list_admin_users filters ──────────────────────────────────────────

#[tokio::test]
async fn list_users_with_query_filter() {
    let (app, admin_token, _, _, _, _storage) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/users?q=admin")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = body_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"].as_i64().unwrap(), 1);
    assert_eq!(body["users"].as_array().unwrap().len(), 1);
    assert_eq!(body["users"][0]["name"].as_str().unwrap(), "admin");
}

#[tokio::test]
async fn list_users_with_status_filter() {
    let (app, admin_token, _, _, _, _storage) = setup().await;
    // The setup creates "pending" with session_enabled=false, verified=true
    // That maps to "disabled" status (session_enabled=false AND verified=true)
    let resp = app
        .oneshot(
            Request::get("/api/admin/users?status=disabled")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = body_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"].as_i64().unwrap(), 1);
    assert_eq!(body["users"][0]["name"].as_str().unwrap(), "pending");
}

#[tokio::test]
async fn list_users_with_pagination() {
    let (app, admin_token, _, _, _, _storage) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/users?limit=1&offset=0")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = body_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["users"].as_array().unwrap().len(), 1);
    assert!(body["total"].as_i64().unwrap() >= 3);
}

// ── enable ───────────────────────────────────────────────────────────

#[tokio::test]
async fn enable_admin_user_flips_bit_and_emits_audit() {
    let (app, admin, _, _, pending_id, storage) = setup().await;
    let resp = app
        .oneshot(
            Request::post(format!("/api/admin/users/{pending_id}/enable"))
                .header("Authorization", format!("Bearer {admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = body_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session_enabled"], true);
    assert_eq!(body["id"], pending_id.to_string());

    // Storage round-trip: the row is actually flipped.
    let row = storage.find_user_by_id(pending_id).await.unwrap().unwrap();
    assert!(row.session_enabled);

    // Audit row was written.
    let sink = telepair_core::audit::AuditSink::new(storage.clone());
    let events = sink.query(AuditFilter::default()).await.unwrap();
    let audit = events
        .iter()
        .find(|e| e.event_type == AuditEventType::AuthUserEnabled)
        .expect("expected auth.user_enabled row");
    assert_eq!(audit.detail["target_user_id"], pending_id.to_string());
    assert_eq!(audit.detail["target_user_name"], "pending");
}

#[tokio::test]
async fn enable_admin_user_non_admin_is_403() {
    let (app, _, regular, _, pending_id, _) = setup().await;
    let resp = app
        .oneshot(
            Request::post(format!("/api/admin/users/{pending_id}/enable"))
                .header("Authorization", format!("Bearer {regular}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn enable_admin_user_unknown_target_is_404() {
    let (app, admin, _, _, _, _) = setup().await;
    let missing = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::post(format!("/api/admin/users/{missing}/enable"))
                .header("Authorization", format!("Bearer {admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn enable_admin_user_malformed_id_is_400() {
    let (app, admin, _, _, _, _) = setup().await;
    let resp = app
        .oneshot(
            Request::post("/api/admin/users/not-a-uuid/enable")
                .header("Authorization", format!("Bearer {admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_cannot_enable_or_disable_self() {
    // Self-mutation guard. An admin disabling their own session bit
    // would lock themselves out of session creation on the very
    // next request, so the handler rejects it at the HTTP layer
    // even though the storage layer would happily honour the write.
    let (app, admin_token, _, _, _, storage) = setup().await;
    let admin = storage.get_user_by_name("admin").await.unwrap().unwrap();

    // Disable self
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/api/admin/users/{}/disable", admin.id))
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    // Body must carry a specific reason so the admin UI can distinguish
    // "self-protection" from "malformed request" and toast accordingly.
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["error"].as_str().unwrap(),
        "cannot change your own account's session access"
    );

    // Enable self (also blocked for symmetry)
    let resp = app
        .oneshot(
            Request::post(format!("/api/admin/users/{}/enable", admin.id))
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── disable ──────────────────────────────────────────────────────────

#[tokio::test]
async fn disable_admin_user_flips_bit_and_emits_audit() {
    let (app, admin, _, _, _, storage) = setup().await;
    // Pick the regular user (seeded with session_enabled = TRUE)
    // and disable them. Exercises the HAPPY path opposite of the
    // pending user in the enable test above.
    let regular = storage.get_user_by_name("regular").await.unwrap().unwrap();
    let resp = app
        .oneshot(
            Request::post(format!("/api/admin/users/{}/disable", regular.id))
                .header("Authorization", format!("Bearer {admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = body_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session_enabled"], false);

    let row = storage.find_user_by_id(regular.id).await.unwrap().unwrap();
    assert!(!row.session_enabled);

    let sink = telepair_core::audit::AuditSink::new(storage.clone());
    let events = sink.query(AuditFilter::default()).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == AuditEventType::AuthUserDisabled
                && e.detail["target_user_id"] == regular.id.to_string()),
        "expected auth.user_disabled row"
    );
}

#[tokio::test]
async fn whoami_surfaces_session_enabled_bit() {
    // The frontend banner reads `session_enabled` from whoami to
    // decide whether to render "pending approval". Pin that the
    // field is there and reflects storage state.
    let (app, _, _, pending_token, _, _) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/auth/whoami")
                .header("Authorization", format!("Bearer {pending_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body) = body_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session_enabled"], false);
    assert_eq!(body["is_admin"], false);
    assert_eq!(body["name"], "pending");
}
