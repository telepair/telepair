use telepair_core::permission::Role;

#[test]
fn owner_can_input_and_resize() {
    let role = Role::Owner;
    assert!(role.can_input());
    assert!(role.can_resize());
}

#[test]
fn operator_can_input_and_resize() {
    let role = Role::Operator;
    assert!(role.can_input());
    assert!(role.can_resize());
}

#[test]
fn viewer_is_read_only() {
    let role = Role::Viewer;
    assert!(!role.can_input());
    assert!(!role.can_resize());
}

#[test]
fn role_serializes_as_lowercase() {
    let json = serde_json::to_string(&Role::Owner).unwrap();
    assert_eq!(json, r#""owner""#);
    let parsed: Role = serde_json::from_str(r#""operator""#).unwrap();
    assert_eq!(parsed, Role::Operator);
}

#[test]
fn role_display() {
    assert_eq!(Role::Owner.as_str(), "owner");
    assert_eq!(Role::Operator.as_str(), "operator");
    assert_eq!(Role::Viewer.as_str(), "viewer");
}
