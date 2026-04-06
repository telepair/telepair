use telepair_core::protocol::{ClientMessage, ServerMessage};

#[test]
fn client_term_input_roundtrip() {
    let msg = ClientMessage::TermInput {
        data: vec![0x1b, 0x5b, 0x41],
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
