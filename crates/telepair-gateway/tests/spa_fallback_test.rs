//! Regression tests for the SPA deep-linking fix.
//!
//! Before this fix, `build_router_with_options(..., Some(web_dir), _)`
//! composed `ServeDir::not_found_service(ServeFile(index.html))`,
//! which returned `HTTP 404` (with the correct `index.html` body)
//! for every client-side route like `/login`, `/join/<token>`, and
//! `/session/<id>`. The status code matters because:
//!
//! - nginx `proxy_intercept_errors` turns 404s into error pages
//! - CDN rules treat 404s as cacheable "dead link" signals
//! - Uptime probes mark the app as down
//! - OG scrapers / SEO bots skip indexing the page
//!
//! The correct behaviour is `200 OK` with the SPA shell body so the
//! client-side router can take over. These tests pin that contract.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router_with_options};
use tempfile::TempDir;
use tower::ServiceExt;

const FAKE_SPA_HTML: &str =
    "<!doctype html><html><head><title>telepair-spa</title></head>\
     <body><div id=\"app\">marker-42</div></body></html>";

/// Spin up a temp directory that looks like a built SPA: a single
/// `index.html` at the root. The returned `TempDir` must be kept
/// alive for the duration of the test or the dir is removed.
fn fake_web_dist() -> TempDir {
    let tmp = TempDir::new().expect("create tempdir for fake web dist");
    std::fs::write(tmp.path().join("index.html"), FAKE_SPA_HTML)
        .expect("write fake index.html");
    tmp
}

async fn build_app_with_spa(tmp: &TempDir) -> axum::Router {
    let state = AppState::new_test().await;
    let web_dir = tmp.path().to_str().expect("tempdir path is utf-8");
    build_router_with_options(state, Some(web_dir), CorsMode::AllowAny)
        .expect("router must build with valid web dir")
}

async fn assert_spa_shell(app: &axum::Router, path: &str) {
    let resp = app
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "SPA deep-link `{path}` must return 200 OK, not 404 (reverse proxies and CDNs interpret \
         404 as a dead link even when the body is right)"
    );

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("text/html"),
        "SPA shell must be text/html, got `{content_type}`"
    );

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = std::str::from_utf8(&bytes).expect("SPA shell body is utf-8");
    assert!(
        body.contains("marker-42"),
        "SPA shell body should contain the marker from index.html, got: {body}"
    );
}

#[tokio::test]
async fn login_deep_link_returns_spa_shell() {
    let tmp = fake_web_dist();
    let app = build_app_with_spa(&tmp).await;
    assert_spa_shell(&app, "/login").await;
}

#[tokio::test]
async fn join_deep_link_returns_spa_shell() {
    let tmp = fake_web_dist();
    let app = build_app_with_spa(&tmp).await;
    assert_spa_shell(&app, "/join/demo-invite-token").await;
}

#[tokio::test]
async fn session_deep_link_returns_spa_shell() {
    let tmp = fake_web_dist();
    let app = build_app_with_spa(&tmp).await;
    assert_spa_shell(&app, "/session/abc123").await;
}

#[tokio::test]
async fn root_path_still_serves_index() {
    // Exercising `/` too, even though it's a `ServeDir` hit rather
    // than a fallback hit, so a future refactor that breaks
    // `ServeDir` itself (not the fallback) is still caught.
    let tmp = fake_web_dist();
    let app = build_app_with_spa(&tmp).await;
    let resp = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_routes_are_not_swallowed_by_spa_fallback() {
    // Sanity guard on the routing composition: adding a `--web-dir`
    // must not cause `/api/*` requests to be answered with the SPA
    // shell. If the fallback caught them, health checks and every
    // other API call would look "alive" even when the handler
    // itself was broken or missing.
    let tmp = fake_web_dist();
    let app = build_app_with_spa(&tmp).await;

    let resp = app
        .clone()
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("application/json"),
        "/api/health must stay JSON, got `{content_type}` (SPA fallback is swallowing API routes)"
    );
}

#[tokio::test]
async fn missing_index_html_fails_build_loudly() {
    // Constructing a router with `--web-dir` pointing at a dir with
    // no `index.html` must fail at startup. The old implementation
    // couldn't tell the difference between "operator forgot to
    // build the web frontend" and "happy path" because
    // `ServeFile::new` was lazy; the result was a server that
    // looked healthy but served empty bodies. Fail loudly so the
    // operator sees the problem before traffic does.
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::new_test().await;
    let res = build_router_with_options(
        state,
        Some(tmp.path().to_str().unwrap()),
        CorsMode::AllowAny,
    );
    assert!(
        res.is_err(),
        "build_router_with_options must reject a web_dir with no index.html"
    );
    let msg = res.unwrap_err();
    assert!(
        msg.contains("index.html"),
        "error message should name the missing file, got: {msg}"
    );
}
