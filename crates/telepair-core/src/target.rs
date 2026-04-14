use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    #[default]
    Virtual,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    pub display: String,
    #[serde(default, rename = "type")]
    pub kind: TargetKind,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Restrict session creation on this target to admin users. The
    /// previous spelling was `required_role: Option<Role>` — that name
    /// was misleading because the handler never looked at the user's
    /// actual role, only `is_admin`. Everything short of `Viewer` was
    /// effectively "admin-only" and `Viewer` was a silent wildcard.
    /// Spelling it `admin_only: bool` matches the real semantics and
    /// kills the wildcard footgun.
    #[serde(default)]
    pub admin_only: bool,
    pub shell: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    pub targets: Vec<Target>,
}

pub fn substitute_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next();
            let var_name: String = chars.by_ref().take_while(|&c| c != '}').collect();
            match std::env::var(&var_name) {
                Ok(val) => result.push_str(&val),
                Err(_) => {
                    result.push_str("${");
                    result.push_str(&var_name);
                    result.push('}');
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}
