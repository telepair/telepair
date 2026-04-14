use std::collections::{HashMap, HashSet};

use serde::Serialize;
use telepair_core::error::{Error, Result};
use telepair_core::target::{Target, TargetConfig, TargetKind, substitute_env_vars};

#[derive(Debug)]
pub struct TargetEngine {
    targets: Vec<Target>,
}

impl TargetEngine {
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let mut config: TargetConfig = serde_yaml::from_str(yaml)?;

        // Ensure local-shell exists before validation so duplicate-name
        // checks also cover the built-in target.
        if !config.targets.iter().any(|t| t.kind == TargetKind::Local) {
            config.targets.push(default_local_shell());
        }

        validate_targets(&config.targets)?;

        Ok(Self {
            targets: config.targets,
        })
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_yaml(&content)
    }

    pub fn empty() -> Self {
        Self {
            targets: vec![default_local_shell()],
        }
    }

    pub fn list_targets(&self) -> &[Target] {
        &self.targets
    }

    /// Look up a target by name without cloning. Returns `None` if missing.
    pub fn find(&self, name: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.name == name)
    }

    /// Resolve a target name to (command, args, env) with env substitution applied.
    pub fn resolve(&self, name: &str) -> Option<(String, Vec<String>, HashMap<String, String>)> {
        let target = self.targets.iter().find(|t| t.name == name)?;
        match target.kind {
            TargetKind::Local => {
                let shell = target
                    .shell
                    .as_deref()
                    .map(substitute_env_vars)
                    .unwrap_or_else(crate::default_shell);
                Some((shell, vec![], HashMap::new()))
            }
            TargetKind::Virtual => {
                let cmd = substitute_env_vars(target.command.as_deref()?);
                let args: Vec<String> =
                    target.args.iter().map(|a| substitute_env_vars(a)).collect();
                let env: HashMap<String, String> = target
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), substitute_env_vars(v)))
                    .collect();
                Some((cmd, args, env))
            }
        }
    }

    /// Compare `self` (old state) with `other` (new state) and return a diff.
    ///
    /// - `added`: name present in `other` but not in `self`
    /// - `removed`: name present in `self` but not in `other`
    /// - `changed`: name present in both but the `Target` value differs
    /// - `unchanged`: identical in both
    ///
    /// All lists are sorted alphabetically.
    pub fn diff(&self, other: &TargetEngine) -> TargetDiff {
        let old_map: HashMap<&str, &Target> =
            self.targets.iter().map(|t| (t.name.as_str(), t)).collect();
        let new_map: HashMap<&str, &Target> =
            other.targets.iter().map(|t| (t.name.as_str(), t)).collect();

        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut changed = Vec::new();
        let mut unchanged = Vec::new();

        for (name, new_target) in &new_map {
            match old_map.get(name) {
                None => added.push(name.to_string()),
                Some(old_target) => {
                    if old_target == new_target {
                        unchanged.push(name.to_string());
                    } else {
                        changed.push(name.to_string());
                    }
                }
            }
        }

        for name in old_map.keys() {
            if !new_map.contains_key(name) {
                removed.push(name.to_string());
            }
        }

        added.sort();
        removed.sort();
        changed.sort();
        unchanged.sort();

        TargetDiff {
            added,
            removed,
            changed,
            unchanged,
        }
    }
}

/// Validate business rules that serde cannot enforce:
/// - every target must have a non-blank name and display
/// - every Virtual target must have a non-blank command
/// - no duplicate target names (Vec-based lookup returns the first
///   match, so later duplicates would be silently shadowed)
fn validate_targets(targets: &[Target]) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for (i, t) in targets.iter().enumerate() {
        let name_empty = t.name.trim().is_empty();
        let label = if name_empty {
            format!("targets[{i}]")
        } else {
            format!("target '{}'", t.name)
        };
        if name_empty {
            errors.push(format!("{label}: name is empty"));
        } else if !seen.insert(t.name.as_str()) {
            errors.push(format!("{label}: duplicate name"));
        }
        if t.display.trim().is_empty() {
            errors.push(format!("{label}: display is empty"));
        }
        if t.kind == TargetKind::Virtual {
            match &t.command {
                None => errors.push(format!("{label}: virtual target requires a command")),
                Some(cmd) if cmd.trim().is_empty() => {
                    errors.push(format!("{label}: command is blank"))
                }
                _ => {}
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "invalid target config: {}",
            errors.join("; ")
        )))
    }
}

fn default_local_shell() -> Target {
    Target {
        name: "local-shell".into(),
        display: "Local Shell".into(),
        kind: TargetKind::Local,
        command: None,
        args: vec![],
        env: Default::default(),
        tags: vec![],
        admin_only: false,
        shell: None,
    }
}

/// Result of comparing two `TargetEngine` states.
#[derive(Debug, Clone, Serialize)]
pub struct TargetDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    pub unchanged: Vec<String>,
}
