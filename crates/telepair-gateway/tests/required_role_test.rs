use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use telepair_agent::virtual_target::TargetEngine;
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router_with_options};
use tower::ServiceExt;

const TARGETS_YAML: &str = r#"
targets:
  - name: restricted
    display: Restricted Target
    command: echo
    required_role: operator
  - name: open
    display: Open Target
    command: echo
"#;

async fn setup() -> (axum::Router, String, String) {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let engine = TargetEngine::from_yaml(TARGETS_YAML).unwrap();
    let state = AppState::new(storage.clone(), engine).await;

    let (_, user_token) = storage.create_user("regular", false).await.unwrap();
    let (_, admin_token) = storage.create_user("admin", true).await.unwrap();
    let router = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    (router, user_token, admin_token)
}

#[tokio::test]
async fn non_admin_blocked_by_required_role() {
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
async fn admin_bypasses_required_role() {
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
async fn no_required_role_allows_non_admin() {
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
