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

    pub fn resolve(&self, name: &str) -> Option<(String, Vec<String>)> {
        self.engine.resolve(name)
    }
}
