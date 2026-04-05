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
fn server_term_output_roundtrip() {
    let msg = ServerMessage::TermOutput {
        data: vec![0x48, 0x65, 0x6c, 0x6c, 0x6f],
    };
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: ServerMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
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

#[test]
fn binary_frame_encode_decode() {
    use telepair_core::protocol::{BinaryFrame, BinaryFrameType};
    let frame = BinaryFrame {
        frame_type: BinaryFrameType::Output,
        payload: b"hello world".to_vec(),
    };
    let bytes = frame.encode().unwrap();
    assert_eq!(bytes[0], 0x01);
    assert_eq!(u16::from_be_bytes([bytes[1], bytes[2]]), 11);
    let decoded = BinaryFrame::decode(&bytes).unwrap();
    assert_eq!(decoded.frame_type, BinaryFrameType::Output);
    assert_eq!(decoded.payload, b"hello world");
}

#[test]
fn binary_frame_resize() {
    use telepair_core::protocol::BinaryFrame;
    let frame = BinaryFrame::resize(120, 40);
    let bytes = frame.encode().unwrap();
    let decoded = BinaryFrame::decode(&bytes).unwrap();
    let (cols, rows) = decoded.parse_resize().unwrap();
    assert_eq!(cols, 120);
    assert_eq!(rows, 40);
}
