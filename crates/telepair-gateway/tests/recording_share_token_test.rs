//! Regression tests for the `X-Share-Token` transport on
//! `GET /api/recordings/{id}/data`.
//!
//! The share token used to travel in a `?token=…` query parameter,
//! which was quietly written to every reverse-proxy access log on
//! the request path (NGINX `$request`, ALB `request_url`, CloudFront
//! standard logs — all capture URIs by default but never arbitrary
//! request headers). Anyone with log read access could therefore
//! replay a still-valid share link. The fix ships the token in an
//! `X-Share-Token` request header and drops query-parameter support.
//!
//! These tests pin the contract so a later refactor cannot silently
//! re-introduce the leak:
//!
//! 1. No auth, no header → 401 with body `{"error":"Unauthorized"}`
//!    (baseline: the handler demands *some* credential and the bearer
//!    branch emits the bare canonical 401).
//! 2. `?token=<any>` but no header → 401 with body
//!    `{"error":"Unauthorized"}` (log-exfil regression: the query
//!    param is no longer honoured; the auth-header branch runs
//!    instead and fails for an unauthenticated caller, matching
//!    the baseline byte-for-byte).
//! 3. `X-Share-Token: <invalid>` → 401 with body that mentions the
//!    share token (header is honoured and routes through the
//!    share-validation path; the failure is a failed credential and
//!    surfaces as `Error::Auth`, whose `Display` is
//!    `"authentication failed: invalid, expired, or exhausted share
//!    token"`).
//!
//! The contrast between (2) and (3) is load-bearing: both return 401
//! but via different error bodies, so any regression that re-enables
//! query-string tokens would flip (2)'s body to the share-token
//! message (the query param re-entering the share path), and any
//! regression that drops header support would flip (3)'s body to the
//! bearer message (the header-extractor no longer wiring into the
//! share branch).

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use telepair_agent::virtual_target::TargetEngine;
use telepair_core::recording::RecordingConfig;
use telepair_core::session::InputMode;
use telepair_core::storage::{SqliteStorage, Storage};
use telepair_gateway::build_router;
use telepair_gateway::state::AppState;
use tower::ServiceExt;

async fn body_error_message(body: Body) -> String {
    let bytes = to_bytes(body, 4096).await.expect("body collect");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("body is JSON");
    json.get("error")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

async fn setup() -> (axum::Router, AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(SqliteStorage::new_memory().await.unwrap());
    let state = AppState::new(
        storage,
        TargetEngine::empty(),
        None,
        None,
        dir.path().join("data"),
        RecordingConfig {
            dir: dir.path().join("recordings"),
            ..RecordingConfig::default()
        },
    )
    .await;
    (build_router(state.clone()), state, dir)
}

#[tokio::test]
async fn recording_data_without_credentials_returns_401_unauthorized() {
    let (app, _, _dir) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/recordings/any-id/data")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "baseline: missing credential must be 401"
    );
    let msg = body_error_message(resp.into_body()).await;
    assert_eq!(
        msg, "Unauthorized",
        "baseline body must be the bare canonical 401 message"
    );
}

#[tokio::test]
async fn recording_data_query_token_is_not_honoured() {
    // If `?token=…` were still honoured, this request would enter
    // the share-token validation path and its body would mention
    // "share token". Asserting the bare `Unauthorized` body locks in
    // the log-exfil fix: the query param is ignored and the
    // unauthenticated caller trips the standard auth gate instead.
    let (app, _, _dir) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/recordings/any-id/data?token=whatever-raw-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let msg = body_error_message(resp.into_body()).await;
    assert_eq!(
        msg, "Unauthorized",
        "query-string token must NOT trigger the share-token path \
         (it would leak to reverse-proxy access logs); the request \
         must fall through to the bearer-auth failure whose body is \
         the bare canonical 401 message"
    );
}

#[tokio::test]
async fn recording_data_x_share_token_header_routes_into_share_validation() {
    // A bogus `X-Share-Token` header must land on the share-validation
    // path, not the bearer-auth path. Both paths now return 401 (a
    // spent/revoked/unknown share is a failed credential — fix for QA
    // v0.1.9 C4), but their bodies differ: share validation surfaces
    // the `Error::Auth` message, bearer missing returns the bare
    // canonical reason. Asserting on the message proves the header
    // extractor wired into the share branch.
    let (app, _, _dir) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/recordings/any-id/data")
                .header("X-Share-Token", "not-a-real-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "invalid share token is a failed credential → 401"
    );
    let msg = body_error_message(resp.into_body()).await;
    assert!(
        msg.contains("share token"),
        "X-Share-Token header must route into share-validation, not \
         bearer auth. Expected body to mention 'share token'; got {msg:?}"
    );
}

#[tokio::test]
async fn recording_data_missing_file_does_not_consume_share_use() {
    let (app, state, _dir) = setup().await;
    let (user, _) = state.storage.create_user("owner", false).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    let rec = state
        .storage
        .create_recording(
            "rec_share_missing_file",
            &session.id,
            user.id,
            80,
            24,
            "rec_share_missing_file.cast",
            None,
        )
        .await
        .unwrap();
    state
        .storage
        .complete_recording(&rec.id, 1000, 1, 64)
        .await
        .unwrap();
    let (raw_share, share) = state
        .recording
        .create_share(&rec.id, 1, None)
        .await
        .unwrap();

    let resp = app
        .oneshot(
            Request::get(format!("/api/recordings/{}/data", rec.id))
                .header("X-Share-Token", raw_share)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let shares = state.storage.list_recording_shares(&rec.id).await.unwrap();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].token_sha256, share.token_sha256);
    assert_eq!(
        shares[0].used_count, 0,
        "failed file read must not burn a share use"
    );
}

#[tokio::test]
async fn recording_data_success_sets_private_no_store_cache_headers() {
    let (app, state, _dir) = setup().await;
    let (user, token) = state.storage.create_user("owner", false).await.unwrap();
    let session = state
        .storage
        .create_session_with_owner(user.id, "local-shell", InputMode::Serialized, None)
        .await
        .unwrap();
    let rec = state
        .storage
        .create_recording(
            "rec_download_headers",
            &session.id,
            user.id,
            80,
            24,
            "rec_download_headers.cast",
            None,
        )
        .await
        .unwrap();
    state
        .storage
        .complete_recording(&rec.id, 1000, 1, 64)
        .await
        .unwrap();

    let path = state.recording.recording_file_path(&rec.id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"cast-data").unwrap();

    let resp = app
        .oneshot(
            Request::get(format!("/api/recordings/{}/data", rec.id))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-store"
    );
    assert_eq!(
        resp.headers().get(header::VARY).unwrap(),
        "Authorization, X-Share-Token"
    );
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/x-asciicast"
    );

    let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
    assert_eq!(bytes.as_ref(), b"cast-data");
}
