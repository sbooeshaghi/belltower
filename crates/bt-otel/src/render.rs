//! Shared text and preview rendering helpers live here. They shape message
//! parts, completion deltas, and small JSON previews for both export and
//! mirrored tracing, while OTLP assembly stays in `otlp`.

use bt_core::{CompletionDelta, Message, MessagePart, Role};
use serde::Serialize;

const TRACE_PREVIEW_LIMIT: usize = 1024;

pub(crate) fn render_message_text(message: &Message) -> String {
    let rendered = message
        .parts
        .iter()
        .map(render_message_part_text)
        .collect::<Vec<_>>()
        .join("\n");
    if rendered.is_empty() {
        "[empty message]".to_owned()
    } else {
        rendered
    }
}

pub(crate) fn render_message_part_text(part: &MessagePart) -> String {
    match part {
        MessagePart::Text { text } => text.clone(),
        MessagePart::Reasoning {
            text: Some(text),
            redacted: false,
            ..
        } => format!("[reasoning]\n{text}"),
        MessagePart::Reasoning {
            text: None,
            redacted: false,
            ..
        } => "[reasoning]".to_owned(),
        MessagePart::Reasoning { redacted: true, .. } => "[reasoning redacted]".to_owned(),
        MessagePart::ToolCall { call } => format!(
            "tool_call {} {}",
            call.tool_name,
            serde_json::to_string_pretty(&call.arguments)
                .unwrap_or_else(|_| call.arguments.to_string())
        ),
        MessagePart::ToolResult { result } => format!(
            "tool_result {} {}",
            result.tool_name,
            serde_json::to_string_pretty(&result.output)
                .unwrap_or_else(|_| result.output.to_string())
        ),
        MessagePart::Refusal {
            text: Some(text),
            provider_reason,
            ..
        } => format!(
            "{} {text}",
            format_refusal_preview(provider_reason.as_deref())
        ),
        MessagePart::Refusal {
            text: None,
            provider_reason,
            ..
        } => format_refusal_preview(provider_reason.as_deref()),
        MessagePart::Structured { schema_name, value } => {
            format_structured_preview(schema_name.as_deref(), value)
        }
    }
}

pub(crate) fn message_part_preview_text(part: &MessagePart) -> Option<String> {
    match part {
        MessagePart::Text { text } => Some(text.clone()),
        MessagePart::Reasoning {
            text: Some(text),
            redacted: false,
            ..
        } => Some(format!("[reasoning] {text}")),
        MessagePart::Reasoning {
            text: None,
            redacted: false,
            ..
        } => Some("[reasoning]".to_owned()),
        MessagePart::Reasoning { redacted: true, .. } => Some("[reasoning redacted]".to_owned()),
        MessagePart::Refusal {
            text: Some(text),
            provider_reason,
            ..
        } => Some(format!(
            "{} {text}",
            format_refusal_preview(provider_reason.as_deref())
        )),
        MessagePart::Refusal {
            text: None,
            provider_reason,
            ..
        } => Some(format_refusal_preview(provider_reason.as_deref())),
        MessagePart::Structured { schema_name, value } => {
            Some(format_structured_preview(schema_name.as_deref(), value))
        }
        MessagePart::ToolCall { .. } | MessagePart::ToolResult { .. } => None,
    }
}

pub(crate) fn format_refusal_preview(provider_reason: Option<&str>) -> String {
    provider_reason
        .map(|reason| format!("[refusal: {reason}]"))
        .unwrap_or_else(|| "[refusal]".to_owned())
}

pub(crate) fn format_structured_preview(
    schema_name: Option<&str>,
    value: &serde_json::Value,
) -> String {
    let prefix = schema_name
        .map(|name| format!("[structured: {name}]"))
        .unwrap_or_else(|| "[structured]".to_owned());
    format!(
        "{prefix} {}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    )
}

pub(crate) fn completion_delta_text_preview(deltas: &[CompletionDelta]) -> Option<String> {
    let preview = deltas
        .iter()
        .filter_map(|delta| match delta {
            CompletionDelta::AppendText { text } => Some(text.clone()),
            CompletionDelta::AppendReasoning {
                text: Some(text),
                redacted: false,
                ..
            } => Some(format!("[reasoning] {text}")),
            CompletionDelta::AppendReasoning {
                text: None,
                redacted: false,
                ..
            } => Some("[reasoning]".to_owned()),
            CompletionDelta::AppendReasoning { redacted: true, .. } => {
                Some("[reasoning redacted]".to_owned())
            }
            CompletionDelta::AppendRefusal {
                text: Some(text),
                provider_reason,
                ..
            } => Some(format!(
                "{} {text}",
                format_refusal_preview(provider_reason.as_deref())
            )),
            CompletionDelta::SetStructuredOutput { schema_name, value } => {
                Some(format_structured_preview(schema_name.as_deref(), value))
            }
            CompletionDelta::AppendToolCallArguments { .. }
            | CompletionDelta::OpenToolCall { .. }
            | CompletionDelta::CloseToolCall { .. }
            | CompletionDelta::AppendRefusal { text: None, .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!preview.is_empty()).then(|| truncate_string(&preview))
}

pub(crate) fn completion_delta_tool_preview(deltas: &[CompletionDelta]) -> Option<String> {
    deltas.iter().find_map(|delta| match delta {
        CompletionDelta::OpenToolCall {
            call_id,
            tool_name,
            arguments,
        } => Some(truncate_string(&format!(
            "{} {} {}",
            tool_name,
            call_id,
            arguments
                .as_ref()
                .map(preview_json)
                .unwrap_or_else(|| "{}".to_owned())
        ))),
        CompletionDelta::AppendToolCallArguments {
            call_id,
            partial_json,
        } => Some(truncate_string(&format!("{call_id} {partial_json}"))),
        CompletionDelta::CloseToolCall { call_id } => {
            Some(truncate_string(&format!("closed {call_id}")))
        }
        CompletionDelta::AppendText { .. }
        | CompletionDelta::AppendReasoning { .. }
        | CompletionDelta::AppendRefusal { .. }
        | CompletionDelta::SetStructuredOutput { .. } => None,
    })
}

pub(crate) fn serialize_json(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<unserializable>\"".to_owned())
}

pub(crate) fn preview_json(value: &impl Serialize) -> String {
    let serialized =
        serde_json::to_string(value).unwrap_or_else(|_| "\"<unserializable>\"".to_owned());
    truncate_string(&serialized)
}

pub(crate) fn role_name(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

pub(crate) fn format_timestamp(timestamp: time::OffsetDateTime) -> String {
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| timestamp.to_string())
}

pub(crate) fn truncate_string(value: &str) -> String {
    if value.chars().count() <= TRACE_PREVIEW_LIMIT {
        return value.to_owned();
    }

    let truncated = value.chars().take(TRACE_PREVIEW_LIMIT).collect::<String>();
    format!("{truncated}…")
}

pub(crate) fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
