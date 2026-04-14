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

// ── TargetEngine::diff ───────────────────────────────────────────

const OLD_YAML: &str = r#"
targets:
  - name: keep-same
    display: Keep Same
    command: echo
  - name: will-change
    display: Old Display
    command: old-cmd
  - name: will-remove
    display: Remove Me
    command: rm
"#;

const NEW_YAML: &str = r#"
targets:
  - name: keep-same
    display: Keep Same
    command: echo
  - name: will-change
    display: New Display
    command: new-cmd
  - name: brand-new
    display: Brand New
    command: true
"#;

#[test]
fn diff_detects_added_removed_changed_unchanged() {
    let old = TargetEngine::from_yaml(OLD_YAML).unwrap();
    let new = TargetEngine::from_yaml(NEW_YAML).unwrap();
    let diff = old.diff(&new);

    assert_eq!(diff.added, vec!["brand-new"]);
    assert_eq!(diff.removed, vec!["will-remove"]);
    assert_eq!(diff.changed, vec!["will-change"]);
    // from_yaml injects local-shell into both sides, so it appears in unchanged
    assert_eq!(diff.unchanged, vec!["keep-same", "local-shell"]);
}

#[test]
fn diff_empty_to_populated() {
    let old = TargetEngine::empty();
    let new = TargetEngine::from_yaml(NEW_YAML).unwrap();
    let diff = old.diff(&new);

    // from_yaml injects local-shell into NEW_YAML too, so it matches empty()'s local-shell
    // → local-shell ends up in unchanged, not removed
    assert!(diff.removed.is_empty());
    assert_eq!(diff.added, vec!["brand-new", "keep-same", "will-change"]);
    assert!(diff.changed.is_empty());
    assert_eq!(diff.unchanged, vec!["local-shell"]);
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
