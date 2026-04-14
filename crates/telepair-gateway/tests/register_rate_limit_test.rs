//! HTTP-level regression tests for the per-IP register throttle.
//!
//! These tests build a router directly (tower `oneshot`), inject a
//! `ConnectInfo<SocketAddr>` extension into each request, and install
//! a short rate-limit window on `AppState.register_rl` so the throttle
//! fires within test time. We deliberately do *not* exercise the
//! production `AppState::new` path here — that starts a sqlite DB and
//! a reaper task, which would slow the test without adding coverage
//! the unit tests don't already provide.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use telepair_gateway::build_router;
use telepair_gateway::rate_limit::RegisterRateLimiter;
use telepair_gateway::state::AppState;
use tower::ServiceExt;

/// Build a router with a short-window limiter attached to AppState.
/// Returns the router so callers can `.clone().oneshot(...)` between
/// requests without losing the shared limiter state.
async fn setup_with_limiter(min_interval: Duration) -> axum::Router {
    let mut state = AppState::new_test().await;
    state.register_rl = Some(Arc::new(RegisterRateLimiter::new(min_interval)));
    build_router(state)
}

/// Build a POST /api/auth/register request with `ConnectInfo` injected
/// as if axum had booted with `into_make_service_with_connect_info`.
/// The body is intentionally malformed so the handler short-circuits
/// after the rate-limit check (keeps these tests fast — we never need
/// SMTP or Argon2 to run, and a rate-limit 429 must take precedence
/// over a 400 on a bad body anyway).
fn register_request(ip: [u8; 4]) -> Request<Body> {
    let mut req = Request::post("/api/auth/register")
        .header("Content-Type", "application/json")
        .body(Body::from(
            r#"{"email":"a@b.c","password":"xxxxxxxx","display_name":"x"}"#,
        ))
        .unwrap();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])), 55555);
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

#[tokio::test]
async fn second_register_from_same_ip_is_429() {
    // Window wide enough that the second call is always within it,
    // but short enough that a later test case can elapse it if ever
    // needed. 200 ms is comfortably above any oneshot latency.
    let app = setup_with_limiter(Duration::from_millis(200)).await;

    let first = app
        .clone()
        .oneshot(register_request([1, 2, 3, 4]))
        .await
        .unwrap();
    // The first call passes the rate-limit gate and then falls into
    // AuthService::register — without SMTP configured the test state
    // returns 503. Anything in the non-429 range is fine here; the
    // assertion we actually care about is the *second* call's 429.
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app
        .clone()
        .oneshot(register_request([1, 2, 3, 4]))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn register_from_different_ip_is_not_throttled() {
    // Regression guard: an attacker on 9.9.9.9 must not lock out a
    // legitimate signup from 8.8.8.8. A previous revision that kept a
    // single global last_seen instant would have failed this test and
    // prevented the bug from reaching production.
    let app = setup_with_limiter(Duration::from_millis(200)).await;
    let first = app
        .clone()
        .oneshot(register_request([8, 8, 8, 8]))
        .await
        .unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app
        .clone()
        .oneshot(register_request([9, 9, 9, 9]))
        .await
        .unwrap();
    assert_ne!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// Build a router with the trust-forwarded-headers flag on.
async fn setup_trusting_proxy(min_interval: Duration) -> axum::Router {
    let mut state = AppState::new_test().await;
    state.register_rl = Some(Arc::new(RegisterRateLimiter::new(min_interval)));
    state.trust_forwarded_headers = true;
    build_router(state)
}

/// Build a request with the peer IP fixed to 127.0.0.1 (simulating a
/// loopback reverse proxy) and a configurable `X-Forwarded-For`
/// header carrying the real client.
fn register_request_with_xff(xff: &str) -> Request<Body> {
    let mut req = Request::post("/api/auth/register")
        .header("Content-Type", "application/json")
        .header("X-Forwarded-For", xff)
        .body(Body::from(
            r#"{"email":"a@b.c","password":"xxxxxxxx","display_name":"x"}"#,
        ))
        .unwrap();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 55555);
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

#[tokio::test]
async fn register_behind_trusted_proxy_keys_on_xff_not_peer() {
    // The deployment shape telepair documents is nginx → localhost.
    // Without `trust_forwarded_headers`, every user's real IP gets
    // collapsed onto 127.0.0.1 and the whole fleet shares one 30 s
    // bucket — a single adversary can 429 every signup. With the
    // flag on, the limiter must pick up the proxy-appended XFF entry
    // instead, so two different clients behind the same proxy both
    // pass even back-to-back.
    let app = setup_trusting_proxy(Duration::from_millis(200)).await;

    let first = app
        .clone()
        .oneshot(register_request_with_xff("203.0.113.7"))
        .await
        .unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app
        .clone()
        .oneshot(register_request_with_xff("198.51.100.42"))
        .await
        .unwrap();
    assert_ne!(
        second.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "distinct XFF clients must not share a bucket"
    );
}

/// Build a request whose peer is 127.0.0.1 (loopback nginx) and which
/// carries an `X-Forwarded-For` header of the operator's choice, plus
/// an optional `X-Real-IP`. Mirrors the headers nginx writes under the
/// documented `$proxy_add_x_forwarded_for` + `$remote_addr` snippet.
fn register_request_with_headers(xff: Option<&str>, real_ip: Option<&str>) -> Request<Body> {
    let mut builder =
        Request::post("/api/auth/register").header("Content-Type", "application/json");
    if let Some(v) = xff {
        builder = builder.header("X-Forwarded-For", v);
    }
    if let Some(v) = real_ip {
        builder = builder.header("X-Real-IP", v);
    }
    let mut req = builder
        .body(Body::from(
            r#"{"email":"a@b.c","password":"xxxxxxxx","display_name":"x"}"#,
        ))
        .unwrap();
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 55555);
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

#[tokio::test]
async fn register_behind_trusted_proxy_ignores_spoofed_leftmost_xff() {
    // Regression guard for the XFF-leftmost-trust bug: nginx's
    // `$proxy_add_x_forwarded_for` APPENDS the real peer to any XFF
    // the client sent, so the real client IP is the rightmost entry.
    // An attacker who sends `X-Forwarded-For: 1.2.3.4` from a single
    // source ends up with `1.2.3.4, <real>` at the gateway. If we
    // read the leftmost entry, the attacker's forged value becomes a
    // fresh bucket key on every request and they can hammer the
    // endpoint indefinitely. Two back-to-back calls from the same
    // forging client must still 429.
    let app = setup_trusting_proxy(Duration::from_millis(200)).await;

    let first = app
        .clone()
        .oneshot(register_request_with_headers(
            Some("1.2.3.4, 203.0.113.7"),
            None,
        ))
        .await
        .unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app
        .clone()
        .oneshot(register_request_with_headers(
            Some("9.9.9.9, 203.0.113.7"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        second.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "spoofed leftmost XFF entries must not reset the bucket when the proxy-appended rightmost is unchanged"
    );
}

#[tokio::test]
async fn register_behind_trusted_proxy_prefers_x_real_ip() {
    // `X-Real-IP` is set by nginx from `$remote_addr` and cannot be
    // forged by the client in the documented single-hop setup. When
    // both headers are present, the limiter must key on `X-Real-IP`
    // so the client cannot use a crafted `X-Forwarded-For` to rotate
    // buckets. Two calls with the same real IP but different XFF
    // chains must still throttle.
    let app = setup_trusting_proxy(Duration::from_millis(200)).await;

    let first = app
        .clone()
        .oneshot(register_request_with_headers(
            Some("1.2.3.4, 203.0.113.7"),
            Some("203.0.113.7"),
        ))
        .await
        .unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app
        .clone()
        .oneshot(register_request_with_headers(
            Some("9.9.9.9, 203.0.113.7"),
            Some("203.0.113.7"),
        ))
        .await
        .unwrap();
    assert_eq!(
        second.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "X-Real-IP must dominate when present"
    );
}

#[tokio::test]
async fn register_behind_trusted_proxy_throttles_same_xff_client() {
    // Pairs with the test above: the limiter must still throttle a
    // single real client hammering behind the proxy. A regression
    // that looked up only the peer would be caught by the first
    // test (distinct XFFs shared the bucket); this one catches the
    // mirror regression where every XFF is ignored and every caller
    // is "new" forever.
    let app = setup_trusting_proxy(Duration::from_millis(200)).await;

    let first = app
        .clone()
        .oneshot(register_request_with_xff("203.0.113.7"))
        .await
        .unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app
        .clone()
        .oneshot(register_request_with_xff("203.0.113.7"))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn register_without_trust_flag_ignores_xff() {
    // Fail-closed default: when the flag is OFF, a spoofed XFF must
    // NOT let an attacker cycle bucket keys. Both calls come from the
    // same socket peer, so the second must be 429 no matter what
    // header the client sets.
    let app = setup_with_limiter(Duration::from_millis(200)).await;

    let first = app
        .clone()
        .oneshot(register_request_with_xff("1.1.1.1"))
        .await
        .unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    let second = app
        .clone()
        .oneshot(register_request_with_xff("2.2.2.2"))
        .await
        .unwrap();
    assert_eq!(
        second.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "spoofed XFF must not reset the bucket when trust flag is off"
    );
}

#[tokio::test]
async fn register_without_connect_info_skips_rate_limit() {
    // Safety-valve contract: if the request has no ConnectInfo (a
    // reverse proxy that forgot to forward the real client, a test
    // harness, etc.) the handler must NOT treat the call as
    // unthrottled-forever-from-unknown; it must pass through the gate
    // so legitimate users don't get blocked. The per-email limiter in
    // AuthService still covers "same user mashing the button".
    let app = setup_with_limiter(Duration::from_millis(200)).await;

    let mk = || {
        Request::post("/api/auth/register")
            .header("Content-Type", "application/json")
            .body(Body::from(
                r#"{"email":"a@b.c","password":"xxxxxxxx","display_name":"x"}"#,
            ))
            .unwrap()
    };

    let first = app.clone().oneshot(mk()).await.unwrap();
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);
    let second = app.clone().oneshot(mk()).await.unwrap();
    assert_ne!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}
