use bt_core::{
    ApprovalRequirement, BelltowerError, BranchId, Result, SessionId, ToolCallId, ToolContext,
    ToolDisplayGroup, ToolExecutionMode, ToolExecutor, ToolInterruptBehavior, ToolMetadata,
    ToolResultEnvelope, ToolRiskClass, ToolSpec,
};
use bt_runtime::BelltowerRuntime;
use bt_tools::BuiltInToolRegistry;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

const DEFAULT_SEARCH_LIMIT: usize = 8;
const MAX_SEARCH_LIMIT: usize = 50;

type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResultEnvelope>> + Send + 'a>>;

pub fn register_session_search_tool(
    registry: &mut BuiltInToolRegistry,
    runtime: Arc<BelltowerRuntime>,
    session_id: SessionId,
    branch_id: BranchId,
) {
    registry.register_arc(Arc::new(SessionSearchTool {
        runtime,
        session_id,
        branch_id,
    }));
}

struct SessionSearchTool {
    runtime: Arc<BelltowerRuntime>,
    session_id: SessionId,
    branch_id: BranchId,
}

impl SessionSearchTool {
    fn execute_inner(&self, arguments: &Value) -> Result<Value> {
        let query = required_string(arguments, "query")?;
        let limit = parse_limit(arguments)?;
        let branch_id = if arguments
            .get("include_all_branches")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            None
        } else {
            Some(self.branch_id)
        };
        let results =
            self.runtime
                .search_session_history(self.session_id, branch_id, &query, limit)?;
        Ok(json!({
            "session_id": self.session_id,
            "branch_id": branch_id,
            "query": query,
            "limit": limit,
            "results": results,
        }))
    }
}

impl ToolExecutor for SessionSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "session_search".to_owned(),
            description:
                "Search the canonical session history by exact text and return event, turn, and tool references."
                    .to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["query", "call_id"],
                "properties": {
                    "call_id": {"type": "string"},
                    "query": {
                        "type": "string",
                        "description": "Exact text to search for in the current branch's canonical session history."
                    },
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_SEARCH_LIMIT},
                    "include_all_branches": {
                        "type": "boolean",
                        "description": "Search all branches in the session instead of the current branch lineage."
                    }
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
                    "session".to_owned(),
                    "history".to_owned(),
                    "search".to_owned(),
                    "telemetry".to_owned(),
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
            let call_id = ToolCallId::new(required_string(&arguments, "call_id")?);
            Ok(ToolResultEnvelope {
                call_id,
                tool_name: "session_search".to_owned(),
                is_error: false,
                output: self.execute_inner(&arguments)?,
                duration_ms: None,
            })
        })
    }
}

fn required_string(arguments: &Value, field: &str) -> Result<String> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BelltowerError::InvalidState(format!("missing string argument `{field}`")))
}

fn parse_limit(arguments: &Value) -> Result<usize> {
    let Some(raw) = arguments.get("limit") else {
        return Ok(DEFAULT_SEARCH_LIMIT);
    };
    let raw = raw.as_u64().ok_or_else(|| {
        BelltowerError::InvalidState("missing integer argument `limit`".to_owned())
    })?;
    Ok(raw.clamp(1, MAX_SEARCH_LIMIT as u64) as usize)
}
