use base64::Engine;
use bt_core::{
    ApprovalRequirement, BelltowerError, BranchId, Result, SessionId, ToolCallId, ToolContext,
    ToolDisplayGroup, ToolExecutionMode, ToolExecutor, ToolInterruptBehavior, ToolMetadata,
    ToolResultEnvelope, ToolRiskClass, ToolSpec, TurnId, TurnInspection,
};
use bt_runtime::BelltowerRuntime;
use bt_tools::{BuiltInToolRegistry, truncate_text};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

const DEFAULT_TURN_LIMIT: usize = 10;
const MAX_TURN_LIMIT: usize = 50;
const DEFAULT_RAW_LIMIT: usize = 20;
const MAX_RAW_LIMIT: usize = 100;
const RAW_PREVIEW_LINES: usize = 64;
const RAW_PREVIEW_BYTES: usize = 4_096;

type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResultEnvelope>> + Send + 'a>>;

pub fn register_inspection_tools(
    registry: &mut BuiltInToolRegistry,
    runtime: Arc<BelltowerRuntime>,
    session_id: SessionId,
    branch_id: BranchId,
) {
    registry.register_arc(Arc::new(InspectionTool::new(
        runtime, session_id, branch_id,
    )));
}

#[derive(Clone, Copy)]
enum InspectionQuery {
    Session,
    Queue,
    Errors,
    Tool,
    Plan,
    Turns,
    Execution,
    RawTurn,
    Mcp,
    Lineage,
    Workflow,
}

impl InspectionQuery {
    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "session" => Ok(Self::Session),
            "queue" => Ok(Self::Queue),
            "errors" => Ok(Self::Errors),
            "tool" => Ok(Self::Tool),
            "plan" | "current_plan" => Ok(Self::Plan),
            "turns" => Ok(Self::Turns),
            "execution" => Ok(Self::Execution),
            "raw_turn" => Ok(Self::RawTurn),
            "mcp" => Ok(Self::Mcp),
            "lineage" => Ok(Self::Lineage),
            "workflow" => Ok(Self::Workflow),
            _ => Err(BelltowerError::InvalidState(format!(
                "unsupported inspect query `{raw}`"
            ))),
        }
    }
}

struct InspectionTool {
    runtime: Arc<BelltowerRuntime>,
    session_id: SessionId,
    branch_id: BranchId,
}

impl InspectionTool {
    fn new(runtime: Arc<BelltowerRuntime>, session_id: SessionId, branch_id: BranchId) -> Self {
        Self {
            runtime,
            session_id,
            branch_id,
        }
    }

    async fn execute_inner(&self, query: InspectionQuery, arguments: Value) -> Result<Value> {
        match query {
            InspectionQuery::Session => {
                let inspection = self
                    .runtime
                    .inspect_session(self.session_id)?
                    .ok_or_else(|| missing_session(self.session_id))?;
                Ok(json!({ "inspection": inspection }))
            }
            InspectionQuery::Queue => {
                let inspection = self
                    .runtime
                    .inspect_queue(self.session_id)?
                    .ok_or_else(|| missing_session(self.session_id))?;
                Ok(json!({ "inspection": inspection }))
            }
            InspectionQuery::Errors => {
                let limit = parse_limit(&arguments, "limit", DEFAULT_TURN_LIMIT, MAX_TURN_LIMIT)?;
                let errors = self
                    .runtime
                    .session_errors(self.session_id)?
                    .ok_or_else(|| missing_session(self.session_id))?
                    .into_iter()
                    .rev()
                    .take(limit)
                    .collect::<Vec<_>>();
                Ok(json!({
                    "session_id": self.session_id,
                    "limit": limit,
                    "errors": errors
                }))
            }
            InspectionQuery::Tool => {
                let target_call_id = parse_target_call_id(&arguments)?;
                if let Some(inspection) = self
                    .runtime
                    .inspect_tool_call(self.session_id, target_call_id.clone())?
                {
                    return Ok(json!({ "inspection": inspection }));
                }

                let recent = self.recent_tool_call_ids()?;
                let suffix = if recent.is_empty() {
                    "no tool calls are recorded in this session".to_owned()
                } else {
                    format!("recent tool call ids: {}", recent.join(", "))
                };
                Err(BelltowerError::InvalidState(format!(
                    "tool call `{target_call_id}` not found in session `{}`; {suffix}",
                    self.session_id
                )))
            }
            InspectionQuery::Plan => {
                let inspection = self
                    .runtime
                    .current_plan(self.session_id, self.branch_id)?
                    .ok_or_else(|| {
                        BelltowerError::InvalidState("no active plan for this branch".to_owned())
                    })?;
                Ok(json!({ "inspection": inspection }))
            }
            InspectionQuery::Turns => {
                let limit = parse_limit(&arguments, "limit", DEFAULT_TURN_LIMIT, MAX_TURN_LIMIT)?;
                let turns = self
                    .runtime
                    .turn_history(self.session_id)?
                    .into_iter()
                    .rev()
                    .take(limit)
                    .collect::<Vec<_>>();
                Ok(json!({
                    "session_id": self.session_id,
                    "limit": limit,
                    "turns": turns
                }))
            }
            InspectionQuery::Execution => {
                let limit = parse_limit(&arguments, "limit", DEFAULT_TURN_LIMIT, MAX_TURN_LIMIT)?;
                let inspection = self
                    .runtime
                    .session_execution(self.session_id)?
                    .ok_or_else(|| missing_session(self.session_id))?;
                let turns = inspection
                    .turns
                    .into_iter()
                    .rev()
                    .take(limit)
                    .collect::<Vec<_>>();
                Ok(json!({
                    "session_id": inspection.session_id,
                    "total_events": inspection.total_events,
                    "event_counts": inspection.event_counts,
                    "related_sessions": inspection.related_sessions,
                    "limit": limit,
                    "turns": turns
                }))
            }
            InspectionQuery::RawTurn => self.inspect_raw_turn(arguments),
            InspectionQuery::Mcp => {
                let inventory = self.runtime.mcp_inventory().await?;
                let tools = inventory
                    .tools
                    .into_iter()
                    .map(|tool| {
                        json!({
                            "server_name": tool.server_name,
                            "tool_name": tool.tool_name,
                            "qualified_name": tool.qualified_name,
                            "description": tool.description,
                            "parameters_schema": tool.parameters_schema,
                            "risk_class": "high",
                            "display_group": "external",
                            "approval_requirement": "conditional",
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(json!({
                    "servers": inventory.servers,
                    "tools": tools,
                }))
            }
            InspectionQuery::Lineage => {
                let inspection = self
                    .runtime
                    .session_lineage(self.session_id)?
                    .ok_or_else(|| missing_session(self.session_id))?;
                Ok(json!({ "inspection": inspection }))
            }
            InspectionQuery::Workflow => {
                let inspection = self
                    .runtime
                    .session_workflow(self.session_id)?
                    .ok_or_else(|| missing_session(self.session_id))?;
                Ok(json!({ "inspection": inspection }))
            }
        }
    }

    fn recent_tool_call_ids(&self) -> Result<Vec<String>> {
        let Some(execution) = self.runtime.session_execution(self.session_id)? else {
            return Ok(Vec::new());
        };

        let mut ids = execution
            .turns
            .iter()
            .rev()
            .flat_map(|turn| turn.tool_calls.iter().rev())
            .map(|tool| tool.call_id.to_string())
            .take(5)
            .collect::<Vec<_>>();
        ids.dedup();
        Ok(ids)
    }

    fn inspect_raw_turn(&self, arguments: Value) -> Result<Value> {
        let limit = parse_limit(&arguments, "limit", DEFAULT_RAW_LIMIT, MAX_RAW_LIMIT)?;
        let llm_call_ordinal = parse_llm_call_ordinal(&arguments)?;
        let turns = self.runtime.turn_history(self.session_id)?;
        let (selected_turn_id, selected_branch_id, selected_turn) =
            select_turn(&turns, self.branch_id, parse_turn_id(&arguments)?)?;

        let page = self.runtime.turn_raw_chunks_page(
            self.session_id,
            selected_branch_id,
            selected_turn_id,
            llm_call_ordinal,
            None,
            limit,
        )?;

        let chunks = page
            .items
            .into_iter()
            .map(|record| {
                let (preview_kind, preview) = render_raw_preview(&record.content);
                json!({
                    "chunk_id": record.chunk_id,
                    "branch_id": record.branch_id,
                    "turn_id": record.turn_id,
                    "llm_call_ordinal": record.llm_call_ordinal,
                    "event_id": record.event_id,
                    "provider": record.provider,
                    "stream_name": record.stream_name,
                    "received_at": record.received_at,
                    "preview_kind": preview_kind,
                    "preview": preview
                })
            })
            .collect::<Vec<_>>();

        Ok(json!({
            "session_id": self.session_id,
            "selected_turn": selected_turn,
            "llm_call_ordinal": llm_call_ordinal,
            "limit": limit,
            "has_more_before": page.has_more_before,
            "oldest_chunk_id": page.oldest_chunk_id,
            "newest_chunk_id": page.newest_chunk_id,
            "chunks": chunks
        }))
    }
}

impl ToolExecutor for InspectionTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "inspect".to_owned(),
            description:
                "Inspect session, queue, errors, a tool call by id, plan, turns, execution, raw chunks, MCP, lineage, or workflow state."
                    .to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["query", "call_id"],
                "properties": {
                    "call_id": {"type": "string"},
                    "query": {
                        "type": "string",
                        "enum": ["session", "queue", "errors", "tool", "plan", "current_plan", "turns", "execution", "raw_turn", "mcp", "lineage", "workflow"]
                    },
                    "target_call_id": {"type": "string"},
                    "tool_call_id": {"type": "string"},
                    "id": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_TURN_LIMIT},
                    "turn_id": {"type": "string"},
                    "llm_call_ordinal": {"type": "integer", "minimum": 1}
                },
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: true,
                is_concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec![
                    "inspect".to_owned(),
                    "execution".to_owned(),
                    "errors".to_owned(),
                    "workflow".to_owned(),
                    "raw".to_owned(),
                    "plan".to_owned(),
                    "mcp".to_owned(),
                ],
                display_group: ToolDisplayGroup::Inspection,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let call_id = parse_call_id(&arguments)?;
            let query = parse_query(&arguments)?;
            let output = self.execute_inner(query, arguments).await?;
            Ok(ToolResultEnvelope {
                call_id,
                tool_name: "inspect".to_owned(),
                is_error: false,
                output,
                duration_ms: None,
            })
        })
    }
}

fn parse_call_id(arguments: &Value) -> Result<ToolCallId> {
    arguments
        .get("call_id")
        .and_then(Value::as_str)
        .map(ToolCallId::new)
        .ok_or_else(|| BelltowerError::InvalidState("missing string argument `call_id`".to_owned()))
}

fn parse_query(arguments: &Value) -> Result<InspectionQuery> {
    let raw = arguments
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            BelltowerError::InvalidState("missing string argument `query`".to_owned())
        })?;
    InspectionQuery::parse(raw)
}

fn parse_target_call_id(arguments: &Value) -> Result<ToolCallId> {
    arguments
        .get("target_call_id")
        .or_else(|| arguments.get("tool_call_id"))
        .or_else(|| arguments.get("lookup_call_id"))
        .or_else(|| arguments.get("id"))
        .and_then(Value::as_str)
        .map(ToolCallId::new)
        .ok_or_else(|| {
            BelltowerError::InvalidState(
                "missing string argument `target_call_id` (or `tool_call_id` / `id`)".to_owned(),
            )
        })
}

fn parse_limit(arguments: &Value, field: &str, default: usize, max: usize) -> Result<usize> {
    let Some(raw) = arguments.get(field) else {
        return Ok(default);
    };
    let raw = raw.as_u64().ok_or_else(|| {
        BelltowerError::InvalidState(format!("missing integer argument `{field}`"))
    })?;
    Ok(raw.clamp(1, max as u64) as usize)
}

fn parse_turn_id(arguments: &Value) -> Result<Option<TurnId>> {
    let Some(raw) = arguments.get("turn_id") else {
        return Ok(None);
    };
    let raw = raw.as_str().ok_or_else(|| {
        BelltowerError::InvalidState("missing string argument `turn_id`".to_owned())
    })?;
    let turn_id = TurnId::from_str(raw)
        .map_err(|error| BelltowerError::InvalidState(format!("invalid `turn_id`: {error}")))?;
    Ok(Some(turn_id))
}

fn parse_llm_call_ordinal(arguments: &Value) -> Result<Option<u32>> {
    let Some(raw) = arguments.get("llm_call_ordinal") else {
        return Ok(None);
    };
    let raw = raw.as_u64().ok_or_else(|| {
        BelltowerError::InvalidState("missing integer argument `llm_call_ordinal`".to_owned())
    })?;
    let ordinal = u32::try_from(raw).map_err(|error| {
        BelltowerError::InvalidState(format!("invalid `llm_call_ordinal`: {error}"))
    })?;
    if ordinal == 0 {
        return Err(BelltowerError::InvalidState(
            "`llm_call_ordinal` must be >= 1".to_owned(),
        ));
    }
    Ok(Some(ordinal))
}

fn select_turn(
    turns: &[TurnInspection],
    current_branch_id: BranchId,
    explicit_turn_id: Option<TurnId>,
) -> Result<(TurnId, BranchId, Value)> {
    let selected = match explicit_turn_id {
        Some(turn_id) => turns.iter().find(|turn| turn.turn_id == turn_id),
        None => turns
            .iter()
            .rev()
            .find(|turn| turn.branch_id == current_branch_id),
    }
    .ok_or_else(|| {
        BelltowerError::InvalidState(match explicit_turn_id {
            Some(turn_id) => format!("turn `{turn_id}` not found in this session"),
            None => "no turns recorded yet on the current branch".to_owned(),
        })
    })?;

    Ok((
        selected.turn_id,
        selected.branch_id,
        serde_json::to_value(selected)
            .map_err(|error| BelltowerError::InvalidState(error.to_string()))?,
    ))
}

fn render_raw_preview(content: &[u8]) -> (&'static str, String) {
    match std::str::from_utf8(content) {
        Ok(text) => (
            "utf8",
            truncate_text(text, RAW_PREVIEW_LINES, RAW_PREVIEW_BYTES),
        ),
        Err(_) => (
            "base64",
            truncate_text(
                &base64::engine::general_purpose::STANDARD.encode(content),
                RAW_PREVIEW_LINES,
                RAW_PREVIEW_BYTES,
            ),
        ),
    }
}

fn missing_session(session_id: SessionId) -> BelltowerError {
    BelltowerError::InvalidState(format!("session `{session_id}` not found"))
}

#[cfg(test)]
mod tests {
    use super::InspectionTool;
    use bt_core::{
        BelltowerConfig, CompletionSummary, ConnectionId, FinishReason, Role, TokenUsage,
        ToolCallId, ToolContext, TurnId, traits::ToolExecutor,
    };
    use bt_runtime::BelltowerRuntime;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    fn fixture() -> (
        Arc<BelltowerRuntime>,
        bt_core::SessionRecord,
        bt_core::BranchRecord,
        NamedTempFile,
    ) {
        let config = BelltowerConfig::from_embedded().expect("config");
        let file = NamedTempFile::new().expect("tempfile");
        let runtime = Arc::new(BelltowerRuntime::open(config, file.path()).expect("runtime"));
        let (session, branch) = runtime
            .create_session(
                "/tmp/project".into(),
                ConnectionId::new("local"),
                None,
                bt_core::SessionToolMode::Extended,
                Some("inspection-demo".to_owned()),
                None,
            )
            .expect("session");
        (runtime, session, branch, file)
    }

    fn tool(
        runtime: Arc<BelltowerRuntime>,
        session: bt_core::SessionRecord,
        branch: bt_core::BranchRecord,
    ) -> InspectionTool {
        InspectionTool::new(runtime, session.session_id, branch.branch_id)
    }

    fn context() -> ToolContext {
        ToolContext {
            project_root: "/tmp/project".into(),
            cancellation: None,
        }
    }

    #[tokio::test]
    async fn inspect_session_reports_current_counts() {
        let (runtime, session, branch, _file) = fixture();
        runtime
            .append_message(&session, &branch, Role::User, "hello")
            .expect("append message");
        let expected_session_id = session.session_id;

        let result = tool(runtime, session, branch)
            .execute(
                json!({ "call_id": "call-1", "query": "session" }),
                context(),
            )
            .await
            .expect("tool result");

        assert_eq!(result.tool_name, "inspect");
        assert_eq!(result.output["inspection"]["message_count"], 1);
        assert_eq!(
            result.output["inspection"]["session"]["session_id"],
            json!(expected_session_id)
        );
    }

    #[tokio::test]
    async fn inspect_turns_returns_latest_first_with_limit() {
        let (runtime, session, branch, _file) = fixture();
        let first_turn = TurnId::new();
        let second_turn = TurnId::new();

        for turn_id in [first_turn, second_turn] {
            runtime
                .record_turn_started(
                    session.session_id,
                    branch.branch_id,
                    turn_id,
                    "openai-compatible".to_owned(),
                    "o4-mini".to_owned(),
                    1,
                    session.settings_revision_id,
                    bt_core::TurnStartSource::UserMessage,
                    None,
                )
                .expect("turn started");
            runtime
                .record_completion_requested(
                    session.session_id,
                    branch.branch_id,
                    1,
                    "openai-compatible".to_owned(),
                    "o4-mini".to_owned(),
                    1,
                    turn_id,
                )
                .expect("completion requested");
            runtime
                .record_completion_finished(
                    session.session_id,
                    branch.branch_id,
                    CompletionSummary {
                        provider: "openai-compatible".to_owned(),
                        model: "o4-mini".to_owned(),
                        finish_reason: FinishReason::Stop,
                        usage: TokenUsage {
                            prompt_tokens: 10,
                            completion_tokens: 5,
                            total_tokens: 15,
                            cache_read_tokens: None,
                            cache_write_tokens: None,
                            reasoning_tokens: None,
                        },
                        cost: None,
                        latency_ms: 42,
                    },
                    1,
                    turn_id,
                )
                .expect("completion finished");
            runtime
                .record_turn_finished(
                    session.session_id,
                    branch.branch_id,
                    turn_id,
                    "openai-compatible".to_owned(),
                    "o4-mini".to_owned(),
                    "completed".to_owned(),
                    Some("Stop".to_owned()),
                    42,
                )
                .expect("turn finished");
        }

        let result = tool(runtime, session, branch)
            .execute(
                json!({ "call_id": "call-2", "query": "turns", "limit": 1 }),
                context(),
            )
            .await
            .expect("tool result");

        let turns = result.output["turns"].as_array().expect("turn array");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["turn_id"], json!(second_turn));
    }

    #[tokio::test]
    async fn inspect_errors_returns_latest_canonical_errors_first() {
        let (runtime, session, branch, _file) = fixture();
        let turn_id = TurnId::new();
        runtime
            .record_turn_started(
                session.session_id,
                branch.branch_id,
                turn_id,
                "openai-compatible".to_owned(),
                "qwen3.5:latest".to_owned(),
                1,
                session.settings_revision_id,
                bt_core::TurnStartSource::UserMessage,
                None,
            )
            .expect("turn started");
        runtime
            .record_session_error(
                session.session_id,
                branch.branch_id,
                &bt_core::BelltowerError::Provider("first provider failure".to_owned()),
                Some(turn_id),
                bt_core::SpanKind::Llm,
            )
            .expect("first error");
        runtime
            .record_session_error(
                session.session_id,
                branch.branch_id,
                &bt_core::BelltowerError::Runtime("second runtime failure".to_owned()),
                Some(turn_id),
                bt_core::SpanKind::Agent,
            )
            .expect("second error");

        let result = tool(runtime, session, branch)
            .execute(
                json!({ "call_id": "call-errors", "query": "errors", "limit": 1 }),
                context(),
            )
            .await
            .expect("tool result");

        assert_eq!(result.tool_name, "inspect");
        let errors = result.output["errors"].as_array().expect("error array");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["code"], json!("runtime_error"));
        assert_eq!(errors[0]["turn_id"], json!(turn_id));
        assert_eq!(errors[0]["span_kind"], json!("Agent"));
    }

    #[tokio::test]
    async fn inspect_tool_returns_projection_and_event_context() {
        let (runtime, session, branch, _file) = fixture();
        let turn_id = TurnId::new();
        let call_id = ToolCallId::new("call-list");
        runtime
            .record_turn_started(
                session.session_id,
                branch.branch_id,
                turn_id,
                "openai-compatible".to_owned(),
                "o4-mini".to_owned(),
                1,
                session.settings_revision_id,
                bt_core::TurnStartSource::UserMessage,
                None,
            )
            .expect("turn started");
        runtime
            .record_tool_call_requested(
                session.session_id,
                branch.branch_id,
                call_id.clone(),
                "list".to_owned(),
                json!({ "path": "." }),
                Some(turn_id),
            )
            .expect("tool call requested");
        runtime
            .record_tool_execution(
                session.session_id,
                branch.branch_id,
                bt_core::ToolResultEnvelope {
                    call_id: call_id.clone(),
                    tool_name: "list".to_owned(),
                    is_error: false,
                    output: json!({ "entries": ["Cargo.toml", "crates"] }),
                    duration_ms: Some(12),
                },
                Some(turn_id),
            )
            .expect("tool result");

        let result = tool(runtime, session, branch)
            .execute(
                json!({
                    "call_id": "call-inspect-tool",
                    "query": "tool",
                    "target_call_id": call_id,
                }),
                context(),
            )
            .await
            .expect("tool result");

        assert_eq!(result.tool_name, "inspect");
        assert_eq!(result.output["inspection"]["tool_name"], json!("list"));
        assert_eq!(result.output["inspection"]["turn_id"], json!(turn_id));
        assert_eq!(result.output["inspection"]["requested_seq_id"], json!(4));
        assert_eq!(result.output["inspection"]["completed_seq_id"], json!(5));
        assert_eq!(
            result.output["inspection"]["execution_status"],
            json!("completed")
        );
        assert_eq!(result.output["inspection"]["arguments"]["path"], json!("."));
    }

    #[tokio::test]
    async fn inspect_tool_accepts_model_facing_tool_call_id_alias() {
        let (runtime, session, branch, _file) = fixture();
        let call_id = ToolCallId::new("call-read");
        runtime
            .record_tool_call_requested(
                session.session_id,
                branch.branch_id,
                call_id.clone(),
                "read".to_owned(),
                json!({ "path": "README.md" }),
                None,
            )
            .expect("tool call requested");

        let result = tool(runtime, session, branch)
            .execute(
                json!({
                    "call_id": "call-inspect-tool",
                    "query": "tool",
                    "tool_call_id": call_id,
                }),
                context(),
            )
            .await
            .expect("tool result");

        assert_eq!(result.output["inspection"]["tool_name"], json!("read"));
        assert_eq!(
            result.output["inspection"]["arguments"]["path"],
            json!("README.md")
        );
    }

    #[tokio::test]
    async fn inspect_tool_missing_call_reports_recent_ids() {
        let (runtime, session, branch, _file) = fixture();
        let turn_id = TurnId::new();
        runtime
            .record_turn_started(
                session.session_id,
                branch.branch_id,
                turn_id,
                "openai-compatible".to_owned(),
                "o4-mini".to_owned(),
                1,
                session.settings_revision_id,
                bt_core::TurnStartSource::UserMessage,
                None,
            )
            .expect("turn started");
        runtime
            .record_tool_call_requested(
                session.session_id,
                branch.branch_id,
                ToolCallId::new("call-read"),
                "read".to_owned(),
                json!({ "path": "README.md" }),
                Some(turn_id),
            )
            .expect("tool call requested");

        let error = tool(runtime, session, branch)
            .execute(
                json!({
                    "call_id": "call-inspect-tool",
                    "query": "tool",
                    "tool_call_id": "call-missing",
                }),
                context(),
            )
            .await
            .expect_err("missing target call should report context");
        let message = error.to_string();

        assert!(message.contains("tool call `call-missing` not found"));
        assert!(message.contains("recent tool call ids: call-read"));
    }

    #[tokio::test]
    async fn inspect_workflow_reports_child_sessions() {
        let (runtime, session, branch, _file) = fixture();
        runtime
            .append_message(&session, &branch, Role::User, "hello")
            .expect("append message");
        let _child = runtime
            .spawn_child_session(
                session.session_id,
                branch.branch_id,
                None,
                "inspect logs".to_owned(),
                Some("child".to_owned()),
                None,
                None,
            )
            .expect("spawn child");

        let result = tool(runtime, session, branch)
            .execute(
                json!({ "call_id": "call-workflow", "query": "workflow" }),
                context(),
            )
            .await
            .expect("tool result");

        assert_eq!(result.tool_name, "inspect");
        assert_eq!(result.output["inspection"]["node_count"], json!(2));
        assert_eq!(
            result.output["inspection"]["nodes"][0]["child_session_count"],
            json!(1)
        );
    }

    #[tokio::test]
    async fn inspect_raw_turn_defaults_to_latest_turn_on_current_branch() {
        let (runtime, session, branch, _file) = fixture();
        let turn_id = TurnId::new();
        runtime
            .record_turn_started(
                session.session_id,
                branch.branch_id,
                turn_id,
                "openai-compatible".to_owned(),
                "o4-mini".to_owned(),
                1,
                session.settings_revision_id,
                bt_core::TurnStartSource::UserMessage,
                None,
            )
            .expect("turn started");
        runtime
            .append_raw_chunk(
                session.session_id,
                branch.branch_id,
                Some(turn_id),
                "openai-compatible",
                "response",
                Some(1),
                b"{\"delta\":\"hello\"}",
            )
            .expect("raw chunk");

        let result = tool(runtime, session, branch)
            .execute(
                json!({ "call_id": "call-3", "query": "raw_turn" }),
                context(),
            )
            .await
            .expect("tool result");

        assert_eq!(result.output["selected_turn"]["turn_id"], json!(turn_id));
        assert_eq!(result.output["chunks"][0]["llm_call_ordinal"], 1);
        assert_eq!(result.output["chunks"][0]["preview_kind"], "utf8");
    }

    #[tokio::test]
    async fn inspect_mcp_returns_server_and_tool_inventory() {
        let (runtime, session, branch, _file) = fixture();

        let result = tool(runtime, session, branch)
            .execute(json!({ "call_id": "call-mcp", "query": "mcp" }), context())
            .await
            .expect("tool result");

        assert_eq!(result.tool_name, "inspect");
        assert!(result.output.get("servers").is_some());
        assert!(result.output.get("tools").is_some());
        assert!(result.output["servers"].is_array());
        assert!(result.output["tools"].is_array());
    }
}
