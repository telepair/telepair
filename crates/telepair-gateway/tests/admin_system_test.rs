//! Integration tests for `GET /api/admin/system`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::recording::RecordingConfig;
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router_with_options};

async fn setup() -> (axum::Router, String, String) {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, admin_token) = storage.create_user("admin", true).await.unwrap();
    let (_, user_token) = storage.create_user("regular", false).await.unwrap();
    let state = AppState::new(
        storage.clone(),
        TargetEngine::empty(),
        None,
        None,
        PathBuf::from("/tmp/telepair-test"),
        RecordingConfig::default(),
    )
    .await;
    let router = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();
    (router, admin_token, user_token)
}

#[tokio::test]
async fn system_info_requires_admin() {
    let (app, _, user_token) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/system")
                .header("Authorization", format!("Bearer {user_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn system_info_returns_expected_fields() {
    let (app, admin_token, _) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/system")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Version must be present and non-empty
    assert!(json["version"].is_string());
    assert!(!json["version"].as_str().unwrap().is_empty());

    // Paths
    assert!(json["data_dir"].is_string());
    assert!(json["db_path"].is_string());

    // Counts
    assert!(json["live_sessions"].is_number());
    assert!(json["registered_users"].is_number());
    // We created 2 users in setup
    assert_eq!(json["registered_users"].as_i64().unwrap(), 2);

    // SMTP not configured in test
    assert!(!json["smtp_configured"].as_bool().unwrap());

    // targets_path is null when not configured
    assert!(json["targets_path"].is_null());

    // Uptime should be a small positive number
    assert!(json["uptime_seconds"].as_u64().unwrap() < 10);
}

#[tokio::test]
async fn system_info_unauthenticated_is_401() {
    let (app, _, _) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/system")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
