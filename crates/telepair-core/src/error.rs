use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    /// The session exists in storage but is no longer accepting new
    /// participants / invites / state changes. Maps to `410 Gone` so
    /// the caller learns "the resource used to be here but is now
    /// permanent-retired" rather than the ambiguous 404 "was it ever
    /// here?". Used by `InviteService::create` / `redeem` and the REST
    /// `create_invite` handler.
    #[error("session closed: {0}")]
    SessionClosed(String),
    #[error("target not found: {0}")]
    TargetNotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Internal server-side failure that does not fit the other
    /// variants. Used for cases like "retry loop exhausted against
    /// an otherwise well-defined failure mode" where callers can't
    /// meaningfully react to the specific cause and we just want a
    /// 500 with a descriptive message. Keep the string concise —
    /// it goes into logs and (generically masked) HTTP responses.
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("rate limited: {0}")]
    RateLimited(String),
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("storage error: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// HTTP status classification for this error. Handlers should prefer
    /// this over hand-rolling `map_err(|_| StatusCode::X)` so that a
    /// client-side problem (bad input, expired invite) never bubbles up
    /// as a 500. Anything not explicitly mapped here is treated as an
    /// internal server error.
    pub fn http_status(&self) -> u16 {
        match self {
            // Token missing/invalid or invite expired — client must
            // retry with different credentials.
            Error::Auth(_) => 401,
            Error::PermissionDenied(_) => 403,
            Error::SessionNotFound(_) | Error::TargetNotFound(_) => 404,
            Error::SessionClosed(_) => 410,
            Error::InvalidInput(_) | Error::Json(_) => 400,
            Error::Conflict(_) => 409,
            Error::RateLimited(_) => 429,
            Error::ServiceUnavailable(_) => 503,
            Error::Internal(_) | Error::Storage(_) | Error::Io(_) | Error::Yaml(_) => 500,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_error_variants_have_correct_http_status() {
        assert_eq!(Error::Conflict("dup".into()).http_status(), 409);
        assert_eq!(Error::RateLimited("slow".into()).http_status(), 429);
        assert_eq!(Error::ServiceUnavailable("smtp".into()).http_status(), 503);
    }
}
