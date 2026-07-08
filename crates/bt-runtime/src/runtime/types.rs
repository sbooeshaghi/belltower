// types.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct BranchTranscriptPage<T> {
    pub items: Vec<T>,
    pub oldest_seq_id: Option<i64>,
    pub newest_seq_id: Option<i64>,
    pub has_more_before: bool,
    pub last_seq_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompactionReport {
    pub compaction_id: bt_core::CompactionId,
    pub trigger: bt_core::ContextCompactionTrigger,
    pub phase: bt_core::ContextCompactionPhase,
    pub status: bt_core::ContextCompactionStatus,
    pub reason: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub context_boundary_seq_id: Option<i64>,
    pub source_event_id: Option<bt_core::EventId>,
    pub summary_message_id: Option<bt_core::MessageId>,
    pub first_kept_message_id: Option<bt_core::MessageId>,
    pub first_kept_branch_id: Option<bt_core::BranchId>,
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

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedTurnContext {
    pub settings_revision_id: u64,
    pub connection: ConnectionDescriptor,
    pub model_id: String,
    pub context_boundary_seq_id: Option<i64>,
    pub message_sources: Vec<bt_core::ContextMessageSourceRef>,
    pub request: bt_core::CompletionRequest,
    pub compaction: Option<ContextCompactionReport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrappedTurn {
    pub turn_id: TurnId,
    pub settings_revision_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedDispatch {
    pub branch_id: bt_core::BranchId,
    pub settings_revision_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingToolCallContext {
    pub(crate) branch_id: bt_core::BranchId,
    pub(crate) turn_id: TurnId,
    pub(crate) settings_revision_id: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResumableToolCall {
    pub session: SessionRecord,
    pub branch: BranchRecord,
    pub turn_id: TurnId,
    pub settings_revision_id: u64,
    pub tool_call: ToolCall,
    pub approval_request_snapshot: Option<bt_core::ApprovalRequestSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostTurnControlAction {
    Stop,
    ContinueCurrentBranch(u64),
    ContinueQueuedBranch(QueuedDispatch),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetEnforcementOutcome {
    NotConfigured,
    WithinBudget,
    Exhausted,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TurnBudgetWindow {
    pub(crate) base_tokens_used: u64,
    pub(crate) base_turns_used: u32,
    pub(crate) base_elapsed_seconds: u64,
    pub(crate) base_cost_used_usd: Option<f64>,
    pub(crate) anchor_time: time::OffsetDateTime,
    pub(crate) delta_usage: TokenUsage,
    pub(crate) delta_cost_used_usd: Option<f64>,
    pub(crate) finished_at: Option<time::OffsetDateTime>,
    pub(crate) already_checkpointed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResumableToolCallKind {
    Approval,
    Input,
}
