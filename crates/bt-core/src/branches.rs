use crate::{BranchId, ConnectionId, EventId, SessionId, SessionToolMode, TurnId};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub const fn default_settings_revision_id() -> u64 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionStatus {
    Active,
    Completed,
    Failed,
    Abandoned,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: SessionId,
    pub project_root: Utf8PathBuf,
    pub connection_id: ConnectionId,
    pub model_id: Option<String>,
    pub tool_mode: SessionToolMode,
    #[serde(default = "default_settings_revision_id")]
    pub settings_revision_id: u64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub status: SessionStatus,
    pub display_name: Option<String>,
    pub objective: Option<String>,
    pub parent_session_id: Option<SessionId>,
    pub parent_branch_id: Option<BranchId>,
    pub parent_turn_id: Option<TurnId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSettingsSnapshot {
    pub session_id: SessionId,
    #[serde(default = "default_settings_revision_id")]
    pub settings_revision_id: u64,
    pub connection_id: ConnectionId,
    pub model_id: Option<String>,
    pub tool_mode: SessionToolMode,
    pub updated_at: OffsetDateTime,
}

impl SessionRecord {
    #[must_use]
    pub fn settings_snapshot(&self) -> SessionSettingsSnapshot {
        SessionSettingsSnapshot {
            session_id: self.session_id,
            settings_revision_id: self.settings_revision_id,
            connection_id: self.connection_id.clone(),
            model_id: self.model_id.clone(),
            tool_mode: self.tool_mode,
            updated_at: self.updated_at,
        }
    }

    #[must_use]
    pub fn with_settings_snapshot(&self, snapshot: &SessionSettingsSnapshot) -> Self {
        Self {
            connection_id: snapshot.connection_id.clone(),
            model_id: snapshot.model_id.clone(),
            tool_mode: snapshot.tool_mode,
            settings_revision_id: snapshot.settings_revision_id,
            updated_at: snapshot.updated_at,
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchRecord {
    pub branch_id: BranchId,
    pub session_id: SessionId,
    pub parent_branch_id: Option<BranchId>,
    pub parent_event_id: Option<EventId>,
    pub head_event_id: Option<EventId>,
    pub summary: Option<String>,
    pub created_at: OffsetDateTime,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchHead {
    pub branch_id: BranchId,
    pub session_id: SessionId,
    pub head_seq_id: i64,
    pub updated_at: OffsetDateTime,
}
