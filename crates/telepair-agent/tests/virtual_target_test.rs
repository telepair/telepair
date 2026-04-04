use telepair_agent::virtual_target::TargetEngine;

const TEST_CONFIG: &str = r#"
targets:
  - name: test-echo
    display: "Test Echo"
    command: echo
    args: ["hello", "world"]
    tags: [test]
    required_role: viewer

  - name: local-shell
    display: "Local Shell"
    type: local
"#;

#[test]
fn load_targets_from_yaml() {
    let engine = TargetEngine::from_yaml(TEST_CONFIG).unwrap();
    let targets = engine.list_targets();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].name, "test-echo");
    assert_eq!(targets[1].name, "local-shell");
}

#[test]
fn resolve_virtual_target() {
    let engine = TargetEngine::from_yaml(TEST_CONFIG).unwrap();
    let (cmd, args) = engine.resolve("test-echo").unwrap();
    assert_eq!(cmd, "echo");
    assert_eq!(args, vec!["hello", "world"]);
}

#[test]
fn resolve_local_shell() {
    let engine = TargetEngine::from_yaml(TEST_CONFIG).unwrap();
    let (cmd, args) = engine.resolve("local-shell").unwrap();
    // Should resolve to $SHELL or /bin/sh
    assert!(!cmd.is_empty());
    assert!(args.is_empty());
}

#[test]
fn unknown_target_returns_none() {
    let engine = TargetEngine::from_yaml(TEST_CONFIG).unwrap();
    assert!(engine.resolve("nonexistent").is_none());
}

#[test]
fn default_local_shell_always_present() {
    let engine = TargetEngine::empty();
    let targets = engine.list_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "local-shell");
}
