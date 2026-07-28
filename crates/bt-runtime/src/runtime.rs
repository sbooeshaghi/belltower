use crate::{
    ApprovalState, ConnectionRegistry, MarkdownInstructionResolver, PolicyApprovalEvaluator,
    approval_request_fingerprint,
};
use bt_context::ContextAssembler;
use bt_core::{
    ApprovalDecision, ApprovalEvaluator, ApprovalRequest, BelltowerConfig, BranchId,
    BranchInspection, BranchRecord, BudgetConfig, CompletionDelta, CompletionSummary,
    ConnectionDescriptor, ConnectionId, CostBreakdown, EventEnvelope, EventPayload,
    MAX_RELATED_SESSION_DEPTH, MAX_RELATED_SESSION_DESCENDANTS, McpServerDescriptor,
    McpToolDescriptor, Message, MessagePart, ModelBackendDescriptor, ModelRecommendationsReport,
    PendingApprovalInspection, PendingInputInspection, PlanInspection, PlanItem, PricingEntry,
    QueuedMessageInspection, QueuedMessageResolutionOutcome, RelatedSessionDeliveryMode,
    RelatedSessionMessage, RelatedSessionMessageDirection, RelatedSessionMessageId,
    RelatedSessionMessageKind, RelatedSessionMessageReceipt, RelatedSessionMessageRecord,
    RelatedSessionMessageStatus, RelatedSessionSummary, Result, Role, SessionBudgetInspection,
    SessionCostSummary, SessionErrorInspection, SessionExecutionInspection, SessionId,
    SessionInspection, SessionLineageInspection, SessionLineageNode, SessionQueueInspection,
    SessionRecord, SessionRelationKind, SessionRuntimeState, SessionSettingsSnapshot,
    SessionStatus, SessionToolCallInspection, SessionToolMode, SessionTreeInspection,
    SessionWorkflowInspection, SpanKind, StartupTrace, SteerResolutionOutcome, ThinkingConfig,
    TokenUsage, ToolCall, ToolCallId, ToolResultEnvelope, ToolSpec, TraceEventCounts,
    TraceTurnInspection, TurnId, TurnInspection, TurnStartSource, TurnToolCallSummary,
    WorkflowRuntimeCounts, WorkflowSessionNode, WorkflowStatusCounts, default_settings_revision_id,
    render_queue_message_input,
};
use bt_mcp::{McpRegisteredTool, McpRegistry};
use bt_models::LocalModelManager;
use bt_session::{
    ChunkPage, ContextMessageRecord, ContinuationClaim, LegacySessionExportBundle,
    LegacySessionExporter, QueuedMessageProjection, RawChunkRecord, RecordedOperatorCommandRecord,
    SequencedMessageRecord, SessionBudgetProjection, SessionControlProjection,
    SessionTurnAdmission, SqliteSessionStore, SteerProjection, TranscriptPage,
};
use camino::Utf8PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

mod budget;
mod context;
mod control_plane;
mod inspection;
mod lifecycle;
mod lineage;
mod queries;
mod records;
mod related_sessions;
mod services;
mod sessions;
mod support;
#[cfg(test)]
#[path = "tests/runtime.rs"]
mod tests;
mod types;

pub(crate) use context::{
    ObservedContextTokensInput, TurnContextCompactionInputs, TurnContextPlan,
    TurnContextSummarizationPlan,
};
pub use types::{
    AdmittedTurn, BranchTranscriptPage, BudgetEnforcementOutcome, ContextCompactionReport,
    PostTurnControlAction, PreparedTurnContext, QueuedDispatch, ResumableToolCall,
    UserMessageAdmission,
};
pub(super) use types::{PendingToolCallContext, ResumableToolCallKind, TurnBudgetWindow};

pub struct BelltowerRuntime {
    config: BelltowerConfig,
    store: Mutex<SqliteSessionStore>,
    #[cfg(any(test, feature = "test-support"))]
    store_append_fault: Mutex<Option<(usize, bt_core::BelltowerError)>>,
    approvals: Arc<ApprovalState>,
    approval_evaluator: Arc<PolicyApprovalEvaluator>,
    connections: ConnectionRegistry,
    mcp: McpRegistry,
    models: LocalModelManager,
    instructions: MarkdownInstructionResolver,
    event_bus: broadcast::Sender<EventEnvelope>,
}
