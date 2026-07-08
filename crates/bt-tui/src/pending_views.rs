use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingToolView {
    pub(crate) call_id: ToolCallId,
    pub(crate) tool_name: String,
    pub(crate) detail: Option<String>,
}

pub(crate) fn pending_tool_calls(messages: &[Message]) -> Vec<PendingToolView> {
    let mut completed = std::collections::BTreeSet::new();
    for message in messages {
        if let Some(result) = message.tool_result() {
            completed.insert(result.call_id.to_string());
        }
    }

    let mut pending = messages
        .iter()
        .rev()
        .filter_map(|message| {
            message.tool_call().and_then(|call| {
                (call.tool_name != "ask" && !completed.contains(&call.call_id)).then(|| {
                    PendingToolView {
                        call_id: bt_core::ToolCallId::new(call.call_id.clone()),
                        tool_name: call.tool_name.clone(),
                        detail: summarize_tool_detail(&call.tool_name, &call.arguments),
                    }
                })
            })
        })
        .collect::<Vec<_>>();
    pending.dedup_by(|left, right| left.call_id == right.call_id);
    pending
}

pub(crate) fn summarize_tool_detail(
    tool_name: &str,
    arguments: &serde_json::Value,
) -> Option<String> {
    match tool_name {
        "list" => arguments
            .get("path")
            .and_then(|value| value.as_str())
            .map(truncate_detail),
        "shell" => arguments
            .get("command")
            .and_then(|value| value.as_str())
            .map(truncate_detail),
        "read" | "write" | "edit" => arguments
            .get("path")
            .and_then(|value| value.as_str())
            .map(truncate_detail),
        "search" => arguments
            .get("query")
            .and_then(|value| value.as_str())
            .map(truncate_detail),
        "web_search" => arguments
            .get("query")
            .and_then(|value| value.as_str())
            .map(truncate_detail),
        "fetch" | "web_fetch" => arguments
            .get("url")
            .and_then(|value| value.as_str())
            .map(truncate_detail),
        "ask" => arguments
            .get("question")
            .and_then(|value| value.as_str())
            .map(truncate_detail),
        "inspect" => arguments
            .get("query")
            .and_then(|value| value.as_str())
            .map(truncate_detail),
        _ => None,
    }
}
