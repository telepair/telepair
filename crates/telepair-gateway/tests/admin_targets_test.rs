//! Integration tests for the admin target management endpoints.
//!
//! These cover the HTTP surface that backs the Admin → Targets page:
//!
//! - `GET  /api/admin/targets`         — full detail + env redaction
//!   + active session count
//! - `POST /api/admin/targets/reload`  — atomic ArcSwap hot-reload
//!
//! Both endpoints are admin-only: 401 without a bearer, 403 for a
//! regular user, 200 for an admin. The tests lean on
//! `AppState::new(..., Some(path))` to exercise the path that
//! backs `POST .../reload`; other specs use `AppState::new_test()`
//! which always passes `targets_path: None`.

use std::io::Write;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tempfile::NamedTempFile;
use tower::ServiceExt;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::audit::{AuditEventType, AuditFilter};
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router_with_options};

const INITIAL_TARGETS_YAML: &str = r#"
targets:
  - name: alpha
    display: Alpha
    command: echo
    args: ["hello"]
    tags: [demo]
    env:
      TELEPAIR_TEST_PRESENT: "${TELEPAIR_TEST_PRESENT}"
      TELEPAIR_TEST_MISSING: "${TELEPAIR_TEST_MISSING}"
  - name: beta
    display: Beta
    command: printf
    admin_only: true
"#;

const RELOADED_TARGETS_YAML: &str = r#"
targets:
  - name: alpha
    display: Alpha v2
    command: echo
  - name: gamma
    display: Gamma
    command: true
"#;

/// Write `yaml` to a fresh temp file and hand back the file handle
/// plus its path. The caller keeps the handle so the file isn't
/// cleaned up before the test finishes.
fn write_targets(yaml: &str) -> (NamedTempFile, std::path::PathBuf) {
    let mut file = NamedTempFile::new().expect("create temp targets.yaml");
    file.write_all(yaml.as_bytes()).expect("write yaml");
    let path = file.path().to_path_buf();
    (file, path)
}

/// Build a router + seed an admin and a regular user. Mirrors the
/// pattern in `admin_only_target_test.rs`. Returns the router, the
/// admin token, the non-admin token, and the path that the reload
/// handler re-reads — so the test can rewrite the file before calling
/// `POST /api/admin/targets/reload`.
async fn setup(
    yaml: &'static str,
) -> (
    axum::Router,
    String,
    String,
    std::path::PathBuf,
    NamedTempFile,
) {
    let (file, path) = write_targets(yaml);
    let engine = TargetEngine::from_file(&path).expect("parse initial yaml");
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, admin_token) = storage.create_user("admin", true).await.unwrap();
    let (_, user_token) = storage.create_user("regular", false).await.unwrap();
    let state = AppState::new(storage.clone(), engine, Some(path.clone())).await;
    let router = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();
    (router, admin_token, user_token, path, file)
}

#[tokio::test]
async fn list_admin_targets_unauthenticated_is_401() {
    let (app, _, _, _, _file) = setup(INITIAL_TARGETS_YAML).await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/targets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_admin_targets_non_admin_is_403() {
    let (app, _, user_token, _, _file) = setup(INITIAL_TARGETS_YAML).await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/targets")
                .header("Authorization", format!("Bearer {user_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_admin_targets_returns_full_detail_with_env_redaction() {
    // The admin list must show BOTH targets (including the
    // `admin_only: true` one — admins are the audience here,
    // the info-leak filter on `/api/targets` does not apply).
    // It must also redact env values: only keys + presence.
    // Set one env var so we can assert `set=true` for present
    // keys and `set=false` for missing ones.
    // SAFETY: the Rust 2024 prelude marks `std::env::set_var` as
    // unsafe because multi-threaded access is a data race risk. Our
    // test runtime only touches this key from this test and reads
    // it from a blocking-resolver inside the handler — no concurrent
    // writes — so the unsafe block is sound for the test scope.
    unsafe {
        std::env::set_var("TELEPAIR_TEST_PRESENT", "redacted-secret");
        std::env::remove_var("TELEPAIR_TEST_MISSING");
    }

    let (app, admin_token, _, _, _file) = setup(INITIAL_TARGETS_YAML).await;
    let resp = app
        .oneshot(
            Request::get("/api/admin/targets")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body_str = std::str::from_utf8(&body).unwrap();
    let targets: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // `local-shell` is always injected by `TargetEngine`, so the
    // list should be 3 entries long sorted alphabetically.
    let arr = targets.as_array().expect("expected an array");
    assert!(
        arr.len() >= 3,
        "expected at least 3 targets, got {}: {body_str}",
        arr.len()
    );
    let names: Vec<&str> = arr.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"alpha"), "alpha missing: {body_str}");
    assert!(names.contains(&"beta"), "beta missing: {body_str}");
    assert!(
        names.contains(&"local-shell"),
        "default local-shell missing: {body_str}"
    );

    // The literal env value must NEVER appear in the response —
    // a simple substring check is the tightest possible guard
    // against accidental value leakage.
    assert!(
        !body_str.contains("redacted-secret"),
        "env value leaked into admin targets response: {body_str}"
    );

    let alpha = arr.iter().find(|t| t["name"] == "alpha").unwrap();
    assert_eq!(alpha["display"], "Alpha");
    assert_eq!(alpha["admin_only"], false);
    let env_keys = alpha["env"].as_array().unwrap();
    let present = env_keys
        .iter()
        .find(|k| k["key"] == "TELEPAIR_TEST_PRESENT")
        .expect("present key missing");
    assert_eq!(present["set"], true);
    let missing = env_keys
        .iter()
        .find(|k| k["key"] == "TELEPAIR_TEST_MISSING")
        .expect("missing key row missing");
    assert_eq!(missing["set"], false);

    let beta = arr.iter().find(|t| t["name"] == "beta").unwrap();
    assert_eq!(beta["admin_only"], true);
    assert_eq!(beta["command"], "printf");

    unsafe {
        std::env::remove_var("TELEPAIR_TEST_PRESENT");
    }
}

#[tokio::test]
async fn reload_targets_non_admin_is_403() {
    let (app, _, user_token, _, _file) = setup(INITIAL_TARGETS_YAML).await;
    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {user_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reload_targets_swaps_in_new_engine_and_emits_audit() {
    let (app, admin_token, _, path, _file) = setup(INITIAL_TARGETS_YAML).await;

    // Overwrite the on-disk yaml with a new target set BEFORE
    // calling reload. `NamedTempFile` keeps the handle alive so the
    // path stays valid; `std::fs::write` is the simple atomic-enough
    // replacement for this test.
    std::fs::write(&path, RELOADED_TARGETS_YAML).unwrap();

    // Clone the router for a second call so we can exercise BOTH
    // the reload AND a follow-up list in the same test without
    // needing two fresh setups. `axum::Router` is `Clone`.
    let app2 = app.clone();
    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // local-shell is always injected, so the reloaded engine has 3 targets.
    assert_eq!(json["targets"], 3);
    assert_eq!(json["path"].as_str().unwrap(), path.display().to_string());

    // And the follow-up list MUST show the new names — this is
    // the real assertion that the ArcSwap store took effect.
    let resp2 = app2
        .oneshot(
            Request::get("/api/admin/targets")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = resp2.into_body().collect().await.unwrap().to_bytes();
    let arr2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
    let names: Vec<&str> = arr2
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"alpha"), "alpha missing post-reload");
    assert!(names.contains(&"gamma"), "gamma missing post-reload");
    assert!(!names.contains(&"beta"), "beta should be gone post-reload");

    // The alpha target's display should reflect the new yaml.
    let alpha = arr2
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "alpha")
        .unwrap();
    assert_eq!(alpha["display"], "Alpha v2");
}

#[tokio::test]
async fn reload_targets_audit_event_is_recorded() {
    // A dedicated test for the audit emit so the assertion is
    // isolated from the swap verification above. We reuse the same
    // storage handle to peek at the audit table after the reload.
    let (file, path) = write_targets(INITIAL_TARGETS_YAML);
    let engine = TargetEngine::from_file(&path).expect("parse initial yaml");
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, admin_token) = storage.create_user("admin", true).await.unwrap();
    let state = AppState::new(storage.clone(), engine, Some(path.clone())).await;
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    std::fs::write(&path, RELOADED_TARGETS_YAML).unwrap();
    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let filter = AuditFilter {
        event_types: vec![AuditEventType::TargetReloaded],
        ..Default::default()
    };
    let events = storage.list_audit_events(&filter).await.unwrap();
    assert_eq!(events.len(), 1, "expected exactly one target.reloaded row");
    let ev = &events[0];
    assert_eq!(ev.event_type, AuditEventType::TargetReloaded);
    assert_eq!(
        ev.actor_name.as_deref(),
        Some("admin"),
        "actor snapshot missing"
    );
    assert_eq!(ev.detail["targets"], 3);
    assert_eq!(ev.detail["path"], path.display().to_string());

    // Keep the temp file alive until the end of the test so `path`
    // stays valid throughout.
    drop(file);
}

#[tokio::test]
async fn reload_targets_surfaces_parse_error_without_swapping() {
    // Seed a working engine, then rewrite the yaml to garbage.
    // The handler should return 400 and the subsequent list call
    // should still reflect the ORIGINAL targets (the old ArcSwap
    // pointer survived the failed parse).
    let (app, admin_token, _, path, _file) = setup(INITIAL_TARGETS_YAML).await;
    std::fs::write(&path, "not: valid: yaml: [unterminated").unwrap();
    let app2 = app.clone();

    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["reason"], "parse_error");

    // Follow-up list must still show the original targets — the
    // parse failure is a hard rollback signal, not a silent no-op.
    let resp2 = app2
        .oneshot(
            Request::get("/api/admin/targets")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = resp2.into_body().collect().await.unwrap().to_bytes();
    let arr2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
    let names: Vec<&str> = arr2
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"beta"),
        "beta should still be present after failed reload"
    );
}

#[tokio::test]
async fn reload_targets_no_path_configured_returns_400() {
    // When the operator never configured a targets.yaml, the
    // admin reload endpoint must respond with a distinct
    // `no_targets_path` reason so the UI can render a clear
    // "configure a file and restart" message. `AppState::new_test`
    // always leaves `targets_path` as `None`, which is exactly the
    // state we need.
    let state = AppState::new_test().await;
    let (_, admin_token) = state.storage.create_user("admin", true).await.unwrap();
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["reason"], "no_targets_path");
}
