//! Integration tests for `POST /api/admin/targets/validate`.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tempfile::NamedTempFile;
use tower::ServiceExt;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router_with_options};

const INITIAL_YAML: &str = r#"
targets:
  - name: alpha
    display: Alpha
    command: echo
  - name: beta
    display: Beta
    command: printf
"#;

fn write_targets(yaml: &str) -> (NamedTempFile, PathBuf) {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(yaml.as_bytes()).unwrap();
    file.flush().unwrap();
    let path = file.path().to_path_buf();
    (file, path)
}

async fn setup(yaml: &str) -> (axum::Router, String, PathBuf, NamedTempFile) {
    let (file, path) = write_targets(yaml);
    let engine = TargetEngine::from_file(&path).unwrap();
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, admin_token) = storage.create_user("admin", true).await.unwrap();
    let state = AppState::new(
        storage.clone(),
        engine,
        Some(path.clone()),
        None,
        PathBuf::from("/tmp/telepair-test"),
    )
    .await;
    let router = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();
    (router, admin_token, path, file)
}

#[tokio::test]
async fn validate_no_changes() {
    let (app, admin_token, _, _file) = setup(INITIAL_YAML).await;
    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/validate")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["valid"].as_bool().unwrap());
    assert!(json["diff"]["added"].as_array().unwrap().is_empty());
    assert!(json["diff"]["removed"].as_array().unwrap().is_empty());
    assert!(json["diff"]["changed"].as_array().unwrap().is_empty());
    // from_file always injects "local-shell" plus the 2 yaml targets
    // The engine was loaded from file, so unchanged should have all targets
    assert!(json["diff"]["unchanged"].as_array().unwrap().len() >= 2);
}

#[tokio::test]
async fn validate_detects_diff() {
    let (app, admin_token, path, _file) = setup(INITIAL_YAML).await;

    // Overwrite file with changed content
    let new_yaml = r#"
targets:
  - name: alpha
    display: Alpha v2
    command: echo
  - name: gamma
    display: Gamma
    command: true
"#;
    std::fs::write(&path, new_yaml).unwrap();

    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/validate")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["valid"].as_bool().unwrap());
    assert_eq!(json["diff"]["added"], serde_json::json!(["gamma"]));
    assert_eq!(json["diff"]["removed"], serde_json::json!(["beta"]));
    assert_eq!(json["diff"]["changed"], serde_json::json!(["alpha"]));
    assert!(json["blocked"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn validate_invalid_yaml_returns_errors() {
    let (app, admin_token, path, _file) = setup(INITIAL_YAML).await;

    // Write invalid YAML
    std::fs::write(&path, "not: valid: yaml: [[").unwrap();

    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/validate")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(!json["valid"].as_bool().unwrap());
    assert!(json["errors"].is_array());
}

#[tokio::test]
async fn validate_returns_expected_sha256_pinned_to_file_bytes() {
    let (app, admin_token, path, _file) = setup(INITIAL_YAML).await;
    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/validate")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let returned = json["expected_sha256"]
        .as_str()
        .expect("validate must return expected_sha256 so reload can TOCTOU-check");
    assert_eq!(returned.len(), 64, "sha256 hex is 64 chars");

    // Recompute from the raw bytes the handler just read; they must match.
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(&path).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let expected: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(returned, expected);
}

#[tokio::test]
async fn reload_rejects_stale_expected_sha256_as_file_changed() {
    // Simulate the preview→confirm TOCTOU: admin previewed version A
    // (captured its sha), a second writer overwrote the file to
    // version B, and the admin hits confirm. The server must refuse
    // so the admin doesn't unknowingly apply B.
    let (app, admin_token, path, _file) = setup(INITIAL_YAML).await;

    // Overwrite the file after the admin "previewed" the original.
    let changed_yaml = r#"
targets:
  - name: delta
    display: Delta
    command: true
"#;
    std::fs::write(&path, changed_yaml).unwrap();

    let stale_sha = "0".repeat(64);
    let body = serde_json::json!({ "expected_sha256": stale_sha });
    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {admin_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["reason"], "file_changed");
    assert_eq!(json["expected_sha256"], stale_sha);
    assert!(json["actual_sha256"].is_string());
}

#[tokio::test]
async fn reload_accepts_matching_expected_sha256() {
    // The happy path for the new guard: the admin's previewed sha
    // matches the file still on disk, so the reload goes through.
    let (app, admin_token, path, _file) = setup(INITIAL_YAML).await;

    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(&path).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let sha: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let body = serde_json::json!({ "expected_sha256": sha });
    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {admin_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn validate_requires_admin() {
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, user_token) = storage.create_user("user", false).await.unwrap();
    let state = AppState::new(
        storage.clone(),
        TargetEngine::empty(),
        None,
        None,
        PathBuf::from("/tmp/telepair-test"),
    )
    .await;
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/validate")
                .header("Authorization", format!("Bearer {user_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
