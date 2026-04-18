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
//! 1. No auth, no header → 401 (baseline: the handler demands *some*
//!    credential).
//! 2. `?token=<any>` but no header → 401 (log-exfil regression: the
//!    query param is no longer honoured; the auth-header branch runs
//!    instead and fails for an unauthenticated caller).
//! 3. `X-Share-Token: <invalid>` → 400 (header is honoured and
//!    routes through the share-validation path, which rejects the
//!    unknown digest as `InvalidInput`).
//!
//! The contrast between (2) and (3) is load-bearing: any regression
//! that re-enables query-string tokens would flip (2) to 400, and
//! any regression that drops header support would flip (3) to 401.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use telepair_gateway::build_router;
use telepair_gateway::state::AppState;
use tower::ServiceExt;

async fn setup() -> axum::Router {
    let state = AppState::new_test().await;
    build_router(state)
}

#[tokio::test]
async fn recording_data_without_credentials_returns_401() {
    let app = setup().await;
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
}

#[tokio::test]
async fn recording_data_query_token_is_not_honoured() {
    // If `?token=…` were still honoured, this request would enter
    // the share-token validation path and fail with 400 (invalid
    // share token). Asserting 401 locks in the log-exfil fix: the
    // query param is ignored and the unauthenticated caller trips
    // the standard auth gate instead.
    let app = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/recordings/any-id/data?token=whatever-raw-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "query-string token must no longer trigger the share-token path \
         (leaks to reverse-proxy access logs)"
    );
}

#[tokio::test]
async fn recording_data_x_share_token_header_routes_into_share_validation() {
    // A bogus `X-Share-Token` header must NOT land on the auth path —
    // if it did, the response would be 401 "missing bearer" and the
    // server would silently accept query-string tokens as a
    // fallback. We assert 400 (InvalidInput, the share-validation
    // verdict) to prove the header extractor wired into the share
    // branch.
    let app = setup().await;
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
        StatusCode::BAD_REQUEST,
        "X-Share-Token header must route into share-validation, \
         which rejects unknown tokens as 400"
    );
}
