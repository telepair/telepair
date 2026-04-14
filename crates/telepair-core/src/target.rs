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

/// Replace `${VAR}` placeholders with the corresponding environment
/// variable value. Unresolved variables pass through verbatim.
///
/// `$$` is **only** treated as an escape when it immediately precedes
/// `{` — `$${VAR}` emits a literal `${VAR}` with no lookup. Any other
/// `$$` sequence (e.g. `$$` as shell PID, or secrets like `pa$$word`)
/// passes through untouched. Earlier versions of this helper collapsed
/// every `$$` to a single `$`, which silently mutated argv and env for
/// virtual targets whose commands legitimately contained `$$`.
pub fn substitute_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(idx) = rest.find('$') {
        result.push_str(&rest[..idx]);
        let tail = &rest[idx..];

        if let Some(after) = tail.strip_prefix("$${")
            && let Some(close) = after.find('}')
        {
            result.push('$');
            result.push('{');
            result.push_str(&after[..close]);
            result.push('}');
            rest = &after[close + 1..];
        } else if let Some(after) = tail.strip_prefix("${")
            && let Some(close) = after.find('}')
        {
            let var_name = &after[..close];
            match std::env::var(var_name) {
                Ok(val) => result.push_str(&val),
                Err(_) => {
                    result.push_str("${");
                    result.push_str(var_name);
                    result.push('}');
                }
            }
            rest = &after[close + 1..];
        } else {
            result.push('$');
            rest = &tail[1..];
        }
    }
    result.push_str(rest);
    result
}
