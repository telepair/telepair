#![deny(unsafe_code)]

pub mod http;
pub mod origin;
pub mod rate_limit;
pub mod recording_cleaner;
pub mod recording_writer;
pub mod session_hub;
pub mod state;
pub mod ws;

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{HeaderName, HeaderValue, Request, Response, StatusCode, header},
    routing::{delete, get, post, put},
};
use bytes::Bytes;
use state::AppState;
use tower::service_fn;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

use origin::{DEFAULT_LOOPBACK_ORIGINS, OriginPolicy};

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
    mut state: AppState,
    web_dir: Option<&str>,
    cors: CorsMode,
) -> Result<Router, String> {
    let origin_policy = match cors {
        CorsMode::AllowAny => {
            tracing::warn!(
                "CORS: allowing any origin — only safe in dev or behind a CORS-enforcing proxy"
            );
            OriginPolicy::allow_any()
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
                DEFAULT_LOOPBACK_ORIGINS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
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

            OriginPolicy::origins(parsed)
        }
    };
    let cors = origin_policy.cors_layer();
    state.origin_policy = Arc::new(origin_policy);

    let api = Router::new()
        .route("/api/health", get(http::health))
        .route("/api/auth/register", post(http::register))
        .route("/api/auth/verify", post(http::verify_otp))
        .route("/api/auth/login", post(http::login))
        .route("/api/auth/whoami", get(http::whoami))
        .route("/api/auth/change-password", post(http::change_password))
        .route("/api/targets", get(http::list_targets))
        .route("/api/user-targets", post(http::create_user_target))
        .route(
            "/api/user-targets/{id}",
            get(http::get_user_target)
                .put(http::update_user_target)
                .delete(http::delete_user_target),
        )
        .route(
            "/api/sessions",
            post(http::create_session).get(http::list_sessions),
        )
        .route("/api/sessions/{session_id}", delete(http::close_session))
        .route(
            "/api/sessions/{session_id}/audit",
            get(http::list_session_audit),
        )
        .route(
            "/api/sessions/{session_id}/participants/{user_id}/role",
            put(http::update_participant_role),
        )
        .route(
            "/api/sessions/{session_id}/invites",
            post(http::create_invite).get(http::list_session_invites),
        )
        .route(
            "/api/sessions/{session_id}/invites/{token_sha256}",
            delete(http::revoke_session_invite),
        )
        .route("/api/invite/redeem", post(http::redeem_invite))
        // Recording control (owner-only)
        .route(
            "/api/sessions/{session_id}/recording/start",
            post(http::start_recording),
        )
        .route(
            "/api/sessions/{session_id}/recording/stop",
            post(http::stop_recording),
        )
        // Recording CRUD
        .route("/api/recordings", get(http::list_recordings))
        .route(
            "/api/recordings/{recording_id}",
            get(http::get_recording).delete(http::delete_recording),
        )
        .route(
            "/api/recordings/{recording_id}/data",
            get(http::get_recording_data),
        )
        // Share management (owner-only)
        .route(
            "/api/recordings/{recording_id}/shares",
            post(http::create_recording_share).get(http::list_recording_shares),
        )
        .route(
            "/api/recordings/{recording_id}/shares/{token}",
            delete(http::revoke_recording_share),
        )
        // Recording lifecycle (owner + admin)
        .route(
            "/api/recordings/{recording_id}/keep",
            post(http::keep_recording),
        )
        .route(
            "/api/recordings/{recording_id}/expire",
            post(http::expire_recording),
        )
        // Admin-only target management. Both handlers gate on
        // `is_admin` after `extract_user`, so unauthenticated callers
        // still get 401 from the shared extractor and non-admin
        // callers get 403. Kept under `/api/admin/*` so reverse
        // proxies that want to isolate admin traffic can match on
        // the prefix.
        .route("/api/admin/targets", get(http::list_admin_targets))
        .route("/api/admin/targets/reload", post(http::reload_targets))
        .route("/api/admin/targets/validate", post(http::validate_targets))
        .route("/api/admin/audit", get(http::list_admin_audit))
        .route("/api/admin/audit/export", get(http::export_audit))
        .route("/api/admin/system", get(http::system_info))
        .route("/api/admin/recordings", get(http::list_admin_recordings))
        .route(
            "/api/admin/recordings/{recording_id}",
            delete(http::delete_admin_recording),
        )
        .route("/api/admin/users", get(http::list_admin_users))
        .route(
            "/api/admin/users/{id}/enable",
            post(http::enable_admin_user),
        )
        .route(
            "/api/admin/users/{id}/disable",
            post(http::disable_admin_user),
        )
        .route("/ws/session/{session_id}", get(ws::ws_handler))
        .layer(cors)
        .with_state(state);

    let router = match web_dir {
        Some(dir) => {
            // SPA deep-linking fix: previously we used
            //   ServeDir::not_found_service(ServeFile(index.html))
            // which correctly served the HTML body for routes like
            // `/login`, `/join/<token>`, and `/session/<id>` — but
            // with `HTTP 404`. The reason is a tower-http gotcha:
            // `ServeFile` is just a `ServeDir` pinned to one path,
            // and when it receives a request whose URI doesn't
            // match that path it returns 404 (not 200 with the
            // pinned file). Chaining it behind `not_found_service`
            // means every SPA deep link went out the door as 404,
            // which breaks reverse proxies (nginx
            // `proxy_intercept_errors`), CDN rules, uptime probes,
            // and OG/SEO crawlers.
            //
            // Correct behaviour: the SPA shell should answer with
            // `200 OK` for any non-asset request so the client-side
            // router can take over. We read `index.html` once at
            // boot, wrap it in a refcounted `Bytes`, and hand
            // `ServeDir` a `service_fn` fallback that returns the
            // bytes with an explicit 200 for every path. `Bytes`
            // lets every request share the same buffer via an Arc
            // bump — no memcpy per deep link. If `index.html` can't
            // be read at startup we fail loudly rather than
            // silently serving an empty body.
            let index_path: PathBuf = PathBuf::from(dir).join("index.html");
            let index_bytes = std::fs::read(&index_path).map_err(|e| {
                format!(
                    "failed to read SPA shell `{}`: {e}. \
                     Build the web frontend before starting the gateway.",
                    index_path.display()
                )
            })?;
            let index_body: Bytes = Bytes::from(index_bytes);
            let spa_fallback = service_fn(move |req: Request<Body>| {
                let body = index_body.clone();
                async move {
                    // API + WebSocket routes must NEVER fall through
                    // to the SPA shell. Without this guard, an
                    // unknown `/api/typo` returns `200 text/html`
                    // (the SPA HTML), which:
                    //   - hides typos in the frontend (caller sees
                    //     200, then a JSON-parse error two layers
                    //     deeper)
                    //   - makes uptime probes report a dead route as
                    //     "alive" because the body type changed but
                    //     the status didn't
                    //   - lets a failed `/ws/*` upgrade silently
                    //     deliver an HTML body to a websocket
                    //     client, which is a debugging nightmare
                    // The contract: anything under `/api/` or `/ws/`
                    // that the router didn't match is a real 404,
                    // returned as JSON so frontend error handling
                    // stays on the structured-error path.
                    let path = req.uri().path();
                    if path.starts_with("/api/") || path.starts_with("/ws/") {
                        let response = Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(br#"{"error":"not_found"}"#.to_vec()))
                            .expect("static 404 JSON response is always well-formed");
                        return Ok::<_, Infallible>(response);
                    }
                    let response = Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                        // SPA shell must never be cached by
                        // intermediaries — a fresh deploy needs to
                        // reach every client on their next
                        // navigation. Hashed asset files keep their
                        // own caching via `ServeDir`.
                        .header(header::CACHE_CONTROL, "no-cache")
                        .body(Body::from(body))
                        .expect("static SPA shell response is always well-formed");
                    Ok::<_, Infallible>(response)
                }
            });

            // Use `.fallback()` (not `.not_found_service()`) —
            // critical gotcha: `not_found_service` wraps the
            // fallback in `SetStatus::new(..., NOT_FOUND)`, which
            // overrides whatever status our service_fn returns and
            // forces it back to 404. That is literally the bug we
            // are fixing in this file. `.fallback()` preserves the
            // fallback's own status code, which is documented at
            // https://docs.rs/tower-http/0.6/tower_http/services/struct.ServeDir.html#method.fallback
            let serve = ServeDir::new(dir).fallback(spa_fallback);
            api.fallback_service(serve)
        }
        None => api,
    };

    // Baseline security response headers applied to every route (API,
    // static assets, SPA shell, WebSocket upgrade responses).
    //
    // `if_not_present` so any individual handler that needs a
    // different value can still set one — the defaults are a
    // safety net, not an override.
    //
    // - `X-Frame-Options: DENY` — the terminal surface is a
    //   clickjacking target; block all framing.
    // - `X-Content-Type-Options: nosniff` — stops MIME sniffing,
    //   relevant for served `.cast` recording downloads.
    // - `Referrer-Policy: strict-origin-when-cross-origin` — leaks
    //   no path info cross-origin (share-link URLs carry digests in
    //   the path since v0.1.8 but the policy still adds margin).
    // - `Content-Security-Policy` — limits token exfiltration blast
    //   radius if a future XSS lands. `style-src 'unsafe-inline'` is
    //   required by the current component-local style tags.
    //   `connect-src 'self'` covers same-origin `wss://` upgrades on
    //   modern browsers (CSP3); the gateway never opens cross-origin
    //   sockets, so no `ws:`/`wss:` wildcard is needed.
    let router = router
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; \
                 script-src 'self'; \
                 style-src 'self' 'unsafe-inline'; \
                 img-src 'self' data: blob:; \
                 font-src 'self' data:; \
                 connect-src 'self'; \
                 worker-src 'self' blob:; \
                 object-src 'none'; \
                 base-uri 'self'; \
                 form-action 'self'; \
                 frame-ancestors 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ));

    Ok(router)
}
