use axum::body::Body;
use axum::http::{Request, StatusCode};
use telepair_gateway::build_router_with_options;
use telepair_gateway::state::AppState;
use tower::ServiceExt;

#[tokio::test]
async fn cors_allows_all_when_no_origins_specified() {
    let state = AppState::new_test().await;
    let app = build_router_with_options(state, None, &[]);

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
    assert_eq!(acao, "*");
}

#[tokio::test]
async fn cors_reflects_allowed_origin() {
    let state = AppState::new_test().await;
    let origins = vec!["http://allowed.example.com".to_string()];
    let app = build_router_with_options(state, None, &origins);

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
    let app = build_router_with_options(state, None, &origins);

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
