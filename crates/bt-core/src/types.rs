use crate::{
    BranchId, BranchRecord, ConnectionId, ErrorClass, EventId, Role, SessionId, SessionRecord,
    SessionStatus, SpanKind, ToolCallId, TurnId, TurnStartSource, default_settings_revision_id,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use url::Url;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub prompt_usd: f64,
    pub completion_usd: f64,
    pub total_usd: f64,
    pub cache_read_usd: Option<f64>,
    pub cache_write_usd: Option<f64>,
    pub reasoning_usd: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    pub provider: String,
    pub model_pattern: String,
    pub prompt_usd_per_million: f64,
    pub completion_usd_per_million: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_usd_per_million: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_usd_per_million: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_usd_per_million: Option<f64>,
    /// When present, the model has no reliable public price. Numeric
    /// rates are placeholders and consumers must treat usage as
    /// unpriced, not free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpriced_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThinkingConfig {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ThinkingEffort>,
    /// This caps model-side reasoning effort inside one completion request.
    /// It is distinct from session autonomy budgets such as wall-clock,
    /// turn-count, or total-cost limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
    pub include_summaries: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

/// Session-level autonomy limits. These are runtime-owned controls, not
/// provider thinking-budget hints.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_clock_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructuredOutputSpec {
    pub schema_name: Option<String>,
    pub schema: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskClass {
    Safe,
    Moderate,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolInterruptBehavior {
    Immediate,
    WaitForCompletion,
    TerminateProcess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionMode {
    Immediate,
    UserInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDisplayGroup {
    Codebase,
    Execution,
    Web,
    Planning,
    Interaction,
    Inspection,
    Workflow,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMetadata {
    pub risk_class: ToolRiskClass,
    pub is_read_only: bool,
    pub is_concurrency_safe: bool,
    pub interrupt_behavior: ToolInterruptBehavior,
    pub execution_mode: ToolExecutionMode,
    pub should_defer: bool,
    pub catalogue_tags: Vec<String>,
    pub display_group: ToolDisplayGroup,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    pub id: String,
    pub content: String,
    pub status: PlanStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters_schema: Value,
    pub metadata: ToolMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebSearchResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebSearchResponse {
    pub backend: String,
    pub query: String,
    pub result_count: usize,
    pub results: Vec<WebSearchResult>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebFetchFormat {
    Text,
    Markdown,
    Raw,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebFetchRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<WebFetchFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebFetchResponse {
    pub backend: String,
    pub requested_url: String,
    pub final_url: String,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub format: WebFetchFormat,
    pub body: String,
    pub bytes: u64,
    pub truncated: bool,
    pub content_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSystemPromptRef {
    pub present: bool,
    pub char_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMessageRef {
    pub message_id: crate::MessageId,
    pub role: crate::Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_branch_id: Option<BranchId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_seq_id: Option<i64>,
    pub part_count: u32,
    pub visible_text_chars: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMessageSourceRef {
    pub message_id: crate::MessageId,
    pub branch_id: BranchId,
    pub seq_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextToolRef {
    pub name: String,
    pub risk_class: ToolRiskClass,
    pub is_read_only: bool,
    pub execution_mode: ToolExecutionMode,
    pub should_defer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextAttachmentType {
    Generated,
    Compaction,
    Memory,
    Signal,
    Resource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextAttachmentProducer {
    Runtime,
    Operator,
    Tool,
    MemoryProvider,
    ExternalServer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextAttachmentVisibility {
    #[serde(default)]
    pub model_visible: bool,
    #[serde(default)]
    pub operator_visible: bool,
    #[serde(default)]
    pub exportable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPromptPlacement {
    #[default]
    NotPlaced,
    SystemPrompt,
    MessageHistory,
    ToolContext,
    AttachmentBlock,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextTruncationStatus {
    #[default]
    None,
    Truncated,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextAttachment {
    pub id: String,
    pub attachment_type: ContextAttachmentType,
    pub producer: ContextAttachmentProducer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<EventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_tool_call_id: Option<ToolCallId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default)]
    pub visibility: ContextAttachmentVisibility,
    #[serde(default)]
    pub placement: ContextPromptPlacement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_estimate: Option<u64>,
    #[serde(default)]
    pub truncation: ContextTruncationStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionTrigger {
    Forced,
    #[default]
    TokenBudget,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionPhase {
    Manual,
    #[default]
    PreTurn,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCompactionStatus {
    #[default]
    Completed,
    Failed,
}

/// Durable manifest of the context that was made visible to one provider call.
///
/// The manifest intentionally stores references and counts, not full message or
/// prompt bodies. Full message content and instruction provenance already live
/// in canonical session events; this shape makes the model-visible boundary
/// replayable without duplicating large payloads in every completion event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextManifest {
    pub turn_id: TurnId,
    pub branch_id: BranchId,
    pub llm_call_ordinal: u32,
    pub provider: String,
    pub model: String,
    #[serde(default = "default_settings_revision_id")]
    pub settings_revision_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_boundary_seq_id: Option<i64>,
    pub system_prompt: ContextSystemPromptRef,
    pub messages: Vec<ContextMessageRef>,
    pub tools: Vec<ContextToolRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ContextAttachment>,
    pub max_tokens: Option<u64>,
    pub thinking: Option<ThinkingConfig>,
    pub compacted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_event_id: Option<EventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_tool_call_id: Option<ToolCallId>,
    #[serde(default)]
    pub model_visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOperationInitiator {
    #[default]
    Agent,
    Human,
    Runtime,
    Mcp,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOperationContext {
    #[serde(default)]
    pub initiator: ToolOperationInitiator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_class: Option<ToolRiskClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_read_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<ToolExecutionMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ArtifactRef>,
}

impl ToolOperationContext {
    #[must_use]
    pub fn from_tool_spec(initiator: ToolOperationInitiator, tool: &ToolSpec) -> Self {
        Self {
            initiator,
            risk_class: Some(tool.metadata.risk_class),
            is_read_only: Some(tool.metadata.is_read_only),
            execution_mode: Some(tool.metadata.execution_mode),
            artifact_refs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResultEnvelope {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub is_error: bool,
    pub output: Value,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub session_id: SessionId,
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub arguments: Value,
    pub requirement: ApprovalRequirement,
    pub tool_metadata: ToolMetadata,
    pub requested_at: OffsetDateTime,
}

impl ApprovalRequest {
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let normalized_arguments = normalize_approval_arguments(&self.arguments);
        format!(
            "{}:{}",
            self.tool_name,
            stable_hash(&canonical_json(&normalized_arguments))
        )
    }

    #[must_use]
    pub fn snapshot(
        &self,
        initiator: ToolOperationInitiator,
        surface: impl Into<String>,
        registry_schema_hash: Option<String>,
        policy_rule: Option<String>,
    ) -> ApprovalRequestSnapshot {
        let normalized_arguments = normalize_approval_arguments(&self.arguments);
        let canonical_arguments = canonical_json(&normalized_arguments);
        ApprovalRequestSnapshot {
            session_id: self.session_id,
            call_id: self.call_id.clone(),
            tool_name: self.tool_name.clone(),
            request_fingerprint: self.fingerprint(),
            arguments_hash: stable_hash(&canonical_arguments),
            redacted_arguments_preview: Some(redact_argument_preview(&normalized_arguments)),
            requirement: self.requirement.clone(),
            tool_metadata: self.tool_metadata.clone(),
            initiator,
            surface: surface.into(),
            registry_schema_hash,
            policy_rule,
            requested_at: self.requested_at,
        }
    }

    #[must_use]
    pub fn from_snapshot_with_arguments(
        snapshot: &ApprovalRequestSnapshot,
        arguments: Value,
    ) -> Self {
        Self {
            session_id: snapshot.session_id,
            call_id: snapshot.call_id.clone(),
            tool_name: snapshot.tool_name.clone(),
            arguments,
            requirement: snapshot.requirement.clone(),
            tool_metadata: snapshot.tool_metadata.clone(),
            requested_at: snapshot.requested_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequestSnapshot {
    pub session_id: SessionId,
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub request_fingerprint: String,
    pub arguments_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_arguments_preview: Option<Value>,
    pub requirement: ApprovalRequirement,
    pub tool_metadata: ToolMetadata,
    #[serde(default)]
    pub initiator: ToolOperationInitiator,
    pub surface: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_schema_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_rule: Option<String>,
    pub requested_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalResolution {
    pub request_fingerprint: String,
    pub decision: ApprovalDecision,
}

impl ApprovalResolution {
    #[must_use]
    pub fn from_request(request: &ApprovalRequest, decision: ApprovalDecision) -> Self {
        Self {
            request_fingerprint: request.fingerprint(),
            decision,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalScope {
    Once,
    Session,
    Always,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalDecisionSource {
    Human,
    Policy { rule: String },
    Runtime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    Approved {
        decided_at: OffsetDateTime,
        decided_by: String,
        scope: ApprovalScope,
        source: ApprovalDecisionSource,
    },
    Denied {
        decided_at: OffsetDateTime,
        decided_by: String,
        reason: Option<String>,
        scope: ApprovalScope,
        source: ApprovalDecisionSource,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ApprovalRequirement {
    Always,
    FirstUsePerSession,
    Never,
    Conditional { description: String },
}

fn normalize_approval_arguments(arguments: &Value) -> Value {
    match arguments {
        Value::Object(map) => {
            let filtered = map
                .iter()
                .filter(|(key, _)| key.as_str() != "call_id")
                .map(|(key, value)| (key.clone(), normalize_approval_arguments(value)))
                .collect();
            Value::Object(filtered)
        }
        Value::Array(values) => {
            Value::Array(values.iter().map(normalize_approval_arguments).collect())
        }
        other => other.clone(),
    }
}

fn redact_argument_preview(arguments: &Value) -> Value {
    match arguments {
        Value::Object(map) => {
            let redacted = map
                .iter()
                .map(|(key, value)| {
                    let lowered = key.to_ascii_lowercase();
                    let value = if lowered.contains("secret")
                        || lowered.contains("token")
                        || lowered.contains("password")
                        || lowered.contains("authorization")
                        || lowered.ends_with("_key")
                    {
                        Value::String("[redacted]".to_owned())
                    } else {
                        redact_argument_preview(value)
                    };
                    (key.clone(), value)
                })
                .collect();
            Value::Object(redacted)
        }
        Value::Array(values) => Value::Array(values.iter().map(redact_argument_preview).collect()),
        Value::String(value) if value.chars().count() > 240 => {
            let mut preview = value.chars().take(240).collect::<String>();
            preview.push_str("...");
            Value::String(preview)
        }
        other => other.clone(),
    }
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned()),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_owned()),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn stable_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstructionDocument {
    pub source: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnInstructionProvenance {
    pub turn_id: TurnId,
    pub provider: String,
    pub model: String,
    pub settings_revision_id: u64,
    pub core_prompt: InstructionDocument,
    pub provider_overlay: Option<InstructionDocument>,
    pub instructions: Vec<InstructionDocument>,
    pub rendered_system_prompt: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    Unknown,
    Healthy,
    Degraded { reason: String },
    Unreachable { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub provider: String,
    pub kind: CredentialKind,
    pub secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_secret: Option<String>,
    #[serde(default, skip_serializing_if = "CredentialMetadata::is_empty")]
    pub metadata: CredentialMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialKind {
    ApiKey,
    OAuthToken,
    JsonDocument,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

impl CredentialMetadata {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.account_id.is_none() && self.plan_type.is_none() && self.workspace_id.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMethodKind {
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "oauth_browser", alias = "o_auth_browser")]
    OAuthBrowser,
    #[serde(rename = "oauth_device_code", alias = "o_auth_device_code")]
    OAuthDeviceCode,
    #[serde(rename = "json_document")]
    JsonDocument,
    #[serde(rename = "external_broker")]
    ExternalBroker,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionAuthMethodDescriptor {
    pub id: String,
    pub kind: AuthMethodKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<String>,
    #[serde(default)]
    pub supports_refresh: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeCredential {
    ApiKey {
        secret: String,
    },
    BearerToken {
        access_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        #[serde(default, skip_serializing_if = "CredentialMetadata::is_empty")]
        metadata: CredentialMetadata,
    },
    JsonDocument {
        document: String,
    },
}

impl RuntimeCredential {
    #[must_use]
    pub const fn credential_kind(&self) -> CredentialKind {
        match self {
            Self::ApiKey { .. } => CredentialKind::ApiKey,
            Self::BearerToken { .. } => CredentialKind::OAuthToken,
            Self::JsonDocument { .. } => CredentialKind::JsonDocument,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DetectionResult {
    pub provider: String,
    pub found: bool,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub connection_id: ConnectionId,
    pub model: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<crate::Message>,
    pub tools: Vec<ToolSpec>,
    pub structured_output: Option<StructuredOutputSpec>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f32>,
    pub thinking: Option<ThinkingConfig>,
}

impl ContextManifest {
    #[must_use]
    pub fn from_completion_request(
        turn_id: TurnId,
        branch_id: BranchId,
        llm_call_ordinal: u32,
        provider: impl Into<String>,
        model: impl Into<String>,
        settings_revision_id: u64,
        context_boundary_seq_id: Option<i64>,
        request: &CompletionRequest,
        compacted: bool,
        message_sources: &[ContextMessageSourceRef],
    ) -> Self {
        Self {
            turn_id,
            branch_id,
            llm_call_ordinal,
            provider: provider.into(),
            model: model.into(),
            settings_revision_id,
            context_boundary_seq_id,
            system_prompt: ContextSystemPromptRef {
                present: request.system_prompt.is_some(),
                char_count: request
                    .system_prompt
                    .as_ref()
                    .map_or(0, |prompt| prompt.chars().count() as u64),
            },
            messages: request
                .messages
                .iter()
                .map(|message| ContextMessageRef {
                    source_branch_id: message_sources
                        .iter()
                        .find(|source| source.message_id == message.message_id)
                        .map(|source| source.branch_id),
                    source_seq_id: message_sources
                        .iter()
                        .find(|source| source.message_id == message.message_id)
                        .map(|source| source.seq_id),
                    message_id: message.message_id,
                    role: message.role.clone(),
                    part_count: message.parts.len() as u32,
                    visible_text_chars: message
                        .text_parts()
                        .map(|text| text.chars().count() as u64)
                        .sum(),
                })
                .collect(),
            tools: request
                .tools
                .iter()
                .map(|tool| ContextToolRef {
                    name: tool.name.clone(),
                    risk_class: tool.metadata.risk_class,
                    is_read_only: tool.metadata.is_read_only,
                    execution_mode: tool.metadata.execution_mode,
                    should_defer: tool.metadata.should_defer,
                })
                .collect(),
            max_tokens: request.max_tokens,
            thinking: request.thinking.clone(),
            attachments: Vec::new(),
            compacted,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CompletionDelta {
    AppendText {
        text: String,
    },
    AppendReasoning {
        text: Option<String>,
        redacted: bool,
        opaque_replay: Option<Value>,
    },
    OpenToolCall {
        call_id: String,
        tool_name: String,
        arguments: Option<Value>,
    },
    AppendToolCallArguments {
        call_id: String,
        partial_json: String,
    },
    CloseToolCall {
        call_id: String,
    },
    AppendRefusal {
        text: Option<String>,
        provider_reason: Option<String>,
        opaque_metadata: Option<Value>,
    },
    SetStructuredOutput {
        schema_name: Option<String>,
        value: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionChunk {
    pub llm_call_ordinal: Option<u32>,
    pub deltas: Vec<CompletionDelta>,
    pub usage: Option<TokenUsage>,
    pub raw: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FinishReason {
    Stop,
    ToolUse,
    MaxTokens,
    Cancelled,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionSummary {
    pub provider: String,
    pub model: String,
    pub finish_reason: FinishReason,
    pub usage: TokenUsage,
    pub cost: Option<CostBreakdown>,
    pub latency_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectionDescriptor {
    pub id: ConnectionId,
    pub provider: String,
    pub base_url: Url,
    pub default_model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_methods: Vec<ConnectionAuthMethodDescriptor>,
    #[serde(default)]
    pub auth_sources: Vec<String>,
    #[serde(default)]
    pub model_fallbacks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discoverable_model_selectors: Vec<ConnectionModelSelector>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionModelSelector {
    Exact { value: String },
    Prefix { value: String },
}

impl ConnectionModelSelector {
    #[must_use]
    pub fn matches(&self, model: &str) -> bool {
        let model = model.to_ascii_lowercase();
        match self {
            Self::Exact { value } => model == value.to_ascii_lowercase(),
            Self::Prefix { value } => model.starts_with(&value.to_ascii_lowercase()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpTransportKind {
    Stdio,
    StreamableHttp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpServerStatus {
    Configured,
    Discovered,
    Ready,
    Degraded { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerDescriptor {
    pub name: String,
    pub transport: McpTransportKind,
    pub enabled: bool,
    pub status: McpServerStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    pub server_name: String,
    pub tool_name: String,
    pub qualified_name: String,
    pub description: String,
    pub parameters_schema: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelBackendKind {
    Ollama,
    LmStudio,
    LlamaCpp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelBackendStatus {
    Ready,
    Unreachable { reason: String },
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelBackendDescriptor {
    pub kind: ModelBackendKind,
    pub label: String,
    pub base_url: Url,
    pub status: ModelBackendStatus,
    pub available_models: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionModelSource {
    Default,
    Fallback,
    Discovered,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionModelOption {
    pub model_id: String,
    pub source: ConnectionModelSource,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectionModelInventory {
    pub connection_id: ConnectionId,
    pub provider: String,
    pub default_model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_methods: Vec<ConnectionAuthMethodDescriptor>,
    pub auth_state: ConnectionAuthState,
    pub auth_source: Option<String>,
    pub support_state: ConnectionSupportState,
    #[serde(default)]
    pub readiness_state: ConnectionReadinessState,
    pub probe_ready: bool,
    pub probe_status: String,
    pub discovered_source: Option<String>,
    pub models: Vec<ConnectionModelOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub total_memory_gb: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRecommendation {
    pub label: String,
    pub max_memory_gb: u64,
    pub models: Vec<String>,
    pub fits_hardware: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRecommendationsReport {
    pub hardware: HardwareProfile,
    pub recommendations: Vec<ModelRecommendation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionSupportState {
    RuntimeSupported,
    Planned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionAuthState {
    NotRequired,
    Missing,
    Configured,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionReadinessState {
    #[default]
    UnknownState,
    Ready,
    RuntimeUnsupported,
    MissingAuth,
    UnsupportedAuthMethod,
    Degraded,
    Unreachable,
    ValidationFailed,
    NoLocalModelsDetected,
    ConfiguredModelUnavailable,
}

impl ConnectionReadinessState {
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    #[must_use]
    pub const fn status_label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::RuntimeUnsupported => "runtime unsupported",
            Self::MissingAuth => "auth missing",
            Self::UnsupportedAuthMethod => "auth method unsupported",
            Self::Degraded => "degraded",
            Self::Unreachable => "unreachable",
            Self::ValidationFailed => "validation failed",
            Self::UnknownState => "unknown",
            Self::NoLocalModelsDetected => "no local models detected",
            Self::ConfiguredModelUnavailable => "configured model unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionReadinessInspection {
    pub connection_id: ConnectionId,
    pub provider: String,
    pub default_model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_methods: Vec<ConnectionAuthMethodDescriptor>,
    pub auth_state: ConnectionAuthState,
    pub auth_kind: Option<CredentialKind>,
    pub auth_source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_tried_sources: Vec<String>,
    pub support_state: ConnectionSupportState,
    #[serde(default)]
    pub readiness_state: ConnectionReadinessState,
    pub probe_ready: bool,
    pub probe_status: String,
}

impl ConnectionModelInventory {
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.readiness_state.is_ready()
    }

    #[must_use]
    pub const fn readiness_label(&self) -> &'static str {
        self.readiness_state.status_label()
    }

    #[must_use]
    pub fn discoverable_models(&self) -> Vec<&str> {
        if self.discovered_source.is_none() {
            return Vec::new();
        }

        self.models
            .iter()
            .filter(|model| {
                matches!(
                    model.source,
                    ConnectionModelSource::Default | ConnectionModelSource::Discovered
                )
            })
            .map(|model| model.model_id.as_str())
            .collect()
    }
}

impl ConnectionReadinessInspection {
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.readiness_state.is_ready()
    }

    #[must_use]
    pub const fn readiness_label(&self) -> &'static str {
        self.readiness_state.status_label()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInspection {
    pub auth_storage: String,
    pub default_connection: ConnectionId,
    pub connections: Vec<ConnectionReadinessInspection>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebBackendReadinessState {
    #[default]
    UnknownState,
    Ready,
    Disabled,
    MissingAuth,
    Unsupported,
    Degraded,
}

impl WebBackendReadinessState {
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    #[must_use]
    pub const fn status_label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Disabled => "disabled",
            Self::MissingAuth => "auth missing",
            Self::Unsupported => "unsupported",
            Self::Degraded => "degraded",
            Self::UnknownState => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebBackendReadinessInspection {
    pub backend_id: String,
    pub provider: String,
    pub enabled: bool,
    pub auth_state: ConnectionAuthState,
    pub auth_kind: Option<CredentialKind>,
    pub auth_source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_tried_sources: Vec<String>,
    #[serde(default)]
    pub readiness_state: WebBackendReadinessState,
    pub probe_ready: bool,
    pub probe_status: String,
}

impl WebBackendReadinessInspection {
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.readiness_state.is_ready()
    }

    #[must_use]
    pub const fn readiness_label(&self) -> &'static str {
        self.readiness_state.status_label()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebRetrievalInspection {
    pub search_backend: Option<String>,
    pub fetch_backend: String,
    pub backends: Vec<WebBackendReadinessInspection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionRuntimeState {
    Idle,
    Working,
    WaitingOnInput,
    WaitingOnApproval,
    CancelRequested,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionToolMode {
    Standard,
    #[default]
    Extended,
}

impl SessionToolMode {
    #[must_use]
    pub const fn is_extended(self) -> bool {
        matches!(self, Self::Extended)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionRelationKind {
    Parent,
    Child,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelatedSessionSummary {
    pub relation: SessionRelationKind,
    pub session_id: SessionId,
    pub display_name: Option<String>,
    pub objective: Option<String>,
    pub status: SessionStatus,
    pub connection_id: ConnectionId,
    pub model_id: Option<String>,
    pub origin_branch_id: Option<BranchId>,
    pub origin_turn_id: Option<TurnId>,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionCostSummary {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub unpriced_completion_count: u64,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionBudgetInspection {
    pub budget: BudgetConfig,
    pub tokens_used: u64,
    pub turns_used: u32,
    pub elapsed_seconds: u64,
    pub cost_used_usd: Option<f64>,
    pub exhausted: bool,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInspection {
    pub session: SessionRecord,
    pub active_branch: Option<BranchRecord>,
    pub branches: Vec<BranchRecord>,
    pub runtime_state: SessionRuntimeState,
    pub turn_count: u32,
    pub message_count: u32,
    pub tool_call_count: u32,
    pub approval_count: u32,
    pub pending_approval_count: u32,
    pub pending_input_count: u32,
    pub raw_chunk_count: u32,
    pub last_seq_id: Option<i64>,
    pub cancel_requested: bool,
    pub pending_steer_count: u32,
    pub related_sessions: Vec<RelatedSessionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_summary: Option<SessionCostSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<SessionBudgetInspection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuedMessageInspection {
    pub branch_id: BranchId,
    pub enqueued_at: OffsetDateTime,
    pub message: crate::Message,
    #[serde(default = "default_settings_revision_id")]
    pub settings_revision_id: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingApprovalInspection {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub branch_id: BranchId,
    pub turn_id: TurnId,
    #[serde(default = "default_settings_revision_id")]
    pub settings_revision_id: u64,
    pub requested_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_snapshot: Option<ApprovalRequestSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingInputInspection {
    pub call_id: ToolCallId,
    pub prompt: String,
    pub choices: Vec<String>,
    pub branch_id: BranchId,
    pub turn_id: TurnId,
    #[serde(default = "default_settings_revision_id")]
    pub settings_revision_id: u64,
    pub requested_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionQueueInspection {
    pub session_id: SessionId,
    pub runtime_state: SessionRuntimeState,
    pub cancel_requested: bool,
    pub pending_steer_count: u32,
    pub queued_messages: Vec<QueuedMessageInspection>,
    pub pending_approvals: Vec<PendingApprovalInspection>,
    pub pending_inputs: Vec<PendingInputInspection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionErrorInspection {
    pub seq_id: Option<i64>,
    pub event_id: EventId,
    pub branch_id: BranchId,
    pub turn_id: Option<TurnId>,
    pub span_kind: SpanKind,
    pub occurred_at: OffsetDateTime,
    pub class: ErrorClass,
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionToolCallInspection {
    pub session_id: SessionId,
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub branch_id: Option<BranchId>,
    pub turn_id: Option<TurnId>,
    pub requested_seq_id: Option<i64>,
    pub completed_seq_id: Option<i64>,
    pub requested_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
    pub approval_status: Option<String>,
    pub approval_decision: Option<ApprovalDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_request_snapshot: Option<ApprovalRequestSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_resolution: Option<ApprovalResolution>,
    pub approval_updated_at: Option<OffsetDateTime>,
    pub execution_status: Option<String>,
    pub arguments: Option<Value>,
    pub result: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSearchMatchKind {
    Message,
    OperatorCommand,
    ToolCall,
    ToolResult,
    SessionError,
    Plan,
    Context,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSearchMatch {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub turn_id: Option<TurnId>,
    pub seq_id: i64,
    pub event_id: EventId,
    pub occurred_at: OffsetDateTime,
    pub kind: SessionSearchMatchKind,
    pub matched_field: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<ToolCallId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanInspection {
    pub session_id: SessionId,
    pub branch_id: BranchId,
    pub items: Vec<PlanItem>,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionLineageNode {
    pub session: SessionRecord,
    pub depth: u32,
    pub is_focus: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionLineageInspection {
    pub focus_session_id: SessionId,
    pub root_session_id: SessionId,
    pub nodes: Vec<SessionLineageNode>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRuntimeCounts {
    pub idle: u32,
    pub working: u32,
    pub waiting_on_input: u32,
    pub waiting_on_approval: u32,
    pub cancel_requested: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStatusCounts {
    pub active: u32,
    pub completed: u32,
    pub failed: u32,
    pub abandoned: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkflowSessionNode {
    pub session: SessionRecord,
    pub depth: u32,
    pub is_focus: bool,
    pub runtime_state: SessionRuntimeState,
    pub active_branch_id: Option<BranchId>,
    pub turn_count: u32,
    pub message_count: u32,
    pub tool_call_count: u32,
    pub pending_approval_count: u32,
    pub child_session_count: u32,
    pub last_seq_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionWorkflowInspection {
    pub focus_session_id: SessionId,
    pub root_session_id: SessionId,
    pub node_count: u32,
    pub runtime_counts: WorkflowRuntimeCounts,
    pub status_counts: WorkflowStatusCounts,
    pub nodes: Vec<WorkflowSessionNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BranchInspection {
    pub branch: BranchRecord,
    pub depth: u32,
    pub is_active: bool,
    pub total_message_count: u32,
    pub local_message_count: u32,
    pub turn_count: u32,
    pub latest_turn_id: Option<TurnId>,
    pub latest_event_seq: Option<i64>,
    pub latest_message_preview: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionTreeInspection {
    pub session: SessionRecord,
    pub active_branch_id: Option<BranchId>,
    pub branches: Vec<BranchInspection>,
    pub related_sessions: Vec<RelatedSessionSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnToolCallSummary {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub approval_status: Option<String>,
    pub execution_status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnInspection {
    pub turn_id: TurnId,
    pub branch_id: BranchId,
    pub provider: String,
    pub model: String,
    pub message_count: u32,
    #[serde(default = "default_settings_revision_id")]
    pub settings_revision_id: u64,
    pub started_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
    pub status: Option<String>,
    pub finish_reason: Option<String>,
    pub latency_ms: Option<u64>,
    pub event_seq_start: Option<i64>,
    pub event_seq_end: Option<i64>,
    pub event_count: u32,
    pub pending_approval_count: u32,
    pub raw_chunk_count: u32,
    pub tool_calls: Vec<TurnToolCallSummary>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEventCounts {
    pub session: u32,
    pub agent: u32,
    pub llm: u32,
    pub tool: u32,
    pub chain: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceTurnInspection {
    pub turn_id: TurnId,
    pub branch_id: BranchId,
    #[serde(default = "default_settings_revision_id")]
    pub settings_revision_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<TurnStartSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from_call_id: Option<ToolCallId>,
    pub provider: String,
    pub model: String,
    pub started_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
    pub status: Option<String>,
    pub finish_reason: Option<String>,
    pub latency_ms: Option<u64>,
    pub event_seq_start: Option<i64>,
    pub event_seq_end: Option<i64>,
    pub event_count: u32,
    pub raw_chunk_count: u32,
    pub llm_call_count: u32,
    pub approval_pause_count: u32,
    pub resumed_after_approval: bool,
    pub event_counts: TraceEventCounts,
    pub usage: Option<TokenUsage>,
    pub cost: Option<CostBreakdown>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_manifests: Vec<ContextManifest>,
    pub tool_calls: Vec<TurnToolCallSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionExecutionInspection {
    pub session_id: SessionId,
    pub total_events: u32,
    pub event_counts: TraceEventCounts,
    pub related_sessions: Vec<RelatedSessionSummary>,
    pub turns: Vec<TraceTurnInspection>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectionId, Message, Role, TurnId};
    use serde_json::json;

    fn test_tool_spec() -> ToolSpec {
        ToolSpec {
            name: "read".to_owned(),
            description: "Read a file".to_owned(),
            parameters_schema: json!({"type": "object"}),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: true,
                is_concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec!["filesystem".to_owned()],
                display_group: ToolDisplayGroup::Codebase,
            },
        }
    }

    #[test]
    fn context_manifest_references_model_visible_request_without_copying_bodies() {
        let request = CompletionRequest {
            connection_id: ConnectionId::new("local"),
            model: "qwen3.5:latest".to_owned(),
            system_prompt: Some("system prompt".to_owned()),
            messages: vec![Message::text(Role::User, "hello")],
            tools: vec![test_tool_spec()],
            structured_output: None,
            max_tokens: Some(1024),
            temperature: None,
            thinking: Some(ThinkingConfig {
                enabled: true,
                effort: Some(ThinkingEffort::High),
                budget_tokens: Some(512),
                include_summaries: false,
            }),
        };
        let branch_id = BranchId::new();
        let source_seq_id = 42;
        let message_sources = vec![ContextMessageSourceRef {
            message_id: request.messages[0].message_id,
            branch_id,
            seq_id: source_seq_id,
        }];

        let manifest = ContextManifest::from_completion_request(
            TurnId::default(),
            branch_id,
            1,
            "openai-compatible",
            request.model.clone(),
            7,
            Some(source_seq_id),
            &request,
            true,
            &message_sources,
        );

        assert_eq!(manifest.branch_id, branch_id);
        assert_eq!(manifest.llm_call_ordinal, 1);
        assert_eq!(manifest.settings_revision_id, 7);
        assert_eq!(manifest.context_boundary_seq_id, Some(source_seq_id));
        assert_eq!(
            manifest.system_prompt.char_count,
            "system prompt".chars().count() as u64
        );
        assert_eq!(manifest.messages.len(), 1);
        assert_eq!(manifest.messages[0].role, Role::User);
        assert_eq!(manifest.messages[0].source_branch_id, Some(branch_id));
        assert_eq!(manifest.messages[0].source_seq_id, Some(source_seq_id));
        assert_eq!(manifest.messages[0].visible_text_chars, 5);
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.tools[0].risk_class, ToolRiskClass::Safe);
        assert!(manifest.tools[0].is_read_only);
        assert!(manifest.attachments.is_empty());
        assert!(manifest.compacted);
    }

    #[test]
    fn tool_operation_context_can_be_derived_from_public_tool_metadata() {
        let tool = test_tool_spec();
        let operation = ToolOperationContext::from_tool_spec(ToolOperationInitiator::Agent, &tool);

        assert_eq!(operation.initiator, ToolOperationInitiator::Agent);
        assert_eq!(operation.risk_class, Some(ToolRiskClass::Safe));
        assert_eq!(operation.is_read_only, Some(true));
        assert_eq!(operation.execution_mode, Some(ToolExecutionMode::Immediate));
        assert!(operation.artifact_refs.is_empty());
    }

    #[test]
    fn approval_request_snapshot_hashes_and_redacts_arguments() {
        let request = ApprovalRequest {
            session_id: SessionId::new(),
            call_id: ToolCallId::new("call-one"),
            tool_name: "shell".to_owned(),
            arguments: json!({
                "command": "echo hello",
                "api_key": "secret-value",
                "call_id": "model-supplied-id"
            }),
            requirement: ApprovalRequirement::Always,
            tool_metadata: test_tool_spec().metadata,
            requested_at: OffsetDateTime::UNIX_EPOCH,
        };

        let snapshot = request.snapshot(ToolOperationInitiator::Agent, "agent_turn", None, None);

        assert!(snapshot.request_fingerprint.starts_with("shell:fnv1a64:"));
        assert!(snapshot.arguments_hash.starts_with("fnv1a64:"));
        assert!(!snapshot.request_fingerprint.contains("secret-value"));
        assert!(!snapshot.request_fingerprint.contains("model-supplied-id"));
        assert_eq!(
            snapshot
                .redacted_arguments_preview
                .as_ref()
                .and_then(|preview| preview.get("api_key"))
                .and_then(|value| value.as_str()),
            Some("[redacted]")
        );
        assert!(
            snapshot
                .redacted_arguments_preview
                .as_ref()
                .and_then(|preview| preview.get("call_id"))
                .is_none()
        );
    }
}
