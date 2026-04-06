#![deny(unsafe_code)]

pub mod http;
pub mod session_hub;
pub mod state;
pub mod ws;

use axum::{
    Router,
    http::HeaderValue,
    routing::{delete, get, post},
};
use state::AppState;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

/// Default loopback origins allowed when the operator does not pass
/// `--allowed-origins` or `--allow-any-origin`. These match the Vite
/// dev server documented in CLAUDE.md (`npm run dev` on :5173 proxies
/// /api and /ws to :7700). Prod deployments serve the frontend from
/// the same origin as the API, so CORS is skipped entirely by the
/// browser — no need to list :7700 here.
const DEFAULT_LOOPBACK_ORIGINS: &[&str] =
    &["http://localhost:5173", "http://127.0.0.1:5173"];

/// CORS policy for `build_router_with_options`. A typed enum instead of
/// a sentinel empty-list so we can't accidentally fall back to "allow
/// everything" when an operator meant "use the default list".
pub enum CorsMode {
    /// Allow any origin (equivalent to `Access-Control-Allow-Origin: *`).
    /// Use only in dev or behind a reverse proxy that enforces CORS.
    /// Must be explicitly opted into at the CLI.
    AllowAny,
    /// Allow only the listed origins. Parse errors are fatal — an
    /// operator typo must not silently widen CORS to everything.
    /// Passing an empty list falls back to `DEFAULT_LOOPBACK_ORIGINS`.
    Origins(Vec<String>),
}

pub fn build_router(state: AppState) -> Router {
    // Tests that don't care about CORS use this. `AllowAny` can never
    // fail to parse so the expect is infallible.
    build_router_with_options(state, None, CorsMode::AllowAny)
        .expect("AllowAny CORS mode is infallible")
}

pub fn build_router_with_options(
    state: AppState,
    web_dir: Option<&str>,
    cors: CorsMode,
) -> Result<Router, String> {
    let cors = match cors {
        CorsMode::AllowAny => {
            tracing::warn!(
                "CORS: allowing any origin — only safe in dev or behind a CORS-enforcing proxy"
            );
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        }
        CorsMode::Origins(list) => {
            // Empty list → tighten to loopback dev defaults instead of
            // silently allowing everything. This fixes the previous
            // behaviour where `no flags` meant `allow_origin(Any)`.
            let source: Vec<String> = if list.is_empty() {
                tracing::info!(
                    "CORS: no --allowed-origins specified, defaulting to loopback dev origins ({})",
                    DEFAULT_LOOPBACK_ORIGINS.join(", ")
                );
                DEFAULT_LOOPBACK_ORIGINS.iter().map(|s| s.to_string()).collect()
            } else {
                list
            };

            // Fail loudly on bad origins — the old code used filter_map
            // and silently dropped typos, which could downgrade an
            // operator's intended allowlist to the empty list (which
            // then blocked every cross-origin request). Surface the
            // error so startup fails visibly.
            let mut parsed: Vec<HeaderValue> = Vec::with_capacity(source.len());
            for origin in &source {
                match origin.parse::<HeaderValue>() {
                    Ok(value) => parsed.push(value),
                    Err(e) => {
                        return Err(format!(
                            "invalid CORS origin `{origin}`: {e}. \
                             Expected an absolute URL like `http://localhost:5173`."
                        ));
                    }
                }
            }

            CorsLayer::new()
                .allow_origin(AllowOrigin::list(parsed))
                .allow_methods(Any)
                .allow_headers(Any)
        }
    };

    let api = Router::new()
        .route("/api/health", get(http::health))
        .route("/api/targets", get(http::list_targets))
        .route(
            "/api/sessions",
            post(http::create_session).get(http::list_sessions),
        )
        .route("/api/sessions/{session_id}", delete(http::close_session))
        .route(
            "/api/sessions/{session_id}/invite",
            post(http::create_invite),
        )
        .route("/api/invite/redeem", post(http::redeem_invite))
        .route("/ws/session/{session_id}", get(ws::ws_handler))
        .layer(cors)
        .with_state(state);

    let router = match web_dir {
        Some(dir) => {
            let serve =
                ServeDir::new(dir).not_found_service(ServeFile::new(format!("{dir}/index.html")));
            api.fallback_service(serve)
        }
        None => api,
    };
    Ok(router)
}
