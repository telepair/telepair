use telepair_core::protocol::{
    CLOSE_CODE_TERMINAL, CLOSE_CODE_TRANSIENT, ClientMessage, ServerMessage, close_code_for,
    error_codes,
};

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

// The STORAGE_ERROR handshake path advertises itself as transient in
// the protocol comment ("Clients should retry"). That contract is only
// real if the WebSocket close frame carries a non-terminal code — the
// frontend switches on `event.code` in `web/src/lib/ws.ts` and treats
// 4001 as a dead end. A regression here means a one-off SQLite hiccup
// strands users on a broken page until they manually reload.
#[test]
fn storage_error_uses_transient_close_code() {
    assert_eq!(
        close_code_for(error_codes::STORAGE_ERROR),
        CLOSE_CODE_TRANSIENT,
        "STORAGE_ERROR is transient — must not use the terminal close code"
    );
    assert_ne!(
        CLOSE_CODE_TRANSIENT, CLOSE_CODE_TERMINAL,
        "transient and terminal close codes must be distinct or the client cannot tell them apart"
    );
}

// Permission / not-found / session-closed errors are permanent for the
// current credential. The client must NOT silently retry on these, or
// a revoked token would DoS the gateway with reconnect storms.
#[test]
fn permanent_errors_use_terminal_close_code() {
    for code in [
        error_codes::AUTH_FAILED,
        error_codes::AUTH_TIMEOUT,
        error_codes::EXPECTED_JOIN,
        error_codes::SESSION_NOT_FOUND,
        error_codes::SESSION_CLOSED,
        error_codes::NOT_PARTICIPANT,
        error_codes::TARGET_NOT_FOUND,
        error_codes::PTY_ERROR,
    ] {
        assert_eq!(
            close_code_for(code),
            CLOSE_CODE_TERMINAL,
            "{code} must be terminal so the client does not reconnect-loop"
        );
    }
}
