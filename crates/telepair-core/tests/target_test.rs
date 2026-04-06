use telepair_core::target::{TargetConfig, TargetKind};

const YAML_CONFIG: &str = r#"
targets:
  - name: production-db
    display: "Production DB"
    command: psql
    args: ["-h", "db.internal", "-U", "readonly"]
    env:
      PGPASSWORD: "${PROD_DB_PASS}"
    tags: [database]
    admin_only: true

  - name: local-shell
    display: "Local Shell"
    type: local
"#;

#[test]
fn parse_targets_yaml() {
    let config: TargetConfig = serde_yaml::from_str(YAML_CONFIG).unwrap();
    assert_eq!(config.targets.len(), 2);

    let db = &config.targets[0];
    assert_eq!(db.name, "production-db");
    assert_eq!(db.display, "Production DB");
    assert_eq!(db.kind, TargetKind::Virtual);
    assert_eq!(db.command.as_deref(), Some("psql"));
    assert_eq!(db.args, vec!["-h", "db.internal", "-U", "readonly"]);
    assert_eq!(db.env.get("PGPASSWORD").unwrap(), "${PROD_DB_PASS}");
    assert!(db.admin_only);

    let shell = &config.targets[1];
    assert_eq!(shell.kind, TargetKind::Local);
    assert!(shell.command.is_none());
    assert!(!shell.admin_only, "admin_only must default to false");
}

#[test]
fn env_var_substitution() {
    // SAFETY: single-threaded test — no concurrent env access
    unsafe { std::env::set_var("TEST_VAR_TELEPAIR", "secret123") };
    let result = telepair_core::target::substitute_env_vars("prefix_${TEST_VAR_TELEPAIR}_suffix");
    assert_eq!(result, "prefix_secret123_suffix");
    unsafe { std::env::remove_var("TEST_VAR_TELEPAIR") };
}

#[test]
fn missing_env_var_kept_as_is() {
    let result = telepair_core::target::substitute_env_vars("${DEFINITELY_NOT_SET_XYZ}");
    assert_eq!(result, "${DEFINITELY_NOT_SET_XYZ}");
}
