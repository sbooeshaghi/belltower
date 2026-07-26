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
    /// 1-based compaction window ordinal on this branch.
    pub window_number: Option<u64>,
    /// `compaction_id` of the previous compaction on this branch.
    pub previous_compaction_id: Option<bt_core::CompactionId>,
    /// `compaction_id` of the first compaction in this branch's chain.
    pub first_compaction_id: Option<bt_core::CompactionId>,
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

#[derive(Debug, PartialEq, Eq)]
pub struct AdmittedTurn {
    pub(crate) session_id: SessionId,
    pub(crate) branch_id: bt_core::BranchId,
    pub(crate) turn_id: TurnId,
    pub(crate) settings_revision_id: u64,
}

impl AdmittedTurn {
    pub(crate) fn new(
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        settings_revision_id: u64,
    ) -> Self {
        Self {
            session_id,
            branch_id,
            turn_id,
            settings_revision_id,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub fn branch_id(&self) -> bt_core::BranchId {
        self.branch_id
    }

    #[must_use]
    pub fn turn_id(&self) -> TurnId {
        self.turn_id
    }

    #[must_use]
    pub fn settings_revision_id(&self) -> u64 {
        self.settings_revision_id
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum UserMessageAdmission {
    Started(AdmittedTurn),
    Queued { position: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedDispatch {
    pub branch_id: bt_core::BranchId,
    pub turn_id: TurnId,
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
    pub requested_seq_id: i64,
    pub settings_revision_id: u64,
    pub tool_call: ToolCall,
    pub approval_request_snapshot: Option<bt_core::ApprovalRequestSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostTurnControlAction {
    Stop,
    ContinueCurrentBranch(QueuedDispatch),
    ContinueQueuedBranch(QueuedDispatch),
    ContinueRelatedSessionBranch(QueuedDispatch),
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
    pub(crate) delta_cost_is_unknown: bool,
    pub(crate) finished_at: Option<time::OffsetDateTime>,
    pub(crate) already_checkpointed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResumableToolCallKind {
    Approval,
    Input,
}
