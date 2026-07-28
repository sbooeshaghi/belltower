use crate::{
    ApprovalDecision, ApprovalRequestSnapshot, ApprovalResolution, BranchId, BudgetConfig,
    CompactionId, CompletionDelta, ContextCompactionPhase, ContextCompactionStatus,
    ContextCompactionTrigger, ContextManifest, CostBreakdown, ErrorClass, EventId, Message,
    MessageId, PlanItem, RelatedSessionMessage, RelatedSessionMessageDirection,
    RelatedSessionMessageId, RelatedSessionMessageStatus, SessionId, SpanId, TokenUsage,
    ToolCallId, ToolOperationContext, ToolResultEnvelope, TurnId, TurnInstructionProvenance,
    default_settings_revision_id,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use time::OffsetDateTime;

pub mod oi_attrs {
    pub const INPUT_VALUE: &str = "input.value";
    pub const OUTPUT_VALUE: &str = "output.value";
    pub const SESSION_ID: &str = "session.id";
    pub const TURN_ID: &str = "turn.id";
    pub const TOOL_NAME: &str = "tool.name";
    pub const TOOL_PARAMETERS: &str = "tool.parameters";
    pub const LLM_MODEL_NAME: &str = "llm.model_name";
    pub const LLM_PROVIDER: &str = "llm.provider";
    pub const LLM_SYSTEM: &str = "llm.system";
    pub const LLM_TOKEN_PROMPT: &str = "llm.token_count.prompt";
    pub const LLM_TOKEN_COMPLETION: &str = "llm.token_count.completion";
    pub const LLM_TOKEN_TOTAL: &str = "llm.token_count.total";
    pub const LLM_TOKEN_PROMPT_DETAILS_CACHE_READ: &str =
        "llm.token_count.prompt_details.cache_read";
    pub const LLM_TOKEN_PROMPT_DETAILS_CACHE_WRITE: &str =
        "llm.token_count.prompt_details.cache_write";
    pub const LLM_TOKEN_COMPLETION_DETAILS_REASONING: &str =
        "llm.token_count.completion_details.reasoning";
    pub const LLM_COST_PROMPT: &str = "llm.cost.prompt";
    pub const LLM_COST_COMPLETION: &str = "llm.cost.completion";
    pub const LLM_COST_TOTAL: &str = "llm.cost.total";
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum SpanKind {
    Session,
    Agent,
    Llm,
    Tool,
    Chain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum QueuedMessageResolutionOutcome {
    Dispatched,
    Dropped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum SteerResolutionOutcome {
    Applied,
    Dropped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum TurnStartSource {
    UserMessage,
    ApprovalResume,
    InputResume,
    SteerFollowUp,
    QueuedFollowUp,
    RelatedSessionMessage,
}

fn default_turn_start_source() -> TurnStartSource {
    TurnStartSource::UserMessage
}

/// Externally tagged canonical event payload union: exactly one key naming
/// the variant (for example `{"SessionStarted": {...}}`). The serialized
/// event kind strings (`session.started`, ...) are derived labels, not the
/// serde tag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(deny_unknown_fields)]
pub enum EventPayload {
    SessionStarted {
        project_root: String,
        connection_id: String,
    },
    SessionSpawnRequested {
        child_session_id: SessionId,
        objective: String,
        connection_id: String,
        model_id: Option<String>,
    },
    SessionSpawned {
        child_session_id: SessionId,
        child_branch_id: BranchId,
        objective: String,
    },
    SessionHandoffRecorded {
        parent_session_id: SessionId,
        parent_branch_id: BranchId,
        parent_turn_id: Option<TurnId>,
        objective: String,
        summary: String,
    },
    SessionResultImported {
        child_session_id: SessionId,
        status: String,
        summary: String,
    },
    SessionResultRejected {
        child_session_id: SessionId,
        reason: String,
    },
    RelatedSessionMessageRecorded {
        direction: RelatedSessionMessageDirection,
        counterpart_event_id: EventId,
        message: RelatedSessionMessage,
    },
    RelatedSessionMessageResolved {
        message_id: RelatedSessionMessageId,
        status: RelatedSessionMessageStatus,
        resulting_turn_id: Option<TurnId>,
        reason: Option<String>,
    },
    RelatedSessionMessageSettled {
        message_id: RelatedSessionMessageId,
        reply_message_id: RelatedSessionMessageId,
        reply_kind: crate::RelatedSessionMessageKind,
        settling_turn_id: TurnId,
    },
    SessionSettingsUpdated {
        #[serde(default = "default_settings_revision_id")]
        settings_revision_id: u64,
        connection_id: String,
        model_id: Option<String>,
        tool_mode: String,
    },
    SessionEnded {
        reason: String,
    },
    BranchCreated {
        parent_branch_id: Option<BranchId>,
        parent_event_id: Option<EventId>,
    },
    BranchActivated {
        branch_id: BranchId,
    },
    BranchSummarized {
        branch_id: BranchId,
        summary: String,
        files_read: Vec<String>,
        files_modified: Vec<String>,
    },
    MessageAppended {
        message: Message,
    },
    TurnStarted {
        turn_id: TurnId,
        provider: String,
        model: String,
        message_count: u32,
        #[serde(default = "default_settings_revision_id")]
        settings_revision_id: u64,
        #[serde(default = "default_turn_start_source")]
        source: TurnStartSource,
        resumed_from_call_id: Option<ToolCallId>,
    },
    TurnInstructionProvenanceRecorded {
        provenance: TurnInstructionProvenance,
    },
    TurnContextManifestRecorded {
        manifest: ContextManifest,
    },
    CompletionRequested {
        llm_call_ordinal: u32,
        provider: String,
        model: String,
        message_count: u32,
    },
    /// Live/store shape. In `session.bt` bundle event records the
    /// store-local `raw_chunk_index` is replaced by a portable
    /// `raw_chunk_content_ref` string (`sha256:<hex>`).
    CompletionChunk {
        llm_call_ordinal: Option<u32>,
        deltas: Vec<CompletionDelta>,
        /// Store-local raw chunk row id; transport-only. Removed from
        /// canonical (hashed) bytes and replaced by
        /// `raw_chunk_content_ref` in bundles.
        raw_chunk_index: Option<i64>,
    },
    CompletionFinished {
        llm_call_ordinal: u32,
        provider: String,
        model: String,
        usage: TokenUsage,
        cost: Option<CostBreakdown>,
        finish_reason: String,
        latency_ms: u64,
    },
    SessionError {
        class: ErrorClass,
        code: String,
        message: String,
        retryable: bool,
    },
    TurnFinished {
        turn_id: TurnId,
        provider: String,
        model: String,
        status: String,
        finish_reason: Option<String>,
        latency_ms: u64,
    },
    ToolCallRequested {
        call_id: ToolCallId,
        tool_name: String,
        arguments: Value,
    },
    ToolOperationRecorded {
        call_id: ToolCallId,
        tool_name: String,
        #[serde(default)]
        operation: ToolOperationContext,
    },
    ToolApprovalRequested {
        call_id: ToolCallId,
        tool_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot: Option<ApprovalRequestSnapshot>,
    },
    ToolApprovalResolved {
        call_id: ToolCallId,
        tool_name: String,
        request_fingerprint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolution: Option<ApprovalResolution>,
        decision: ApprovalDecision,
    },
    ToolExecutionFinished {
        call_id: ToolCallId,
        tool_name: String,
        result: ToolResultEnvelope,
    },
    PlanUpdated {
        items: Vec<PlanItem>,
    },
    BudgetConfigured {
        budget: BudgetConfig,
    },
    ContextCompacted {
        #[serde(default)]
        #[schemars(skip_serializing_if = "crate::schema_support::omit_nondeterministic_default")]
        compaction_id: CompactionId,
        /// 1-based compaction window ordinal on this branch: how many
        /// compactions (including this one) have been recorded on the branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window_number: Option<u64>,
        /// `compaction_id` of the previous `context.compacted` event on this
        /// branch, forming the window chain.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_compaction_id: Option<CompactionId>,
        /// `compaction_id` of the first `context.compacted` event in this
        /// branch's window chain (self-referential for the first window).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_compaction_id: Option<CompactionId>,
        #[serde(default)]
        trigger: ContextCompactionTrigger,
        #[serde(default)]
        phase: ContextCompactionPhase,
        #[serde(default)]
        status: ContextCompactionStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Store-local sequence ref; transport-only, removed from canonical
        /// (hashed) bytes and from `session.bt` bundle event records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_boundary_seq_id: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary_message_id: Option<MessageId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_kept_message_id: Option<MessageId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_kept_branch_id: Option<BranchId>,
        /// Store-local sequence ref; transport-only, removed from canonical
        /// (hashed) bytes and from `session.bt` bundle event records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_kept_seq_id: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        latency_ms: Option<u64>,
        #[serde(default)]
        summary: String,
        messages_before: u32,
        messages_after: u32,
        tokens_before: u64,
        tokens_after: u64,
        files_read: Vec<String>,
        files_modified: Vec<String>,
    },
    BudgetCheckpoint {
        tokens_used: u64,
        turns_used: u32,
        elapsed_seconds: u64,
        cost_used_usd: Option<f64>,
    },
    SessionQueuedMessageEnqueued {
        message: Message,
        #[serde(default = "default_settings_revision_id")]
        settings_revision_id: u64,
    },
    SessionQueuedMessageResolved {
        queue_event_id: EventId,
        outcome: QueuedMessageResolutionOutcome,
        reason: Option<String>,
    },
    SessionCancelled {
        reason: String,
    },
    SessionCancelCleared {
        reason: String,
    },
    SessionSteered {
        message: String,
        #[serde(default = "default_settings_revision_id")]
        settings_revision_id: u64,
    },
    SessionSteersResolved {
        steer_event_ids: Vec<EventId>,
        outcome: SteerResolutionOutcome,
        combined_message: Option<String>,
        reason: Option<String>,
    },
    OperatorCommandRecorded {
        command_type: String,
        raw_input: String,
        output: String,
        success: bool,
    },
    /// Live/store shape. In `session.bt` bundle event records the
    /// store-local `chunk_index` is replaced by a portable
    /// `raw_chunk_content_ref` string (`sha256:<hex>`).
    RawChunkPersisted {
        provider: String,
        /// Store-local raw chunk row id; transport-only. Removed from
        /// canonical (hashed) bytes and replaced by
        /// `raw_chunk_content_ref` in bundles.
        chunk_index: i64,
        stream: String,
        llm_call_ordinal: Option<u32>,
    },
}

impl EventPayload {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted { .. } => "session.started",
            Self::SessionSpawnRequested { .. } => "session.spawn.requested",
            Self::SessionSpawned { .. } => "session.spawned",
            Self::SessionHandoffRecorded { .. } => "session.handoff.recorded",
            Self::SessionResultImported { .. } => "session.result.imported",
            Self::SessionResultRejected { .. } => "session.result.rejected",
            Self::RelatedSessionMessageRecorded { .. } => "session.related_message.recorded",
            Self::RelatedSessionMessageResolved { .. } => "session.related_message.resolved",
            Self::RelatedSessionMessageSettled { .. } => "session.related_message.settled",
            Self::SessionSettingsUpdated { .. } => "session.settings.updated",
            Self::SessionEnded { .. } => "session.ended",
            Self::BranchCreated { .. } => "branch.created",
            Self::BranchActivated { .. } => "branch.activated",
            Self::BranchSummarized { .. } => "branch.summarized",
            Self::MessageAppended { .. } => "message.appended",
            Self::TurnStarted { .. } => "turn.started",
            Self::TurnInstructionProvenanceRecorded { .. } => "turn.instructions.recorded",
            Self::TurnContextManifestRecorded { .. } => "turn.context_manifest.recorded",
            Self::CompletionRequested { .. } => "completion.requested",
            Self::CompletionChunk { .. } => "completion.chunk",
            Self::CompletionFinished { .. } => "completion.finished",
            Self::SessionError { .. } => "session.error",
            Self::TurnFinished { .. } => "turn.finished",
            Self::ToolCallRequested { .. } => "tool.call.requested",
            Self::ToolOperationRecorded { .. } => "tool.operation.recorded",
            Self::ToolApprovalRequested { .. } => "tool.approval.requested",
            Self::ToolApprovalResolved { .. } => "tool.approval.resolved",
            Self::ToolExecutionFinished { .. } => "tool.execution.finished",
            Self::PlanUpdated { .. } => "plan.updated",
            Self::BudgetConfigured { .. } => "budget.configured",
            Self::ContextCompacted { .. } => "context.compacted",
            Self::BudgetCheckpoint { .. } => "budget.checkpoint",
            Self::SessionQueuedMessageEnqueued { .. } => "session.queued_message.enqueued",
            Self::SessionQueuedMessageResolved { .. } => "session.queued_message.resolved",
            Self::SessionCancelled { .. } => "session.cancelled",
            Self::SessionCancelCleared { .. } => "session.cancel.cleared",
            Self::SessionSteered { .. } => "session.steered",
            Self::SessionSteersResolved { .. } => "session.steers.resolved",
            Self::OperatorCommandRecorded { .. } => "operator.command.recorded",
            Self::RawChunkPersisted { .. } => "raw_chunk.persisted",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EventEnvelope {
    /// Store sequence id; transport-only. Present (possibly `null`) on the
    /// live event API, absent from `session.bt` bundle event records, and
    /// excluded from the canonical bytes that `event_hash` covers.
    pub seq_id: Option<i64>,
    pub event_id: EventId,
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub turn_id: Option<TurnId>,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub span_kind: SpanKind,
    #[schemars(schema_with = "crate::schema_support::offset_date_time_schema")]
    pub occurred_at: OffsetDateTime,
    pub payload: EventPayload,
    pub attributes: BTreeMap<String, Value>,
}

impl EventEnvelope {
    #[must_use]
    pub fn new(
        session_id: SessionId,
        branch_id: BranchId,
        span_kind: SpanKind,
        payload: EventPayload,
    ) -> Self {
        Self {
            seq_id: None,
            event_id: EventId::new(),
            session_id,
            branch_id,
            turn_id: None,
            span_id: SpanId::new(),
            parent_span_id: None,
            span_kind,
            occurred_at: OffsetDateTime::now_utc(),
            payload,
            attributes: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_attribute(mut self, key: impl Into<String>, value: Value) -> Self {
        self.attributes.insert(key.into(), value);
        self
    }

    #[must_use]
    pub fn with_turn_id(mut self, turn_id: TurnId) -> Self {
        self.turn_id = Some(turn_id);
        self.attributes.insert(
            oi_attrs::TURN_ID.to_owned(),
            Value::String(turn_id.to_string()),
        );
        self
    }

    #[must_use]
    pub fn kind(&self) -> &'static str {
        self.payload.kind()
    }
}

#[cfg(test)]
mod tests {
    use super::{EventPayload, TurnStartSource};
    use crate::TurnId;
    use serde_json::json;

    #[test]
    fn turn_started_defaults_legacy_source_to_user_message() {
        let payload: EventPayload = serde_json::from_value(json!({
            "TurnStarted": {
                "turn_id": TurnId::new(),
                "provider": "openai-compatible",
                "model": "o4-mini",
                "message_count": 1,
                "settings_revision_id": 1,
                "resumed_from_call_id": null
            }
        }))
        .expect("legacy turn.started should deserialize");

        assert!(matches!(
            payload,
            EventPayload::TurnStarted {
                source: TurnStartSource::UserMessage,
                ..
            }
        ));
    }
}
