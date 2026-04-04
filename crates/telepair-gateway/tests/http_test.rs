use axum::body::Body;
use axum::http::{Request, StatusCode};
use telepair_gateway::build_router;
use telepair_gateway::state::AppState;
use tower::ServiceExt; // for oneshot

async fn setup() -> (axum::Router, String) {
    let state = AppState::new_test().await;
    let token = state.create_test_user("tester").await;
    let router = build_router(state);
    (router, token)
}

#[tokio::test]
async fn health_check() {
    let (app, _) = setup().await;
    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn list_targets_requires_auth() {
    let (app, _) = setup().await;
    let resp = app
        .oneshot(Request::get("/api/targets").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_targets_with_auth() {
    let (app, token) = setup().await;
    let resp = app
        .oneshot(
            Request::get("/api/targets")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn create_session_and_list() {
    let (app, token) = setup().await;
    // Create session
    let resp = app
        .clone()
        .oneshot(
            Request::post("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"target_name":"local-shell"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // List sessions
    let resp = app
        .oneshot(
            Request::get("/api/sessions")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
