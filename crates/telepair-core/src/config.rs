use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_server")]
    pub server: ServerConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub session: SessionDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_storage_type")]
    pub r#type: String,
    #[serde(default = "default_db_path")]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_auth_type")]
    pub r#type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDefaults {
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: u64,
    #[serde(default = "default_max_scrollback")]
    pub max_scrollback: usize,
}

fn default_server() -> ServerConfig {
    ServerConfig {
        host: default_host(),
        port: default_port(),
    }
}
fn default_host() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    7700
}
fn default_storage_type() -> String {
    "sqlite".into()
}
fn default_db_path() -> String {
    "~/.telepair/telepair.db".into()
}
fn default_auth_type() -> String {
    "token".into()
}
fn default_idle_timeout() -> u64 {
    3600
}
fn default_max_scrollback() -> usize {
    10000
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: default_server(),
            storage: StorageConfig::default(),
            auth: AuthConfig::default(),
            session: SessionDefaults::default(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            r#type: default_storage_type(),
            path: default_db_path(),
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            r#type: default_auth_type(),
        }
    }
}

impl Default for SessionDefaults {
    fn default() -> Self {
        Self {
            idle_timeout: default_idle_timeout(),
            max_scrollback: default_max_scrollback(),
        }
    }
}
