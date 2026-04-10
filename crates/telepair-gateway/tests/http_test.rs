use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use telepair_core::session::{InputMode, Session};
use telepair_core::storage::Storage;
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

#[tokio::test]
async fn list_sessions_target_name_query_param_narrows_results() {
    // Regression for the v0.1.1-dev bug where `ListSessionsQuery`
    // deserialized `target` instead of `target_name`, so the
    // frontend's `?target_name=local-shell` filter silently fell
    // through to the unfiltered query. Seed two sessions with
    // different `target_name`s for the same user (bypassing the
    // create_session handler, which only knows `local-shell`), then
    // GET /api/sessions?target_name=local-shell and assert only the
    // matching row comes back.
    let state = AppState::new_test().await;
    let (user, token) = state
        .storage
        .create_user("filter-tester", false)
        .await
        .unwrap();
    let kept = state
        .storage
        .create_session_with_owner(user.id, "local-shell", InputMode::Multiplexed)
        .await
        .unwrap();
    state
        .storage
        .create_session_with_owner(user.id, "other-target", InputMode::Multiplexed)
        .await
        .unwrap();
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::get("/api/sessions?target_name=local-shell")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sessions: Vec<Session> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sessions.len(), 1, "filter should narrow to one row");
    assert_eq!(sessions[0].id, kept.id);
    assert_eq!(sessions[0].target_name, "local-shell");
}
