use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Operator,
    Viewer,
}

impl Role {
    pub fn can_input(&self) -> bool {
        matches!(self, Self::Owner | Self::Operator)
    }

    pub fn can_resize(&self) -> bool {
        matches!(self, Self::Owner | Self::Operator)
    }

    pub fn can_manage_participants(&self) -> bool {
        matches!(self, Self::Owner)
    }

    pub fn can_close_session(&self) -> bool {
        matches!(self, Self::Owner)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Operator => "operator",
            Self::Viewer => "viewer",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
