use std::collections::HashMap;

use telepair_agent::virtual_target::TargetEngine;
use telepair_core::target::Target;

pub struct TargetService {
    engine: TargetEngine,
}

impl TargetService {
    pub fn new(engine: TargetEngine) -> Self {
        Self { engine }
    }

    pub fn list_targets(&self) -> &[Target] {
        self.engine.list_targets()
    }

    /// Look up a single target by name without cloning the list.
    pub fn find(&self, name: &str) -> Option<&Target> {
        self.engine.find(name)
    }

    pub fn resolve(&self, name: &str) -> Option<(String, Vec<String>, HashMap<String, String>)> {
        self.engine.resolve(name)
    }
}
