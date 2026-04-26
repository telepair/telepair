use axum::http::{HeaderValue, Uri};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// Default browser origins allowed when the operator does not pass
/// `--allowed-origins` or `--allow-any-origin`. Same-origin production
/// traffic is allowed by matching the `Origin` authority against `Host`;
/// these entries are for the Vite dev server talking to the gateway on
/// a different port.
pub const DEFAULT_LOOPBACK_ORIGINS: &[&str] = &["http://localhost:5173", "http://127.0.0.1:5173"];

/// Shared browser-origin policy for HTTP CORS and WebSocket upgrades.
///
/// HTTP CORS is enforced by browsers after the response. WebSocket
/// handshakes are different: the server must inspect `Origin` before
/// accepting the upgrade. Keeping both on one policy prevents the two
/// browser entry points from drifting.
///
/// `Default` is the conservative `Origins(vec![])` — denies every
/// browser origin until `build_router_with_options` installs the
/// operator-chosen policy. Non-browser clients (no `Origin` header)
/// still pass and authenticate via the normal bearer-token handshake.
#[derive(Clone, Debug)]
pub enum OriginPolicy {
    AllowAny,
    Origins(Vec<HeaderValue>),
}

impl Default for OriginPolicy {
    fn default() -> Self {
        Self::Origins(Vec::new())
    }
}

impl OriginPolicy {
    pub fn allow_any() -> Self {
        Self::AllowAny
    }

    pub fn origins(origins: Vec<HeaderValue>) -> Self {
        Self::Origins(origins)
    }

    pub fn cors_layer(&self) -> CorsLayer {
        let layer = CorsLayer::new().allow_methods(Any).allow_headers(Any);
        match self {
            Self::AllowAny => layer.allow_origin(Any),
            Self::Origins(origins) => layer.allow_origin(AllowOrigin::list(origins.clone())),
        }
    }

    /// Decide whether a WebSocket upgrade may proceed.
    ///
    /// Browser WebSocket handshakes include `Origin`; non-browser clients
    /// often omit it, so a missing origin is accepted and still must pass
    /// the normal `SessionJoin` bearer-token handshake. Present origins
    /// must either be explicitly listed or be same-host with the request.
    pub fn allows_ws_origin(
        &self,
        origin: Option<&HeaderValue>,
        host: Option<&HeaderValue>,
    ) -> bool {
        let Some(origin) = origin else {
            return true;
        };
        match self {
            Self::AllowAny => true,
            Self::Origins(origins) => {
                origins.iter().any(|allowed| allowed == origin)
                    || origin_authority_matches_host(origin, host)
            }
        }
    }
}

fn origin_authority_matches_host(origin: &HeaderValue, host: Option<&HeaderValue>) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Some(host) = host.and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let Ok(uri) = origin.parse::<Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    authority.as_str().eq_ignore_ascii_case(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_origin_is_allowed() {
        let policy = OriginPolicy::origins(vec![HeaderValue::from_static("https://app.example")]);

        assert!(policy.allows_ws_origin(
            Some(&HeaderValue::from_static("https://app.example")),
            Some(&HeaderValue::from_static("api.example")),
        ));
    }

    #[test]
    fn same_host_origin_is_allowed() {
        let policy = OriginPolicy::origins(vec![HeaderValue::from_static("http://localhost:5173")]);

        assert!(policy.allows_ws_origin(
            Some(&HeaderValue::from_static("https://telepair.example.com")),
            Some(&HeaderValue::from_static("telepair.example.com")),
        ));
    }

    #[test]
    fn unrelated_origin_is_rejected() {
        let policy = OriginPolicy::origins(vec![HeaderValue::from_static("https://app.example")]);

        assert!(!policy.allows_ws_origin(
            Some(&HeaderValue::from_static("https://evil.example")),
            Some(&HeaderValue::from_static("telepair.example.com")),
        ));
    }

    #[test]
    fn missing_origin_passes_for_non_browser_clients() {
        let policy = OriginPolicy::default();
        assert!(policy.allows_ws_origin(None, Some(&HeaderValue::from_static("any.example"))));
    }

    #[test]
    fn default_rejects_unknown_browser_origin() {
        let policy = OriginPolicy::default();
        assert!(!policy.allows_ws_origin(
            Some(&HeaderValue::from_static("https://attacker.example")),
            Some(&HeaderValue::from_static("telepair.example.com")),
        ));
    }
}
