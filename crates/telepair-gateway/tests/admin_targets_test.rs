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

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tempfile::NamedTempFile;
use tower::ServiceExt;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::audit::{AuditEventType, AuditFilter};
use telepair_core::session::InputMode;
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::session_hub::PtyLaunch;
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
    let state = AppState::new(storage.clone(), engine, Some(path.clone()), None, std::path::PathBuf::from("/tmp/telepair-test")).await;
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
    let state = AppState::new(storage.clone(), engine, Some(path.clone()), None, std::path::PathBuf::from("/tmp/telepair-test")).await;
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

#[tokio::test]
async fn reload_targets_rejects_drop_of_still_referenced_target() {
    // Regression test for the v0.1.1 hot-reload finding: if an
    // admin rewrites targets.yaml to drop a target that still has
    // a live session, the old code would happily swap the new
    // engine in — the next WS reconnect on that session would hit
    // `TARGET_NOT_FOUND` and `cleanup_orphan_session` would stamp
    // the row as Error, killing the session through the back door.
    //
    // The guard must: (1) return 400 with `reason=still_referenced`,
    // (2) include the offending target and its active-session count,
    // (3) leave the OLD engine loaded so the live session is still
    // reachable, (4) NOT touch the DB session row.
    //
    // This test pins the **hub-backed** contract: the guard counts
    // live PTYs in the `SessionHub` (in-memory map), not `status=
    // 'active'` rows in SQLite. A zombie DB row whose PTY has
    // already exited must NOT wedge a reload — see
    // `reload_targets_allows_drop_when_only_stale_db_row_present`
    // for the companion proof.
    let (file, path) = write_targets(INITIAL_TARGETS_YAML);
    let engine = TargetEngine::from_file(&path).expect("parse initial yaml");
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (admin, admin_token) = storage.create_user("admin", true).await.unwrap();

    // Seed a live session on `alpha`: create the DB row, then spawn
    // a real PTY through the hub so the reload guard's
    // `count_live_sessions_per_target()` walk sees it. We use
    // `sleep 300` as the child — cheap to spawn, never tries to
    // read stdin, and we tear it down via `hub.stop_session` at
    // the end of the test.
    let session = storage
        .create_session_with_owner(admin.id, "alpha", InputMode::Serialized, None)
        .await
        .unwrap();

    let state = AppState::new(storage.clone(), engine, Some(path.clone()), None, std::path::PathBuf::from("/tmp/telepair-test")).await;
    state
        .hub
        .start_or_join(
            &session.id,
            "alpha",
            PtyLaunch {
                command: "sleep",
                args: &["300".to_string()],
                env: &HashMap::new(),
                cols: 80,
                rows: 24,
            },
        )
        .await
        .expect("spawn live session on alpha");
    let hub = state.hub.clone();
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    // Rewrite the yaml to drop `alpha` entirely.
    const DROP_ALPHA_YAML: &str = r#"
targets:
  - name: beta
    display: Beta Only
    command: printf
"#;
    std::fs::write(&path, DROP_ALPHA_YAML).unwrap();

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
    assert_eq!(json["reason"], "still_referenced");
    let targets = json["targets"].as_array().expect("targets array");
    assert_eq!(targets.len(), 1, "expected exactly one blocked target");
    assert_eq!(targets[0]["target"], "alpha");
    assert_eq!(targets[0]["active_sessions"], 1);

    // Follow-up list MUST still show the original `alpha` — the
    // guard rolled the swap back, so the old engine (with its
    // `alpha` entry) stays loaded. This is the real assertion
    // that live sessions are still reachable through the engine.
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
        names.contains(&"alpha"),
        "alpha must still be loaded after rejected reload: {names:?}"
    );
    assert!(
        names.contains(&"beta"),
        "beta must still be loaded after rejected reload: {names:?}"
    );

    // And the session DB row must still be `active` — the reject
    // path is purely a handler-level rollback; no session state
    // touched.
    let row = storage.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(row.status.as_str(), "active");

    // Tear down the live PTY we spawned so the test doesn't leak a
    // sleep child into the harness runtime.
    hub.stop_session(&session.id).await;

    drop(file);
}

#[tokio::test]
async fn reload_targets_allows_adding_new_target_with_referenced_alive() {
    // Companion to `reload_targets_rejects_drop_of_still_referenced_target`
    // — the guard must NOT block reloads that only ADD targets or
    // tweak a referenced target's metadata. This is the "don't
    // over-rotate" side of the conservative gate: admins still need
    // to be able to extend targets.yaml while sessions are live.
    let (file, path) = write_targets(INITIAL_TARGETS_YAML);
    let engine = TargetEngine::from_file(&path).expect("parse initial yaml");
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (admin, admin_token) = storage.create_user("admin", true).await.unwrap();

    // Live session on alpha — DB row plus a hub entry backing it,
    // same pattern as the reject test. The hub is the guard's
    // source of truth, so we have to spawn a real PTY for the guard
    // to "see" alpha as referenced.
    let session = storage
        .create_session_with_owner(admin.id, "alpha", InputMode::Serialized, None)
        .await
        .unwrap();

    let state = AppState::new(storage.clone(), engine, Some(path.clone()), None, std::path::PathBuf::from("/tmp/telepair-test")).await;
    state
        .hub
        .start_or_join(
            &session.id,
            "alpha",
            PtyLaunch {
                command: "sleep",
                args: &["300".to_string()],
                env: &HashMap::new(),
                cols: 80,
                rows: 24,
            },
        )
        .await
        .expect("spawn live session on alpha");
    let hub = state.hub.clone();
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    // New yaml keeps alpha (same name), tweaks its display, adds
    // `delta`, drops the unreferenced `beta`. All three of these
    // edits are allowed by the conservative gate.
    const EXTEND_YAML: &str = r#"
targets:
  - name: alpha
    display: Alpha Tweaked
    command: echo
  - name: delta
    display: Delta
    command: true
"#;
    std::fs::write(&path, EXTEND_YAML).unwrap();

    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "reload with still-referenced alpha preserved must succeed"
    );

    hub.stop_session(&session.id).await;

    drop(file);
}

#[tokio::test]
async fn reload_targets_allows_drop_when_only_stale_db_row_present() {
    // Complementary regression test for the hub-backed reload
    // guard: a session that EXITS in DB-land (`status='active'`
    // with no live PTY in the hub) must NOT block the reload.
    // This is the scenario the DB-backed guard fumbled — the
    // reaper's cleanup window or a crashed PTY would leave a
    // zombie row that made admins unable to rotate targets until
    // startup cleanup ran.
    //
    // Setup: create an `alpha` DB row directly via storage (no
    // hub entry), then reload a yaml that drops `alpha`. With the
    // hub-backed count, live_counts for alpha is 0, so the guard
    // does not reject. We assert the reload succeeds, and the
    // stale DB row is untouched.
    let (file, path) = write_targets(INITIAL_TARGETS_YAML);
    let engine = TargetEngine::from_file(&path).expect("parse initial yaml");
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (admin, admin_token) = storage.create_user("admin", true).await.unwrap();

    // DB-only session on alpha (no hub entry) — simulates a
    // zombie row whose PTY exited but whose close hasn't landed.
    let session = storage
        .create_session_with_owner(admin.id, "alpha", InputMode::Serialized, None)
        .await
        .unwrap();

    let state = AppState::new(storage.clone(), engine, Some(path.clone()), None, std::path::PathBuf::from("/tmp/telepair-test")).await;
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    // Rewrite yaml to drop alpha — with the old DB-backed guard
    // this would have returned 400; with the hub-backed guard it
    // should succeed.
    const DROP_ALPHA_YAML: &str = r#"
targets:
  - name: beta
    display: Beta Only
    command: printf
"#;
    std::fs::write(&path, DROP_ALPHA_YAML).unwrap();

    let resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "stale DB row must not wedge reload — guard counts the hub"
    );

    // Stale row is untouched by the reload path.
    let row = storage.get_session(&session.id).await.unwrap().unwrap();
    assert_eq!(row.status.as_str(), "active");

    drop(file);
}

#[tokio::test]
async fn reload_targets_rejects_drop_during_create_to_attach_gap() {
    // Regression test for the v0.1.1 hot-reload race that codex
    // flagged: between `POST /api/sessions` returning 201 and the
    // client's WS handshake landing in `start_or_join`, the hub
    // map has no `LiveSession` for the freshly minted session.
    // Pre-fix, the reload guard's `count_live_sessions_per_target`
    // walked an empty hub for the new target, so a concurrent
    // `POST /api/admin/targets/reload` that dropped the target
    // happily swapped in the new engine. The next WS attach then
    // failed `targets.load().resolve(...)` and
    // `cleanup_orphan_session` stamped the row `Error`, leaving
    // the owner unable to rejoin a target that, from their POV,
    // had just been created and acknowledged.
    //
    // The fix is `SessionHub::reserve_target`: the create-session
    // HTTP handler reserves a `SessionEntry::Pending` slot before
    // returning 201, so the reload guard sees the target as
    // "still in use" until the WS attach upgrades it. This test
    // exercises the HTTP path end-to-end and asserts:
    //   1. POST /api/sessions returns 201
    //   2. NO WS attach happens (we never call `hub.start_or_join`)
    //   3. POST /api/admin/targets/reload with a yaml that drops
    //      the new target returns 400 `still_referenced`
    //   4. The session DB row is still `active` — reload was
    //      rolled back, not committed
    //
    // Without the reservation this test fails at step 3 with a
    // 200, and at step 4 the target is gone from the engine
    // (followup ws attach would orphan the session).
    let (file, path) = write_targets(INITIAL_TARGETS_YAML);
    let engine = TargetEngine::from_file(&path).expect("parse initial yaml");
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let (_, admin_token) = storage.create_user("admin", true).await.unwrap();

    let state = AppState::new(storage.clone(), engine, Some(path.clone()), None, std::path::PathBuf::from("/tmp/telepair-test")).await;
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    // Step 1: create a session via the HTTP handler — this is
    // the only call site that exercises `reserve_target`.
    let create_resp = app
        .clone()
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {admin_token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"alpha"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let create_body = create_resp.into_body().collect().await.unwrap().to_bytes();
    let session_json: serde_json::Value = serde_json::from_slice(&create_body).unwrap();
    let session_id = session_json["id"].as_str().expect("session id").to_string();

    // Step 2: deliberately do NOT attach via WS. The whole point
    // is that the reservation must protect the gap.

    // Step 3: rewrite yaml to drop alpha and reload.
    const DROP_ALPHA_YAML: &str = r#"
targets:
  - name: beta
    display: Beta Only
    command: printf
"#;
    std::fs::write(&path, DROP_ALPHA_YAML).unwrap();
    let reload_resp = app
        .oneshot(
            Request::post("/api/admin/targets/reload")
                .header("Authorization", format!("Bearer {admin_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        reload_resp.status(),
        StatusCode::BAD_REQUEST,
        "reload must reject — pending reservation should keep alpha referenced"
    );
    let reload_body = reload_resp.into_body().collect().await.unwrap().to_bytes();
    let reload_json: serde_json::Value = serde_json::from_slice(&reload_body).unwrap();
    assert_eq!(reload_json["reason"], "still_referenced");
    let blocked = reload_json["targets"].as_array().expect("targets array");
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["target"], "alpha");
    assert_eq!(
        blocked[0]["active_sessions"], 1,
        "the pending reservation must count exactly once toward the per-target total"
    );

    // Step 4: the reject path is purely a handler-level rollback.
    // The session DB row stays `active`, ready for the WS attach
    // that's about to arrive.
    let row = storage.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(row.status.as_str(), "active");

    drop(file);
}

/// Reload with bad yaml, assert 400 + parse_error containing
/// `expected_substr`, then verify the old engine is still loaded.
async fn assert_reload_rejected(bad_yaml: &str, expected_substr: &str) {
    let (app, admin_token, _, path, _file) = setup(INITIAL_TARGETS_YAML).await;
    std::fs::write(&path, bad_yaml).unwrap();

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
    let message = json["message"].as_str().unwrap();
    assert!(
        message.contains(expected_substr),
        "expected '{expected_substr}' in message, got: {message}"
    );

    // Old engine must still be loaded.
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
        "beta should still be present after rejected reload: {names:?}"
    );
}

#[tokio::test]
async fn reload_targets_rejects_duplicate_names() {
    let yaml = r#"
targets:
  - name: alpha
    display: Alpha
    command: echo
  - name: alpha
    display: Alpha Clone
    command: printf
"#;
    assert_reload_rejected(yaml, "duplicate name").await;
}

#[tokio::test]
async fn reload_targets_rejects_missing_command() {
    let yaml = r#"
targets:
  - name: broken
    display: Broken Target
"#;
    assert_reload_rejected(yaml, "requires a command").await;
}
