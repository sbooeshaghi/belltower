use bt_core::{
    ApprovalDecision, ApprovalScope, BelltowerError, BranchId, BranchInspection, BranchRecord,
    BudgetConfig, CompactionId, ConnectionDescriptor, ConnectionId, ConnectionModelInventory,
    ContextCompactionPhase, ContextCompactionStatus, ContextCompactionTrigger, ErrorClass,
    EventEnvelope, EventId, McpServerDescriptor, McpToolDescriptor, Message, MessageId,
    ModelBackendDescriptor, ModelRecommendationsReport, QueuedMessageInspection,
    SessionExecutionInspection, SessionId, SessionInspection, SessionLineageInspection,
    SessionQueueInspection, SessionRecord, SessionSearchMatch, SessionToolCallInspection,
    SessionToolMode, SessionTreeInspection, SessionWorkflowInspection, StatusInspection,
    ToolCallId, TurnId, TurnInspection,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub class: ErrorClass,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Option<Value>,
}

impl ErrorEnvelope {
    #[must_use]
    pub fn from_core(error: &BelltowerError) -> Self {
        Self {
            class: error.class(),
            code: error.code().to_owned(),
            message: error.to_string(),
            retryable: error.retryable(),
            details: None,
        }
    }

    #[must_use]
    pub fn new(
        class: ErrorClass,
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            class,
            code: code.into(),
            message: message.into(),
            retryable,
            details: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    #[serde(rename = "legacy-bundle")]
    LegacyBundle,
    Jsonl,
    Html,
    ShareGpt,
    Otlp,
}

impl ExportFormat {
    pub fn parse(value: &str) -> bt_core::Result<Self> {
        match value {
            "legacy-bundle" => Ok(Self::LegacyBundle),
            "jsonl" => Ok(Self::Jsonl),
            "html" => Ok(Self::Html),
            "sharegpt" => Ok(Self::ShareGpt),
            "otlp" => Ok(Self::Otlp),
            other => Err(BelltowerError::InvalidState(format!(
                "unsupported export format `{other}`"
            ))),
        }
    }

    #[must_use]
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::LegacyBundle => "application/json",
            Self::Jsonl => "application/jsonl",
            Self::Html => "text/html; charset=utf-8",
            Self::ShareGpt => "application/json",
            Self::Otlp => "application/json",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub project_root: String,
    pub connection_id: ConnectionId,
    pub model_id: Option<String>,
    pub tool_mode: Option<SessionToolMode>,
    pub display_name: Option<String>,
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetConfig>,
    /// "auto" puts the session into auto-approval mode: every tool approval
    /// resolves as a recorded policy decision instead of prompting. Omit (or
    /// "prompt") for the default human-in-the-loop behavior. Process-local
    /// and fail-safe: a server restart falls back to prompting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session: SessionRecord,
    pub branch: BranchRecord,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpawnSessionRequest {
    pub parent_branch_id: BranchId,
    pub parent_turn_id: Option<TurnId>,
    pub objective: String,
    pub display_name: Option<String>,
    pub connection_id: Option<ConnectionId>,
    pub model_id: Option<String>,
    /// Whether to deliver the objective as a wake instruction so the child
    /// starts working immediately (default true, matching the model-facing
    /// spawn_agent tool). Set false to create a prepared-but-idle child that
    /// only runs once it is messaged directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpawnSessionResponse {
    pub parent_session_id: SessionId,
    pub parent_branch_id: BranchId,
    pub parent_turn_id: Option<TurnId>,
    pub child_session: SessionRecord,
    pub child_branch: BranchRecord,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInspectionResponse {
    pub inspection: SessionInspection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTurnsResponse {
    pub session_id: SessionId,
    pub turns: Vec<TurnInspection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionExecutionResponse {
    pub inspection: SessionExecutionInspection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionQueueResponse {
    pub inspection: SessionQueueInspection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionQueueClearResponse {
    pub session_id: SessionId,
    pub cleared_messages: Vec<QueuedMessageInspection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionToolCallResponse {
    pub inspection: SessionToolCallInspection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSearchResponse {
    pub session_id: SessionId,
    pub branch_id: Option<BranchId>,
    pub query: String,
    pub limit: usize,
    pub results: Vec<SessionSearchMatch>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionLineageResponse {
    pub inspection: SessionLineageInspection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionWorkflowResponse {
    pub inspection: SessionWorkflowInspection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBranchInspectionResponse {
    pub session_id: SessionId,
    pub active_branch_id: Option<BranchId>,
    pub branches: Vec<BranchInspection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTreeResponse {
    pub inspection: SessionTreeInspection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateSessionRequest {
    pub branch_id: BranchId,
    pub connection_id: Option<ConnectionId>,
    pub model_id: Option<String>,
    pub tool_mode: Option<SessionToolMode>,
    pub reset_model_to_default: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateSessionBudgetRequest {
    pub branch_id: BranchId,
    pub budget: BudgetConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub branch_id: BranchId,
    pub message: Message,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SendMessageOutcome {
    Dispatched,
    Queued { position: usize },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    #[serde(flatten)]
    pub outcome: SendMessageOutcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateBranchRequest {
    pub from_branch_id: BranchId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_event_id: Option<EventId>,
    pub activate: bool,
    pub carry_summary: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateBranchResponse {
    pub branch: BranchRecord,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactSessionRequest {
    pub branch_id: BranchId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextCompactionDto {
    pub compaction_id: CompactionId,
    pub trigger: ContextCompactionTrigger,
    pub phase: ContextCompactionPhase,
    pub status: ContextCompactionStatus,
    pub reason: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub context_boundary_seq_id: Option<i64>,
    pub summary_message_id: Option<MessageId>,
    pub first_kept_message_id: Option<MessageId>,
    pub first_kept_branch_id: Option<BranchId>,
    pub first_kept_seq_id: Option<i64>,
    pub latency_ms: Option<u64>,
    pub summary: String,
    pub messages_before: u32,
    pub messages_after: u32,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub files_read: Vec<String>,
    pub files_modified: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactSessionResponse {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub model_id: String,
    pub compaction: Option<ContextCompactionDto>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivateBranchRequest {
    pub carry_summary: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApproveToolRequest {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub scope: ApprovalScope,
    pub decision: ApprovalDecision,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerToolRequest {
    pub call_id: ToolCallId,
    pub response: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CancelSessionRequest {
    pub branch_id: BranchId,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SteerSessionRequest {
    pub branch_id: BranchId,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordOperatorCommandRequest {
    pub branch_id: BranchId,
    pub command_type: String,
    pub raw_input: String,
    pub output: String,
    pub success: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunShellCommandRequest {
    pub branch_id: BranchId,
    pub raw_input: String,
    pub command: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionEventsResponse {
    pub session_id: SessionId,
    pub events: Vec<EventEnvelope>,
    pub last_seq_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchOperatorCommandsResponse {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub events: Vec<EventEnvelope>,
    pub last_seq_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequencedMessageDto {
    pub seq_id: i64,
    pub message: Message,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedOperatorCommandDto {
    pub seq_id: i64,
    pub occurred_at: time::OffsetDateTime,
    pub command_type: String,
    pub raw_input: String,
    pub output: String,
    pub success: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchMessagesPageResponse {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub messages: Vec<SequencedMessageDto>,
    pub oldest_seq_id: Option<i64>,
    pub newest_seq_id: Option<i64>,
    pub has_more_before: bool,
    pub last_seq_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchOperatorCommandsPageResponse {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub commands: Vec<RecordedOperatorCommandDto>,
    pub oldest_seq_id: Option<i64>,
    pub newest_seq_id: Option<i64>,
    pub has_more_before: bool,
    pub last_seq_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionMessagesResponse {
    pub session_id: SessionId,
    pub branch_id: Option<BranchId>,
    pub messages: Vec<Message>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawChunkDto {
    pub chunk_id: i64,
    pub branch_id: Option<BranchId>,
    pub turn_id: Option<TurnId>,
    pub llm_call_ordinal: Option<u32>,
    pub event_id: Option<String>,
    pub provider: String,
    pub stream_name: String,
    pub content_base64: String,
    pub received_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionRawChunksResponse {
    pub session_id: SessionId,
    pub chunks: Vec<RawChunkDto>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnRawChunksPageResponse {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub turn_id: TurnId,
    pub chunks: Vec<RawChunkDto>,
    pub oldest_chunk_id: Option<i64>,
    pub newest_chunk_id: Option<i64>,
    pub has_more_before: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionExportResponse {
    pub session_id: SessionId,
    pub format: ExportFormat,
    pub content_type: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PushOtlpExportRequest {
    pub endpoint: Option<String>,
    pub project_name: Option<String>,
    pub api_key: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PushOtlpExportResponse {
    pub session_id: SessionId,
    pub request_url: String,
    pub content_type: String,
    pub bytes_sent: usize,
    pub status_code: u16,
    pub rejected_spans: Option<i64>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchesResponse {
    pub session_id: SessionId,
    pub branches: Vec<BranchRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectionsResponse {
    pub connections: Vec<ConnectionDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusInspectionResponse {
    pub inspection: StatusInspection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpServersResponse {
    pub servers: Vec<McpServerDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpToolsResponse {
    pub tools: Vec<McpToolDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpInventoryResponse {
    pub servers: Vec<McpServerDescriptor>,
    pub tools: Vec<McpToolDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelBackendsResponse {
    pub backends: Vec<ModelBackendDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRecommendationsResponse {
    pub report: ModelRecommendationsReport,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectionModelsResponse {
    pub connections: Vec<ConnectionModelInventory>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub protocol_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub approvals: bool,
    pub pending_input: bool,
    pub session_queue: bool,
    pub workflow_inspection: bool,
    pub lineage_inspection: bool,
    pub raw_chunk_paging: bool,
    pub exports: bool,
    pub mcp_inventory: bool,
    pub mcp_reload: bool,
    pub spawn_session: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfoResponse {
    pub server_version: String,
    pub protocol_version: String,
    pub supported_protocol_versions: Vec<String>,
    pub capabilities: ServerCapabilities,
}

#[cfg(test)]
mod tests {
    use super::ErrorEnvelope;
    use bt_core::ErrorClass;

    #[test]
    fn error_envelope_round_trips() {
        let envelope = ErrorEnvelope {
            class: ErrorClass::Storage,
            code: "storage_error".to_owned(),
            message: "boom".to_owned(),
            retryable: false,
            details: None,
        };
        let raw = serde_json::to_string(&envelope).expect("serialize");
        let parsed: ErrorEnvelope = serde_json::from_str(&raw).expect("deserialize");
        assert_eq!(parsed, envelope);
    }
}
