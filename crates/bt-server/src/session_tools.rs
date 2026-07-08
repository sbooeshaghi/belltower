use bt_core::{
    ApprovalRequirement, BelltowerError, BranchId, PlanInspection, PlanItem, PlanStatus, Result,
    SessionId, ToolCall, ToolCallId, ToolContext, ToolDisplayGroup, ToolExecutionMode,
    ToolExecutor, ToolInterruptBehavior, ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec,
};
use bt_runtime::BelltowerRuntime;
use bt_tools::BuiltInToolRegistry;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResultEnvelope>> + Send + 'a>>;

pub fn register_session_tools(
    registry: &mut BuiltInToolRegistry,
    runtime: Arc<BelltowerRuntime>,
    session_id: SessionId,
    branch_id: BranchId,
) {
    registry.register_arc(Arc::new(PlanTool::new(
        runtime.clone(),
        session_id,
        branch_id,
    )));
    registry.register_arc(Arc::new(AskTool::new(session_id, branch_id)));
}

pub fn try_build_ask_response(tool_call: &ToolCall, response: Value) -> Result<ToolResultEnvelope> {
    if tool_call.tool_name != "ask" {
        return Err(BelltowerError::Tool(format!(
            "tool `{}` is not resumable via answer",
            tool_call.tool_name
        )));
    }
    let question = tool_call
        .arguments
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| BelltowerError::Tool("ask tool call is missing `question`".to_owned()))?;
    let choices = tool_call
        .arguments
        .get("choices")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(ToolResultEnvelope {
        call_id: ToolCallId::new(tool_call.call_id.clone()),
        tool_name: "ask".to_owned(),
        is_error: false,
        output: json!({
            "question": question,
            "choices": choices,
            "response": response,
        }),
        duration_ms: None,
    })
}

struct PlanTool {
    runtime: Arc<BelltowerRuntime>,
    session_id: SessionId,
    branch_id: BranchId,
}

impl PlanTool {
    fn new(runtime: Arc<BelltowerRuntime>, session_id: SessionId, branch_id: BranchId) -> Self {
        Self {
            runtime,
            session_id,
            branch_id,
        }
    }

    fn current_plan_or_empty(&self) -> Result<PlanInspection> {
        Ok(self
            .runtime
            .current_plan(self.session_id, self.branch_id)?
            .unwrap_or_else(|| PlanInspection {
                session_id: self.session_id,
                branch_id: self.branch_id,
                items: Vec::new(),
                updated_at: time::OffsetDateTime::now_utc(),
            }))
    }

    fn replace_plan(&self, items: Vec<PlanItem>) -> Result<PlanInspection> {
        self.runtime
            .record_plan_updated(self.session_id, self.branch_id, items, None)?;
        self.current_plan_or_empty()
    }

    fn execute_inner(&self, arguments: Value) -> Result<Value> {
        let action = required_string(&arguments, "action")?;
        let inspection = match action.as_str() {
            "list" => self.current_plan_or_empty()?,
            "create" => self.replace_plan(parse_plan_items(&arguments)?)?,
            "update" => {
                let mut inspection = self.current_plan_or_empty()?;
                for item in parse_plan_items(&arguments)? {
                    if let Some(existing) = inspection
                        .items
                        .iter_mut()
                        .find(|entry| entry.id == item.id)
                    {
                        *existing = item;
                    } else {
                        inspection.items.push(item);
                    }
                }
                self.replace_plan(inspection.items)?
            }
            "complete" => {
                let ids = parse_plan_ids(&arguments)?;
                let mut inspection = self.current_plan_or_empty()?;
                for item in &mut inspection.items {
                    if ids.iter().any(|id| id == &item.id) {
                        item.status = PlanStatus::Completed;
                    }
                }
                self.replace_plan(inspection.items)?
            }
            "clear" => self.replace_plan(Vec::new())?,
            other => {
                return Err(BelltowerError::Tool(format!(
                    "unsupported plan action `{other}`"
                )));
            }
        };
        Ok(json!({ "inspection": inspection }))
    }
}

impl ToolExecutor for PlanTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "plan".to_owned(),
            description: "Create, update, complete, clear, or list the active branch plan."
                .to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["action", "call_id"],
                "properties": {
                    "call_id": {"type": "string"},
                    "action": {
                        "type": "string",
                        "enum": ["list", "create", "update", "complete", "clear"]
                    },
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["id", "content", "status"],
                            "properties": {
                                "id": {"type": "string"},
                                "content": {"type": "string"},
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed", "blocked"]
                                }
                            },
                            "additionalProperties": false
                        }
                    },
                    "ids": {
                        "type": "array",
                        "items": {"type": "string"}
                    }
                },
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: false,
                is_concurrency_safe: true,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::Immediate,
                should_defer: false,
                catalogue_tags: vec!["plan".to_owned(), "task".to_owned(), "todo".to_owned()],
                display_group: ToolDisplayGroup::Planning,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        Box::pin(async move {
            let call_id = ToolCallId::new(required_string(&arguments, "call_id")?);
            let output = self.execute_inner(arguments)?;
            Ok(ToolResultEnvelope {
                call_id,
                tool_name: "plan".to_owned(),
                is_error: false,
                output,
                duration_ms: None,
            })
        })
    }
}

struct AskTool {
    session_id: SessionId,
    branch_id: BranchId,
}

impl AskTool {
    fn new(session_id: SessionId, branch_id: BranchId) -> Self {
        Self {
            session_id,
            branch_id,
        }
    }
}

impl ToolExecutor for AskTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask".to_owned(),
            description: "Ask the human for input and pause the turn until they answer.".to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["question", "call_id"],
                "properties": {
                    "call_id": {"type": "string"},
                    "question": {"type": "string"},
                    "choices": {
                        "type": "array",
                        "items": {"type": "string"}
                    },
                    "allow_empty": {"type": "boolean"}
                },
                "additionalProperties": false
            }),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: true,
                is_concurrency_safe: false,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                execution_mode: ToolExecutionMode::UserInput,
                should_defer: false,
                catalogue_tags: vec!["ask".to_owned(), "human".to_owned(), "input".to_owned()],
                display_group: ToolDisplayGroup::Interaction,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, _arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        let session_id = self.session_id;
        let branch_id = self.branch_id;
        Box::pin(async move {
            Err(BelltowerError::InvalidState(format!(
                "ask tool for session {session_id} branch {branch_id} should suspend before execution"
            )))
        })
    }
}

fn required_string(arguments: &Value, key: &str) -> Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BelltowerError::Tool(format!("missing string argument `{key}`")))
}

fn parse_plan_items(arguments: &Value) -> Result<Vec<PlanItem>> {
    let items = arguments
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| BelltowerError::Tool("missing array argument `items`".to_owned()))?;
    let mut parsed = Vec::with_capacity(items.len());
    for item in items {
        let id = required_string(item, "id")?;
        let content = required_string(item, "content")?;
        let status = match required_string(item, "status")?.as_str() {
            "pending" => PlanStatus::Pending,
            "in_progress" => PlanStatus::InProgress,
            "completed" => PlanStatus::Completed,
            "blocked" => PlanStatus::Blocked,
            other => {
                return Err(BelltowerError::Tool(format!(
                    "unsupported plan status `{other}`"
                )));
            }
        };
        if parsed.iter().any(|existing: &PlanItem| existing.id == id) {
            return Err(BelltowerError::Tool(format!(
                "duplicate plan item id `{id}`"
            )));
        }
        parsed.push(PlanItem {
            id,
            content,
            status,
        });
    }
    Ok(parsed)
}

fn parse_plan_ids(arguments: &Value) -> Result<Vec<String>> {
    let ids = arguments
        .get("ids")
        .and_then(Value::as_array)
        .ok_or_else(|| BelltowerError::Tool("missing array argument `ids`".to_owned()))?;
    let parsed = ids
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if parsed.is_empty() {
        return Err(BelltowerError::Tool(
            "at least one non-empty plan id is required".to_owned(),
        ));
    }
    Ok(parsed)
}
