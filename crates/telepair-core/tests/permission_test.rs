use telepair_core::permission::Role;

#[test]
fn owner_has_all_permissions() {
    let role = Role::Owner;
    assert!(role.can_input());
    assert!(role.can_resize());
    assert!(role.can_manage_participants());
    assert!(role.can_close_session());
}

#[test]
fn operator_can_input_but_not_manage() {
    let role = Role::Operator;
    assert!(role.can_input());
    assert!(role.can_resize());
    assert!(!role.can_manage_participants());
    assert!(!role.can_close_session());
}

#[test]
fn viewer_is_read_only() {
    let role = Role::Viewer;
    assert!(!role.can_input());
    assert!(!role.can_resize());
    assert!(!role.can_manage_participants());
    assert!(!role.can_close_session());
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
