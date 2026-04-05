#![deny(unsafe_code)]

pub mod http;
pub mod session_hub;
pub mod state;
pub mod ws;

use axum::{
    routing::{delete, get, post},
    Router,
};
use state::AppState;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

pub fn build_router(state: AppState) -> Router {
    build_router_with_options(state, None, &[])
}

pub fn build_router_with_options(
    state: AppState,
    web_dir: Option<&str>,
    allowed_origins: &[String],
) -> Router {
    let cors = if allowed_origins.is_empty() {
        tracing::warn!("CORS: allowing all origins (no --allowed-origins specified)");
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<_> = allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods(Any)
            .allow_headers(Any)
    };

    let api = Router::new()
        .route("/api/health", get(http::health))
        .route("/api/targets", get(http::list_targets))
        .route(
            "/api/sessions",
            post(http::create_session).get(http::list_sessions),
        )
        .route(
            "/api/sessions/{session_id}",
            delete(http::close_session),
        )
        .route(
            "/api/sessions/{session_id}/invite",
            post(http::create_invite),
        )
        .route("/api/invite/redeem", post(http::redeem_invite))
        .route("/ws/session/{session_id}", get(ws::ws_handler))
        .layer(cors)
        .with_state(state);

    match web_dir {
        Some(dir) => {
            let serve = ServeDir::new(dir)
                .not_found_service(ServeFile::new(format!("{dir}/index.html")));
            api.fallback_service(serve)
        }
        None => api,
    }
}
