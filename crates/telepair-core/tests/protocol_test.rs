use telepair_core::protocol::{ClientMessage, ServerMessage};

#[test]
fn client_term_resize_roundtrip() {
    // Terminal input itself travels as a raw binary WS frame and has no JSON
    // variant. Resize is the closest structured input-path message.
    let msg = ClientMessage::TermResize {
        cols: 120,
        rows: 40,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: ClientMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn client_session_join_json() {
    let msg = ClientMessage::SessionJoin {
        session_id: "abc123".into(),
        token: "tok_secret".into(),
        cols: 120,
        rows: 40,
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("SessionJoin"));
    assert!(json.contains("abc123"));
}

#[test]
fn server_error_json() {
    let msg = ServerMessage::Error {
        code: "PERM_DENIED".into(),
        message: "you cannot type in this session".into(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("PERM_DENIED"));
}
