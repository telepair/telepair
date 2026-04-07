use axum::body::Body;
use axum::http::{Request, StatusCode};
use telepair_gateway::state::AppState;
use telepair_gateway::{CorsMode, build_router_with_options};
use tower::ServiceExt;

#[tokio::test]
async fn cors_allow_any_is_wildcard() {
    let state = AppState::new_test().await;
    let app = build_router_with_options(state, None, CorsMode::AllowAny).unwrap();

    let resp = app
        .oneshot(
            Request::get("/api/health")
                .header("Origin", "http://evil.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let acao = resp.headers().get("access-control-allow-origin").unwrap();
    assert_eq!(acao, "*", "CorsMode::AllowAny must advertise wildcard ACAO");
}

#[tokio::test]
async fn cors_empty_list_defaults_to_loopback() {
    // No allowed_origins specified → the policy must fall back to
    // the dev-loopback defaults (localhost:5173 / 127.0.0.1:5173),
    // NOT silently allow every origin. This is the core of the
    // CORS default-tightening fix.
    let state = AppState::new_test().await;
    let app = build_router_with_options(state, None, CorsMode::Origins(vec![])).unwrap();

    // Loopback dev origin → reflected back
    let ok = app
        .clone()
        .oneshot(
            Request::get("/api/health")
                .header("Origin", "http://localhost:5173")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(
        ok.headers().get("access-control-allow-origin").unwrap(),
        "http://localhost:5173",
        "default loopback allowlist should reflect localhost:5173"
    );

    // Random internet origin → no ACAO header (request still runs
    // because CORS is browser-enforced, but the browser will block it)
    let nope = app
        .oneshot(
            Request::get("/api/health")
                .header("Origin", "http://evil.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(nope.status(), StatusCode::OK);
    assert!(
        nope.headers().get("access-control-allow-origin").is_none(),
        "default loopback allowlist must NOT expose wildcard ACAO"
    );
}

#[tokio::test]
async fn cors_reflects_allowed_origin() {
    let state = AppState::new_test().await;
    let origins = vec!["http://allowed.example.com".to_string()];
    let app = build_router_with_options(state, None, CorsMode::Origins(origins)).unwrap();

    let resp = app
        .oneshot(
            Request::get("/api/health")
                .header("Origin", "http://allowed.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let acao = resp.headers().get("access-control-allow-origin").unwrap();
    assert_eq!(acao, "http://allowed.example.com");
}

#[tokio::test]
async fn cors_omits_header_for_unlisted_origin() {
    let state = AppState::new_test().await;
    let origins = vec!["http://allowed.example.com".to_string()];
    let app = build_router_with_options(state, None, CorsMode::Origins(origins)).unwrap();

    let resp = app
        .oneshot(
            Request::get("/api/health")
                .header("Origin", "http://evil.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Request still succeeds (CORS is browser-enforced) but no ACAO header
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn cors_rejects_malformed_origin() {
    // A typo or invalid URL used to be silently dropped by filter_map,
    // which could leave an operator with an effectively-empty allowlist
    // and every cross-origin request blocked for mysterious reasons.
    // The new policy returns an error so startup fails loudly.
    let state = AppState::new_test().await;
    // HeaderValue parsing blocks raw newlines (CRLF smuggling) —
    // that's the surface area we want to catch.
    let origins = vec!["http://valid.example.com\r\ninjected".to_string()];
    let result = build_router_with_options(state, None, CorsMode::Origins(origins));

    let err = result.expect_err("malformed origin must not silently pass");
    assert!(
        err.contains("injected") || err.contains("invalid"),
        "error message should mention the offending origin, got: {err}"
    );
}
