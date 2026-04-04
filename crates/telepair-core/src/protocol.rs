use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::permission::Role;
use crate::session::Session;

// --- Client -> Server ---
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    SessionJoin { session_id: String, token: String },
    TermInput { data: Vec<u8> },
    TermResize { cols: u16, rows: u16 },
    CursorMove { x: u16, y: u16 },
    ChatMessage { text: String },
}

// --- Server -> Client ---
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    SessionState {
        session: Session,
        participants: Vec<ParticipantInfo>,
        your_role: Role,
    },
    TermOutput {
        data: Vec<u8>,
    },
    PeerJoined {
        user_id: Uuid,
        name: String,
        role: Role,
        color: String,
    },
    PeerLeft {
        user_id: Uuid,
    },
    PeerCursor {
        user_id: Uuid,
        x: u16,
        y: u16,
    },
    PeerChat {
        user_id: Uuid,
        name: String,
        text: String,
        ts: String,
    },
    PermUpdate {
        user_id: Uuid,
        new_role: Role,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub user_id: Uuid,
    pub name: String,
    pub role: Role,
    pub color: String,
}

// --- Binary Frame Protocol ---
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BinaryFrameType {
    Output = 0x01,
    Input = 0x02,
    Resize = 0x03,
}

impl BinaryFrameType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Output),
            0x02 => Some(Self::Input),
            0x03 => Some(Self::Resize),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryFrame {
    pub frame_type: BinaryFrameType,
    pub payload: Vec<u8>,
}

impl BinaryFrame {
    pub fn encode(&self) -> Vec<u8> {
        let len = u16::try_from(self.payload.len())
            .expect("BinaryFrame payload exceeds u16::MAX (65535) bytes");
        let mut buf = Vec::with_capacity(3 + self.payload.len());
        buf.push(self.frame_type as u8);
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 3 {
            return None;
        }
        let frame_type = BinaryFrameType::from_byte(data[0])?;
        let len = u16::from_be_bytes([data[1], data[2]]) as usize;
        if data.len() < 3 + len {
            return None;
        }
        Some(Self {
            frame_type,
            payload: data[3..3 + len].to_vec(),
        })
    }

    pub fn resize(cols: u16, rows: u16) -> Self {
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&cols.to_be_bytes());
        payload.extend_from_slice(&rows.to_be_bytes());
        Self {
            frame_type: BinaryFrameType::Resize,
            payload,
        }
    }

    pub fn parse_resize(&self) -> Option<(u16, u16)> {
        if self.frame_type != BinaryFrameType::Resize || self.payload.len() != 4 {
            return None;
        }
        let cols = u16::from_be_bytes([self.payload[0], self.payload[1]]);
        let rows = u16::from_be_bytes([self.payload[2], self.payload[3]]);
        Some((cols, rows))
    }
}
