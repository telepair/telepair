use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Asciicast v2 format types ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsciicastHeader {
    pub version: u8,
    pub width: u16,
    pub height: u16,
    pub timestamp: i64,
    pub env: HashMap<String, String>,
    pub telepair: serde_json::Value,
}

/// Events captured during recording.
/// `Stop` is a control signal — not serialized to the .cast file.
#[derive(Debug, Clone)]
pub enum RecordingEvent {
    Output(Bytes),
    Resize {
        cols: u16,
        rows: u16,
    },
    ParticipantJoin {
        user_id: String,
        name: String,
        role: String,
    },
    ParticipantLeave {
        user_id: String,
    },
    Chat {
        user_id: String,
        name: String,
        text: String,
    },
    Stop,
}

impl RecordingEvent {
    /// Serialize to a single asciicast v2 NDJSON line.
    pub fn to_asciicast_line(&self, elapsed_secs: f64) -> String {
        match self {
            Self::Output(data) => {
                let s = String::from_utf8_lossy(data.as_ref());
                let escaped = serde_json::to_string(s.as_ref()).unwrap_or_default();
                format!("[{elapsed_secs:.6}, \"o\", {escaped}]")
            }
            Self::Resize { cols, rows } => {
                format!("[{elapsed_secs:.6}, \"r\", \"{cols}x{rows}\"]")
            }
            Self::ParticipantJoin {
                user_id,
                name,
                role,
            } => {
                let obj = serde_json::json!({
                    "user_id": user_id, "name": name, "role": role
                });
                let serialized = serde_json::to_string(&obj).unwrap_or_default();
                format!("[{elapsed_secs:.6}, \"j\", {serialized}]")
            }
            Self::ParticipantLeave { user_id } => {
                let obj = serde_json::json!({ "user_id": user_id });
                let serialized = serde_json::to_string(&obj).unwrap_or_default();
                format!("[{elapsed_secs:.6}, \"l\", {serialized}]")
            }
            Self::Chat {
                user_id,
                name,
                text,
            } => {
                let obj = serde_json::json!({
                    "user_id": user_id, "name": name, "text": text
                });
                let serialized = serde_json::to_string(&obj).unwrap_or_default();
                format!("[{elapsed_secs:.6}, \"c\", {serialized}]")
            }
            Self::Stop => unreachable!("Stop is a control signal, not serialized"),
        }
    }
}

// ── Database row types ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordingStatus {
    Recording,
    Completed,
    Failed,
}

impl RecordingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "recording" => Some(Self::Recording),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingRow {
    pub id: String,
    pub session_id: String,
    pub status: String,
    pub file_path: Option<String>,
    pub file_size: i64,
    pub duration_ms: Option<i64>,
    pub width: i64,
    pub height: i64,
    pub event_count: i64,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingShareRow {
    pub token_sha256: String,
    pub recording_id: String,
    pub max_uses: i64,
    pub used_count: i64,
    pub expires_at: Option<String>,
    pub created_at: String,
}

// ── Recording config ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RecordingConfig {
    pub enabled: bool,
    pub ttl_days: u32,
    pub dir: std::path::PathBuf,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ttl_days: 30,
            dir: std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".telepair")
                .join("recordings"),
        }
    }
}
