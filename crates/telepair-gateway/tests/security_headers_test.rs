use axum::body::Body;
use axum::http::Request;
use telepair_gateway::state::AppState;
use tower::ServiceExt;

#[tokio::test]
async fn responses_include_content_security_policy() {
    let state = AppState::new_test().await;
    let app = telepair_gateway::build_router(state);

    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let csp = resp
        .headers()
        .get("content-security-policy")
        .expect("CSP header must be present")
        .to_str()
        .unwrap();

    assert!(csp.contains("default-src 'self'"));
    assert!(csp.contains("script-src 'self'"));
    assert!(csp.contains("connect-src 'self'"));
    assert!(!csp.contains("ws:"));
    assert!(!csp.contains("wss:"));
    assert!(csp.contains("frame-ancestors 'none'"));
}
