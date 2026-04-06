use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("target not found: {0}")]
    TargetNotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("database schema is outdated: {0}")]
    SchemaOutdated(String),
    #[error("storage error: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
