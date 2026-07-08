use bt_core::{
    ApprovalRequirement, BelltowerError, Result, ToolContext, ToolDisplayGroup, ToolExecutionMode,
    ToolExecutor, ToolInterruptBehavior, ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec,
    traits::ToolFuture,
};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct CatalogueTool {
    tools: Vec<ToolSpec>,
}

impl CatalogueTool {
    #[must_use]
    pub fn new(tools: Vec<ToolSpec>) -> Self {
        Self { tools }
    }
}

impl ToolExecutor for CatalogueTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "catalogue".to_owned(),
            description:
                "Discover available tools by searching names, descriptions, tags, and groups."
                    .to_owned(),
            parameters_schema: json!({
                "type": "object",
                "required": ["call_id"],
                "properties": {
                    "call_id": {"type": "string"},
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 20},
                    "group": {
                        "type": "string",
                        "enum": [
                            "codebase",
                            "execution",
                            "web",
                            "planning",
                            "interaction",
                            "inspection",
                            "workflow",
                            "external"
                        ]
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
                    "discovery".to_owned(),
                    "tools".to_owned(),
                    "catalogue".to_owned(),
                ],
                display_group: ToolDisplayGroup::Inspection,
            },
        }
    }

    fn approval_requirement(&self, _arguments: &Value) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    fn execute(&self, arguments: Value, _context: ToolContext) -> ToolFuture<'_> {
        let tools = self.tools.clone();
        Box::pin(async move {
            let call_id = arguments
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    BelltowerError::Tool("missing string argument `call_id`".to_owned())
                })?
                .to_owned();
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let limit = arguments
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(8)
                .min(20) as usize;
            let group = parse_group(arguments.get("group").and_then(Value::as_str))?;

            let mut matches = tools
                .into_iter()
                .filter(|tool| tool.name != "catalogue")
                .filter(|tool| group.is_none_or(|group| tool.metadata.display_group == group))
                .filter_map(|tool| {
                    let score = match_score(&tool, &query);
                    (score > 0).then_some((score, tool))
                })
                .collect::<Vec<_>>();
            matches.sort_by(|left, right| {
                right
                    .0
                    .cmp(&left.0)
                    .then_with(|| left.1.name.cmp(&right.1.name))
            });

            let entries = matches
                .into_iter()
                .take(limit)
                .map(|(score, tool)| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parameters_schema": tool.parameters_schema,
                        "score": score,
                        "metadata": tool.metadata,
                    })
                })
                .collect::<Vec<_>>();

            Ok(ToolResultEnvelope {
                call_id: bt_core::ToolCallId::new(call_id),
                tool_name: "catalogue".to_owned(),
                is_error: false,
                output: json!({
                    "query": if query.is_empty() { Value::Null } else { Value::String(query) },
                    "group": group.map(group_name),
                    "tools": entries,
                }),
                duration_ms: None,
            })
        })
    }
}

fn parse_group(raw: Option<&str>) -> Result<Option<ToolDisplayGroup>> {
    match raw {
        None => Ok(None),
        Some("codebase") => Ok(Some(ToolDisplayGroup::Codebase)),
        Some("execution") => Ok(Some(ToolDisplayGroup::Execution)),
        Some("web") => Ok(Some(ToolDisplayGroup::Web)),
        Some("planning") => Ok(Some(ToolDisplayGroup::Planning)),
        Some("interaction") => Ok(Some(ToolDisplayGroup::Interaction)),
        Some("inspection") => Ok(Some(ToolDisplayGroup::Inspection)),
        Some("workflow") => Ok(Some(ToolDisplayGroup::Workflow)),
        Some("external") => Ok(Some(ToolDisplayGroup::External)),
        Some(other) => Err(BelltowerError::InvalidState(format!(
            "unsupported catalogue group `{other}`"
        ))),
    }
}

fn match_score(tool: &ToolSpec, query: &str) -> usize {
    if query.is_empty() {
        return 1;
    }

    let mut score = 0;
    if tool.name == query {
        score += 100;
    } else if tool.name.starts_with(query) {
        score += 50;
    } else if tool.name.contains(query) {
        score += 30;
    }

    let description = tool.description.to_ascii_lowercase();
    if description.contains(query) {
        score += 20;
    }

    score
        + tool
            .metadata
            .catalogue_tags
            .iter()
            .filter(|tag| tag.to_ascii_lowercase().contains(query))
            .count()
            * 10
}

fn group_name(group: ToolDisplayGroup) -> Value {
    Value::String(
        match group {
            ToolDisplayGroup::Codebase => "codebase",
            ToolDisplayGroup::Execution => "execution",
            ToolDisplayGroup::Web => "web",
            ToolDisplayGroup::Planning => "planning",
            ToolDisplayGroup::Interaction => "interaction",
            ToolDisplayGroup::Inspection => "inspection",
            ToolDisplayGroup::Workflow => "workflow",
            ToolDisplayGroup::External => "external",
        }
        .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::CatalogueTool;
    use bt_core::{
        ToolContext, ToolDisplayGroup, ToolExecutionMode, ToolInterruptBehavior, ToolMetadata,
        ToolRiskClass, ToolSpec, traits::ToolExecutor,
    };
    use serde_json::json;

    fn tool(name: &str, description: &str, tags: &[&str], group: ToolDisplayGroup) -> ToolSpec {
        ToolSpec {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters_schema: json!({"type": "object"}),
            metadata: ToolMetadata {
                risk_class: ToolRiskClass::Safe,
                is_read_only: true,
                is_concurrency_safe: true,
                execution_mode: ToolExecutionMode::Immediate,
                interrupt_behavior: ToolInterruptBehavior::Immediate,
                should_defer: false,
                catalogue_tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
                display_group: group,
            },
        }
    }

    #[tokio::test]
    async fn catalogue_matches_name_and_tags() {
        let tool = CatalogueTool::new(vec![
            tool(
                "read",
                "Read a file",
                &["file", "read"],
                ToolDisplayGroup::Codebase,
            ),
            tool(
                "inspect",
                "Inspect session state",
                &["trace"],
                ToolDisplayGroup::Inspection,
            ),
        ]);

        let result = tool
            .execute(
                json!({"call_id": "call-1", "query": "trace"}),
                ToolContext {
                    project_root: camino::Utf8PathBuf::from("."),
                },
            )
            .await
            .expect("catalogue result");

        let tools = result.output["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "inspect");
    }
}
