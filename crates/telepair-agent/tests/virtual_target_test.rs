use telepair_agent::virtual_target::TargetEngine;

const TEST_CONFIG: &str = r#"
targets:
  - name: test-echo
    display: "Test Echo"
    command: echo
    args: ["hello", "world"]
    tags: [test]

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
    let (cmd, args, env) = engine.resolve("test-echo").unwrap();
    assert_eq!(cmd, "echo");
    assert_eq!(args, vec!["hello", "world"]);
    assert!(env.is_empty());
}

#[test]
fn resolve_local_shell() {
    let engine = TargetEngine::from_yaml(TEST_CONFIG).unwrap();
    let (cmd, args, env) = engine.resolve("local-shell").unwrap();
    // Should resolve to $SHELL or /bin/sh
    assert!(!cmd.is_empty());
    assert!(args.is_empty());
    assert!(env.is_empty());
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

#[test]
fn duplicate_target_names_rejected() {
    let yaml = r#"
targets:
  - name: dup
    display: First
    command: echo
  - name: dup
    display: Second
    command: printf
"#;
    let err = TargetEngine::from_yaml(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("duplicate name"),
        "expected duplicate error, got: {msg}"
    );
    assert!(
        msg.contains("dup"),
        "error should name the duplicate: {msg}"
    );
}

#[test]
fn multiple_duplicate_names_all_reported() {
    let yaml = r#"
targets:
  - name: alpha
    display: A1
    command: echo
  - name: beta
    display: B1
    command: echo
  - name: alpha
    display: A2
    command: echo
  - name: beta
    display: B2
    command: echo
"#;
    let err = TargetEngine::from_yaml(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("alpha"), "should mention alpha: {msg}");
    assert!(msg.contains("beta"), "should mention beta: {msg}");
}

#[test]
fn unique_names_accepted() {
    let yaml = r#"
targets:
  - name: one
    display: One
    command: echo
  - name: two
    display: Two
    command: printf
"#;
    let engine = TargetEngine::from_yaml(yaml).unwrap();
    assert!(engine.find("one").is_some());
    assert!(engine.find("two").is_some());
}

// ── Validation: command ──────────────────────────────────────────

#[test]
fn virtual_target_missing_command_rejected() {
    let yaml = r#"
targets:
  - name: no-cmd
    display: "No Command"
"#;
    let err = TargetEngine::from_yaml(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("requires a command"), "got: {msg}");
    assert!(msg.contains("no-cmd"), "should name the target: {msg}");
}

#[test]
fn virtual_target_empty_command_rejected() {
    let yaml = r#"
targets:
  - name: empty-cmd
    display: "Empty"
    command: ""
"#;
    let err = TargetEngine::from_yaml(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("command is blank"), "got: {msg}");
}

#[test]
fn virtual_target_whitespace_command_rejected() {
    let yaml = r#"
targets:
  - name: ws-cmd
    display: "Whitespace"
    command: "   "
"#;
    let err = TargetEngine::from_yaml(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("command is blank"), "got: {msg}");
}

// ── Validation: name / display ───────────────────────────────────

#[test]
fn empty_name_rejected() {
    let yaml = r#"
targets:
  - name: ""
    display: "Has Display"
    command: echo
"#;
    let err = TargetEngine::from_yaml(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("name is empty"), "got: {msg}");
}

#[test]
fn empty_display_rejected() {
    let yaml = r#"
targets:
  - name: no-display
    display: ""
    command: echo
"#;
    let err = TargetEngine::from_yaml(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("display is empty"), "got: {msg}");
}

// ── Regression: local target without command is fine ─────────────

#[test]
fn local_target_without_command_accepted() {
    let yaml = r#"
targets:
  - name: my-shell
    display: "My Shell"
    type: local
"#;
    let engine = TargetEngine::from_yaml(yaml).unwrap();
    assert!(engine.find("my-shell").is_some());
}

// ── Built-in local-shell collision ──────────────────────────────

#[test]
fn virtual_target_named_local_shell_rejected() {
    let yaml = r#"
targets:
  - name: local-shell
    display: "Fake Shell"
    command: echo
"#;
    let err = TargetEngine::from_yaml(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("duplicate name"),
        "virtual target shadowing built-in local-shell should be rejected: {msg}"
    );
}

// ── Multiple validation errors reported at once ──────────────────

#[test]
fn multiple_validation_errors_all_reported() {
    let yaml = r#"
targets:
  - name: ""
    display: "No Name"
    command: echo
  - name: blank-cmd
    display: ""
    command: "  "
"#;
    let err = TargetEngine::from_yaml(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("name is empty"),
        "should flag empty name: {msg}"
    );
    assert!(
        msg.contains("display is empty"),
        "should flag empty display: {msg}"
    );
    assert!(
        msg.contains("command is blank"),
        "should flag blank command: {msg}"
    );
}
