#![deny(unsafe_code)]

pub mod http;
pub mod session_hub;
pub mod state;
pub mod ws;

use axum::{
    routing::{get, post},
    Router,
};
use state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(http::health))
        .route("/api/targets", get(http::list_targets))
        .route(
            "/api/sessions",
            post(http::create_session).get(http::list_sessions),
        )
        .route("/ws/session/{session_id}", get(ws::ws_handler))
        .with_state(state)
}
