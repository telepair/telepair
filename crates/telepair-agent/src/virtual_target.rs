use telepair_core::target::{substitute_env_vars, Target, TargetConfig, TargetKind};

pub struct TargetEngine {
    targets: Vec<Target>,
}

impl TargetEngine {
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        let mut config: TargetConfig = serde_yaml::from_str(yaml)?;
        // Ensure local-shell exists
        if !config.targets.iter().any(|t| t.kind == TargetKind::Local) {
            config.targets.push(default_local_shell());
        }
        Ok(Self {
            targets: config.targets,
        })
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Ok(Self::from_yaml(&content)?)
    }

    pub fn empty() -> Self {
        Self {
            targets: vec![default_local_shell()],
        }
    }

    pub fn list_targets(&self) -> &[Target] {
        &self.targets
    }

    /// Resolve a target name to (command, args) with env substitution applied.
    pub fn resolve(&self, name: &str) -> Option<(String, Vec<String>)> {
        let target = self.targets.iter().find(|t| t.name == name)?;
        match target.kind {
            TargetKind::Local => {
                let shell = target
                    .shell
                    .as_deref()
                    .map(substitute_env_vars)
                    .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()));
                Some((shell, vec![]))
            }
            TargetKind::Virtual => {
                let cmd = substitute_env_vars(target.command.as_deref()?);
                let args: Vec<String> =
                    target.args.iter().map(|a| substitute_env_vars(a)).collect();
                Some((cmd, args))
            }
        }
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
        required_role: None,
        shell: None,
    }
}
