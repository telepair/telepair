use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use telepair_agent::virtual_target::TargetEngine;
use telepair_core::recording::RecordingConfig;
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router_with_options};
use tower::ServiceExt;

const TARGETS_YAML: &str = r#"
targets:
  - name: restricted
    display: Restricted Target
    command: echo
    admin_only: true
  - name: open
    display: Open Target
    command: echo
"#;

async fn setup() -> (axum::Router, String, String) {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let engine = TargetEngine::from_yaml(TARGETS_YAML).unwrap();
    let state = AppState::new(
        storage.clone(),
        engine,
        None,
        None,
        std::path::PathBuf::from("/tmp/telepair-test"),
        RecordingConfig::default(),
    )
    .await;

    let (_, user_token) = storage.create_user("regular", false).await.unwrap();
    let (_, admin_token) = storage.create_user("admin", true).await.unwrap();
    let router = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    (router, user_token, admin_token)
}

#[tokio::test]
async fn non_admin_blocked_from_admin_only_target() {
    let (app, user_token, _) = setup().await;

    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {user_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"restricted"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_can_access_admin_only_target() {
    let (app, _, admin_token) = setup().await;

    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {admin_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"restricted"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn unrestricted_target_allows_non_admin() {
    let (app, user_token, _) = setup().await;

    let resp = app
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {user_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"open"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn list_targets_hides_admin_only_from_non_admin() {
    // Information-leak fix: before this filter, a non-admin user
    // could `GET /api/targets` and still see the `restricted`
    // target's name / display / tags — only the create path was
    // gated. Target names in the wild often encode environment info
    // (prod-db, staging-ssh), so leaking the list is its own
    // problem. This test pins the filter so a future refactor
    // can't silently drop it.
    use http_body_util::BodyExt;
    let (app, user_token, _) = setup().await;

    let resp = app
        .oneshot(
            Request::get("/api/targets")
                .header("Authorization", format!("Bearer {user_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let targets: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    let names: Vec<&str> = targets
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"open"),
        "non-admin should still see open targets, got {names:?}"
    );
    assert!(
        !names.contains(&"restricted"),
        "admin_only target `restricted` must be hidden from non-admin; got {names:?}"
    );
}

#[tokio::test]
async fn list_targets_shows_admin_only_to_admin() {
    // Dual to the filter test above: admins must still see the full
    // list so they can actually use the admin-only targets.
    use http_body_util::BodyExt;
    let (app, _, admin_token) = setup().await;

    let resp = app
        .oneshot(
            Request::get("/api/targets")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let targets: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    let names: Vec<&str> = targets
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"open") && names.contains(&"restricted"),
        "admin should see all targets, got {names:?}"
    );
}
