use bytes::Bytes;
use telepair_core::recording::{
    AsciicastHeader, RecordingEvent, RecordingRow, RecordingShareRow, RecordingStatus,
};

#[test]
fn asciicast_header_serializes_correctly() {
    let header = AsciicastHeader {
        version: 2,
        width: 120,
        height: 40,
        timestamp: 1713264000,
        env: std::collections::HashMap::from([("TERM".into(), "xterm-256color".into())]),
        telepair: serde_json::json!({
            "session_id": "abc123",
            "owner": {"id": "uuid", "name": "alice"},
            "input_mode": "multiplexed",
            "target_name": "default"
        }),
    };
    let json = serde_json::to_string(&header).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["version"], 2);
    assert_eq!(parsed["width"], 120);
    assert_eq!(parsed["telepair"]["session_id"], "abc123");
}

#[test]
fn recording_event_output_serializes_as_asciicast_v2() {
    let event = RecordingEvent::Output(Bytes::from_static(b"$ "));
    let line = event.to_asciicast_line(0.0);
    assert_eq!(line, r#"[0.000000, "o", "$ "]"#);
}

#[test]
fn recording_event_output_escapes_special_chars() {
    let event = RecordingEvent::Output(Bytes::from_static(b"hello\r\n"));
    let line = event.to_asciicast_line(1.5);
    assert_eq!(line, r#"[1.500000, "o", "hello\r\n"]"#);
}

#[test]
fn recording_event_resize_serializes() {
    let event = RecordingEvent::Resize {
        cols: 120,
        rows: 30,
    };
    let line = event.to_asciicast_line(2.5);
    assert_eq!(line, r#"[2.500000, "r", "120x30"]"#);
}

#[test]
fn recording_event_join_serializes() {
    let event = RecordingEvent::ParticipantJoin {
        user_id: "uid1".into(),
        name: "bob".into(),
        role: "operator".into(),
    };
    let line = event.to_asciicast_line(3.1);
    assert!(line.starts_with("[3.100000, \"j\", "));
    assert!(line.contains("\"user_id\":\"uid1\""));
}

#[test]
fn recording_event_leave_serializes() {
    let event = RecordingEvent::ParticipantLeave {
        user_id: "uid1".into(),
    };
    let line = event.to_asciicast_line(5.2);
    assert!(line.starts_with("[5.200000, \"l\", "));
    assert!(line.contains("\"user_id\":\"uid1\""));
}

#[test]
fn recording_event_chat_serializes() {
    let event = RecordingEvent::Chat {
        user_id: "uid1".into(),
        name: "bob".into(),
        text: "look here".into(),
    };
    let line = event.to_asciicast_line(6.0);
    assert!(line.starts_with("[6.000000, \"c\", "));
    assert!(line.contains("\"text\":\"look here\""));
}

#[test]
fn recording_status_display() {
    assert_eq!(RecordingStatus::Recording.as_str(), "recording");
    assert_eq!(RecordingStatus::Completed.as_str(), "completed");
    assert_eq!(RecordingStatus::Failed.as_str(), "failed");
}

// Ensure these types are used (suppress dead_code warnings in tests)
#[test]
fn recording_row_and_share_row_are_constructible() {
    let _row = RecordingRow {
        id: "id1".into(),
        session_id: "sess1".into(),
        status: "recording".into(),
        file_path: None,
        file_size: 0,
        duration_ms: None,
        width: 120,
        height: 40,
        event_count: 0,
        started_at: "2024-01-01T00:00:00Z".into(),
        completed_at: None,
        expires_at: None,
        created_by: "user1".into(),
    };
    let _share = RecordingShareRow {
        token_sha256: "abc".into(),
        recording_id: "id1".into(),
        max_uses: 10,
        used_count: 0,
        expires_at: None,
        created_at: "2024-01-01T00:00:00Z".into(),
    };
}
