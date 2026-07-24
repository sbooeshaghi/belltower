//! OpenAI-compatible request translation.
//!
//! This module owns provider-specific request DTOs, message conversion, and
//! tool/structured-output request knobs. Streaming response parsing stays in
//! the parent `openai` module.

use bt_core::{
    CompletionRequest, Message, MessagePart, Role, StructuredOutputSpec, ThinkingConfig,
    ThinkingEffort, ToolSpec,
};
use serde::Serialize;
use serde_json::{Value, json};

pub(super) fn build_chat_request(
    provider_id: &str,
    request: CompletionRequest,
) -> OpenAiChatRequest {
    let tool_choice = preferred_tool_choice(provider_id, &request);
    let response_format =
        structured_output_response_format(provider_id, request.structured_output.as_ref());
    let mut messages = Vec::new();
    if let Some(system_prompt) = request
        .system_prompt
        .filter(|value| !value.trim().is_empty())
    {
        messages.push(OpenAiMessage {
            role: "system".to_owned(),
            content: Some(json!(system_prompt)),
            tool_calls: None,
            tool_call_id: None,
        });
    }
    messages.extend(
        request
            .messages
            .iter()
            .map(|message| message_to_openai(provider_id, message)),
    );

    let uses_reasoning_token_budget =
        uses_openai_reasoning_token_budget(provider_id, &request.model);
    let reasoning_effort =
        openai_reasoning_effort(provider_id, &request.model, request.thinking.as_ref());
    let (max_tokens, max_completion_tokens) = if uses_reasoning_token_budget {
        (None, request.max_tokens)
    } else {
        (request.max_tokens, None)
    };
    OpenAiChatRequest {
        model: request.model,
        stream: true,
        stream_options: uses_openai_stream_usage(provider_id).then_some(OpenAiStreamOptions {
            include_usage: true,
        }),
        max_tokens,
        max_completion_tokens,
        temperature: request.temperature,
        tool_choice,
        tools: (!request.tools.is_empty()).then(|| {
            request
                .tools
                .iter()
                .map(tool_spec_to_openai)
                .collect::<Vec<_>>()
        }),
        response_format,
        reasoning_effort,
        messages,
    }
}

fn uses_openai_reasoning_token_budget(provider_id: &str, model: &str) -> bool {
    provider_id == "openai" && bt_core::model_capability::openai_reasoning_family(model)
}

fn uses_openai_stream_usage(provider_id: &str) -> bool {
    provider_id == "openai"
}

fn openai_reasoning_effort(
    provider_id: &str,
    model: &str,
    thinking: Option<&ThinkingConfig>,
) -> Option<ThinkingEffort> {
    if !uses_openai_reasoning_token_budget(provider_id, model) {
        return None;
    }
    let thinking = thinking?;
    if !thinking.enabled {
        return None;
    }
    let effort = thinking.effort.unwrap_or(ThinkingEffort::High);
    Some(bt_core::model_capability::clamp_openai_chat_reasoning_effort(effort))
}

fn preferred_tool_choice(_provider_id: &str, request: &CompletionRequest) -> Option<String> {
    (!request.tools.is_empty()).then(|| "auto".to_owned())
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiChatRequest {
    pub(super) model: String,
    pub(super) messages: Vec<OpenAiMessage>,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream_options: Option<OpenAiStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) response_format: Option<OpenAiResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_effort: Option<ThinkingEffort>,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiStreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiMessage {
    pub(super) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_calls: Option<Vec<OpenAiMessageToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiMessageToolCall {
    pub(super) id: String,
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) function: OpenAiMessageToolCallFunction,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiMessageToolCallFunction {
    pub(super) name: String,
    pub(super) arguments: String,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiFunctionSpec,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiFunctionSpec {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
    json_schema: OpenAiJsonSchema,
}

#[derive(Debug, Serialize)]
pub(super) struct OpenAiJsonSchema {
    name: String,
    schema: Value,
    strict: bool,
}

pub(super) fn message_to_openai(provider_id: &str, message: &Message) -> OpenAiMessage {
    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };

    let content = render_message_text_for_openai(&message.parts);
    let tool_calls = message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::ToolCall { call } => Some(OpenAiMessageToolCall {
                id: call.call_id.clone(),
                kind: "function",
                function: OpenAiMessageToolCallFunction {
                    name: call.tool_name.clone(),
                    arguments: stringify_openai_message_value(&call.arguments),
                },
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let tool_call_id = message
        .tool_result()
        .map(|result| result.call_id.to_string());

    let content = if !content.is_empty() {
        Some(json!(content))
    } else {
        // Empty assistant/tool-history messages should still serialize with an
        // explicit empty string. Hosted OpenAI and stricter compatible
        // backends can reject null content in message histories, especially
        // around tool-call-only assistant turns.
        let _ = provider_id;
        Some(json!(""))
    };

    OpenAiMessage {
        role: role.to_owned(),
        content,
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        tool_call_id,
    }
}

fn render_message_text_for_openai(parts: &[MessagePart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.clone()),
            MessagePart::ToolResult { result } => {
                Some(stringify_openai_message_value(&result.output))
            }
            MessagePart::Refusal {
                text: Some(text), ..
            } => Some(text.clone()),
            MessagePart::Structured { value, .. } => Some(stringify_openai_message_value(value)),
            MessagePart::Reasoning { .. }
            | MessagePart::ToolCall { .. }
            | MessagePart::Refusal { text: None, .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn stringify_openai_message_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

fn tool_spec_to_openai(spec: &ToolSpec) -> OpenAiTool {
    OpenAiTool {
        kind: "function",
        function: OpenAiFunctionSpec {
            name: spec.name.clone(),
            description: spec.description.clone(),
            parameters: spec.parameters_schema.clone(),
        },
    }
}

pub(super) fn supports_structured_output(provider_id: &str) -> bool {
    provider_id == "openai"
}

fn structured_output_response_format(
    provider_id: &str,
    structured_output: Option<&StructuredOutputSpec>,
) -> Option<OpenAiResponseFormat> {
    if !supports_structured_output(provider_id) {
        return None;
    }

    structured_output.map(|structured_output| OpenAiResponseFormat {
        kind: "json_schema",
        json_schema: OpenAiJsonSchema {
            name: structured_output
                .schema_name
                .clone()
                .unwrap_or_else(|| "belltower_structured_output".to_owned()),
            schema: structured_output.schema.clone(),
            strict: true,
        },
    })
}
