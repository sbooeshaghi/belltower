//! Public record and projection types produced by the SQLite session store.
//!
//! These shapes are consumed by runtime inspection, export, replay, and client
//! surfaces. Persistence methods live in the parent `store` module.

use bt_core::{
    ApprovalDecision, ApprovalRequestSnapshot, ApprovalResolution, BranchId, BudgetConfig,
    ContextManifest, EventId, Message, PlanItem, SessionId, SessionToolMode, ToolCallId, TurnId,
    TurnStartSource,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawChunkRecord {
    pub chunk_id: i64,
    pub session_id: SessionId,
    pub branch_id: Option<BranchId>,
    pub turn_id: Option<TurnId>,
    pub llm_call_ordinal: Option<u32>,
    pub event_id: Option<String>,
    pub provider: String,
    pub stream_name: String,
    pub content: Vec<u8>,
    pub received_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawChunkInsert {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub turn_id: Option<TurnId>,
    pub llm_call_ordinal: Option<u32>,
    pub event_id: Option<String>,
    pub provider: String,
    pub stream_name: String,
    pub content: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalProjection {
    pub call_id: ToolCallId,
    pub session_id: SessionId,
    pub tool_name: String,
    pub status: String,
    pub request_fingerprint: Option<String>,
    pub request_snapshot: Option<ApprovalRequestSnapshot>,
    pub decision: Option<ApprovalDecision>,
    pub resolution: Option<ApprovalResolution>,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReusableApprovalProjection {
    pub session_id: SessionId,
    pub request_fingerprint: String,
    pub decision: ApprovalDecision,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOperationRecoveryRecord {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub operation: bt_core::ToolOperationContext,
    pub requested_seq_id: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionControlProjection {
    pub session_id: SessionId,
    pub cancel_requested: bool,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueuedMessageProjection {
    pub queue_event_id: EventId,
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub message: Message,
    pub settings_revision_id: u64,
    pub status: String,
    pub source_seq: i64,
    pub enqueued_at: OffsetDateTime,
    pub resolved_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SteerProjection {
    pub steer_event_id: EventId,
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub message: String,
    pub settings_revision_id: u64,
    pub status: String,
    pub source_seq: i64,
    pub enqueued_at: OffsetDateTime,
    pub resolved_at: Option<OffsetDateTime>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolRunProjection {
    pub call_id: ToolCallId,
    pub session_id: SessionId,
    pub tool_name: String,
    pub status: String,
    pub arguments: Option<Value>,
    pub result: Option<Value>,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextManifestProjection {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub turn_id: TurnId,
    pub llm_call_ordinal: u32,
    pub provider: String,
    pub model: String,
    pub settings_revision_id: u64,
    pub context_boundary_seq_id: Option<i64>,
    pub message_count: u32,
    pub tool_count: u32,
    pub attachment_count: u32,
    pub compacted: bool,
    pub manifest: ContextManifest,
    pub recorded_event_id: EventId,
    pub recorded_seq_id: i64,
    pub recorded_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CostSummaryProjection {
    pub session_id: SessionId,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub unpriced_completion_count: u64,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionBudgetProjection {
    pub session_id: SessionId,
    pub budget: BudgetConfig,
    pub tokens_used: u64,
    pub turns_used: u32,
    pub elapsed_seconds: u64,
    pub cost_used_usd: Option<f64>,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanProjection {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub items: Vec<PlanItem>,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionSettingsRevisionProjection {
    pub session_id: SessionId,
    pub settings_revision_id: u64,
    pub connection_id: bt_core::ConnectionId,
    pub model_id: Option<String>,
    pub tool_mode: SessionToolMode,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnRecoveryRecord {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub turn_id: TurnId,
    pub provider: String,
    pub model: String,
    pub source: TurnStartSource,
    pub resumed_from_call_id: Option<bt_core::ToolCallId>,
    pub started_seq_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumedTurnRecoveryRecord {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub turn_id: TurnId,
    pub provider: String,
    pub model: String,
    pub started_seq_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInspectionMetrics {
    pub last_seq_id: Option<i64>,
    pub turn_count: u32,
    pub message_count: u32,
    pub tool_call_count: u32,
    pub approval_count: u32,
    pub pending_approval_count: u32,
    pub raw_chunk_count: u32,
    pub active_turn_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionTurnAdmission {
    Started { seq_ids: Vec<i64> },
    Queued { seq_ids: Vec<i64>, position: usize },
    RetryWithSettings { settings_revision_id: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContinuationClaim {
    Claimed { seq_ids: Vec<i64> },
    Busy,
    CancelPending,
    Stale,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SequencedMessageRecord {
    pub seq_id: i64,
    pub message: Message,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextMessageRecord {
    pub message: Message,
    pub source_branch_id: Option<BranchId>,
    pub source_seq_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordedOperatorCommandRecord {
    pub seq_id: i64,
    pub occurred_at: OffsetDateTime,
    pub command_type: String,
    pub raw_input: String,
    pub output: String,
    pub success: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptPage<T> {
    pub items: Vec<T>,
    pub oldest_seq_id: Option<i64>,
    pub newest_seq_id: Option<i64>,
    pub has_more_before: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChunkPage<T> {
    pub items: Vec<T>,
    pub oldest_chunk_id: Option<i64>,
    pub newest_chunk_id: Option<i64>,
    pub has_more_before: bool,
}
