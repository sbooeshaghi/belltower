use crate::SseParser;
use async_stream::try_stream;
use bt_core::{
    BelltowerError, CompletionChunk, CompletionDelta, CompletionRequest, CompletionSummary,
    ConnectionStatus, CredentialMetadata, Message, MessagePart, ModelPricing, Result, Role,
    ThinkingConfig, ThinkingEffort, TokenUsage, ToolResultEnvelope, ToolSpec,
    traits::{BoxFuture, BoxStream, Provider},
};
use futures_util::StreamExt;
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use url::Url;

// ChatGPT's Codex backend gates model visibility on a Codex compatibility
// version. That is a backend contract and must not follow Belltower's package
// version, which would hide newer models from discovery.
const CHATGPT_CODEX_CLIENT_VERSION: &str = "0.124.0";
const DEFAULT_CHATGPT_INSTRUCTIONS: &str = "You are Belltower.";

#[derive(Clone)]
pub struct OpenAiChatGptProvider {
    provider_id: String,
    base_url: Url,
    access_token: String,
    account_id: String,
    http: Client,
}

impl OpenAiChatGptProvider {
    pub fn new(
        provider_id: impl Into<String>,
        base_url: Url,
        access_token: impl Into<String>,
        account_id: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            provider_id: provider_id.into(),
            base_url,
            access_token: access_token.into(),
            account_id: account_id.into(),
            http: Client::builder().build().map_err(reqwest_error)?,
        })
    }

    fn responses_url(&self) -> Result<Url> {
        self.endpoint("codex/responses")
    }

    fn models_url(&self) -> Result<Url> {
        let mut url = self.endpoint("codex/models")?;
        url.query_pairs_mut()
            .append_pair("client_version", CHATGPT_CODEX_CLIENT_VERSION);
        Ok(url)
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        let mut base = self.base_url.clone();
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(base.join(path)?)
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .bearer_auth(&self.access_token)
            .header("chatgpt-account-id", &self.account_id)
    }

    fn build_request(&self, request: CompletionRequest) -> ChatGptResponsesRequest {
        let instructions = request_instructions(&request);
        let input = request_input_items(&request);
        let reasoning = chatgpt_reasoning(request.thinking.as_ref());
        ChatGptResponsesRequest {
            model: request.model,
            instructions,
            input,
            stream: true,
            store: false,
            reasoning,
            tools: (!request.tools.is_empty()).then(|| {
                request
                    .tools
                    .iter()
                    .map(tool_spec_to_chatgpt)
                    .collect::<Vec<_>>()
            }),
            tool_choice: (!request.tools.is_empty()).then_some("auto".to_owned()),
            parallel_tool_calls: (!request.tools.is_empty()).then_some(true),
            temperature: request.temperature,
        }
    }
}

impl Provider for OpenAiChatGptProvider {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn validate(&self) -> BoxFuture<'_, Result<ConnectionStatus>> {
        Box::pin(async move {
            let response = self
                .authorized(self.http.get(self.models_url()?))
                .send()
                .await
                .map_err(reqwest_error)?;
            if response.status().is_success() {
                Ok(ConnectionStatus::Healthy)
            } else {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                Ok(ConnectionStatus::Degraded {
                    reason: format!("unexpected status {status}: {}", compact_body(&body)),
                })
            }
        })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(async move {
            let response = self
                .authorized(self.http.get(self.models_url()?))
                .send()
                .await
                .map_err(reqwest_error)?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(BelltowerError::Provider(format!(
                    "model discovery failed with status {status}: {}",
                    compact_body(&body)
                )));
            }
            let payload = response.json::<Value>().await.map_err(reqwest_error)?;
            Ok(parse_chatgpt_model_ids(&payload))
        })
    }

    fn stream_completion(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<BoxStream<Result<CompletionChunk>>>> {
        Box::pin(async move {
            if request.structured_output.is_some() {
                return Err(BelltowerError::Unsupported(
                    "structured output is not supported for provider `openai-chatgpt` yet"
                        .to_owned(),
                ));
            }

            let body = self.build_request(request);
            let response = self
                .authorized(self.http.post(self.responses_url()?))
                .json(&body)
                .send()
                .await
                .map_err(reqwest_error)?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(BelltowerError::Provider(format!(
                    "streaming completion failed with status {status}: {}",
                    compact_body(&body)
                )));
            }

            let mut bytes_stream = response.bytes_stream();
            let stream = try_stream! {
                let mut parser = SseParser::new();
                let mut tool_calls = BTreeMap::new();
                while let Some(chunk) = bytes_stream.next().await {
                    let chunk = chunk.map_err(reqwest_error)?;
                    for event in parser.push(chunk.as_ref()) {
                        for output in completion_chunks_from_responses_event(&event.data, &mut tool_calls)? {
                            yield output;
                        }
                    }
                }

                for output in flush_tool_calls(&mut tool_calls) {
                    yield output;
                }
            };

            let stream: BoxStream<Result<CompletionChunk>> = Box::pin(stream);
            Ok(stream)
        })
    }

    fn pricing(&self, model: &str) -> Option<ModelPricing> {
        Some(ModelPricing {
            provider: self.provider_id.clone(),
            model_pattern: model.to_owned(),
            prompt_usd_per_million: 0.0,
            completion_usd_per_million: 0.0,
            cache_read_usd_per_million: None,
            cache_write_usd_per_million: None,
            reasoning_usd_per_million: None,
            unpriced_reason: None,
        })
    }

    fn completion_summary(&self, summary: CompletionSummary) -> Result<CompletionSummary> {
        Ok(summary)
    }
}

#[derive(Debug, Serialize)]
struct ChatGptResponsesRequest {
    model: String,
    instructions: String,
    input: Vec<Value>,
    stream: bool,
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ChatGptReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ChatGptTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct ChatGptReasoning {
    effort: ThinkingEffort,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'static str>,
}

fn chatgpt_reasoning(thinking: Option<&ThinkingConfig>) -> Option<ChatGptReasoning> {
    let thinking = thinking?;
    if !thinking.enabled {
        return None;
    }
    Some(ChatGptReasoning {
        effort: thinking.effort.unwrap_or(ThinkingEffort::High),
        summary: thinking.include_summaries.then_some("auto"),
    })
}

#[derive(Debug, Serialize)]
struct ChatGptTool {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    description: String,
    parameters: Value,
}

fn request_input_items(request: &CompletionRequest) -> Vec<Value> {
    let mut items = Vec::new();
    let mut assistant_message_index = 0usize;

    for message in &request.messages {
        match message.role {
            Role::System => {}
            Role::User => {
                let text = render_visible_message_text(&message.parts);
                if !text.trim().is_empty() {
                    items.push(message_input_text_item("user", &text));
                }
            }
            Role::Assistant => {
                extend_assistant_input_items(message, &mut items, &mut assistant_message_index);
            }
            Role::Tool => {
                extend_tool_result_items(message, &mut items);
            }
        }
    }

    items
}

fn request_instructions(request: &CompletionRequest) -> String {
    let mut sections = Vec::new();
    if let Some(system_prompt) = request
        .system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        sections.push(system_prompt.to_owned());
    }

    for message in &request.messages {
        if message.role != Role::System {
            continue;
        }
        let text = render_visible_message_text(&message.parts);
        let text = text.trim();
        if !text.is_empty() {
            sections.push(text.to_owned());
        }
    }

    if sections.is_empty() {
        DEFAULT_CHATGPT_INSTRUCTIONS.to_owned()
    } else {
        sections.join("\n\n")
    }
}

fn message_input_text_item(role: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [
            {
                "type": "input_text",
                "text": text,
            }
        ]
    })
}

fn extend_assistant_input_items(
    message: &Message,
    items: &mut Vec<Value>,
    assistant_message_index: &mut usize,
) {
    let mut buffered_text = String::new();
    for part in &message.parts {
        match part {
            MessagePart::Text { text } => buffered_text.push_str(text),
            MessagePart::Refusal {
                text: Some(text), ..
            } => buffered_text.push_str(text),
            MessagePart::Structured { value, .. } => {
                buffered_text.push_str(&stringify_message_value(value));
            }
            MessagePart::ToolCall { call } => {
                flush_assistant_text(items, assistant_message_index, &mut buffered_text);
                items.push(json!({
                    "type": "function_call",
                    "id": function_item_id(&call.call_id),
                    "call_id": call.call_id,
                    "name": call.tool_name,
                    "arguments": stringify_message_value(&call.arguments),
                }));
            }
            MessagePart::ToolResult { result } => {
                flush_assistant_text(items, assistant_message_index, &mut buffered_text);
                items.push(function_call_output_item(result));
            }
            MessagePart::Reasoning { .. } | MessagePart::Refusal { text: None, .. } => {}
        }
    }

    flush_assistant_text(items, assistant_message_index, &mut buffered_text);
}

fn extend_tool_result_items(message: &Message, items: &mut Vec<Value>) {
    for part in &message.parts {
        if let MessagePart::ToolResult { result } = part {
            items.push(function_call_output_item(result));
        }
    }
}

fn flush_assistant_text(
    items: &mut Vec<Value>,
    assistant_message_index: &mut usize,
    buffered_text: &mut String,
) {
    if buffered_text.trim().is_empty() {
        buffered_text.clear();
        return;
    }

    let item = json!({
        "type": "message",
        "id": assistant_message_item_id(*assistant_message_index),
        "role": "assistant",
        "status": "completed",
        "content": [
            {
                "type": "output_text",
                "text": buffered_text,
                "annotations": [],
            }
        ]
    });
    items.push(item);
    *assistant_message_index += 1;
    buffered_text.clear();
}

fn function_call_output_item(result: &ToolResultEnvelope) -> Value {
    json!({
        "type": "function_call_output",
        "call_id": result.call_id,
        "output": stringify_message_value(&result.output),
    })
}

fn assistant_message_item_id(index: usize) -> String {
    format!("msg_{}", index + 1)
}

fn function_item_id(call_id: &str) -> String {
    let mut normalized = call_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if normalized.is_empty() {
        normalized = "generated".to_owned();
    }
    if !normalized.starts_with("fc_") {
        normalized = format!("fc_{normalized}");
    }
    if normalized.len() > 64 {
        normalized.truncate(64);
    }
    normalized
}

fn render_visible_message_text(parts: &[MessagePart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.clone()),
            MessagePart::Refusal {
                text: Some(text), ..
            } => Some(text.clone()),
            MessagePart::Structured { value, .. } => Some(stringify_message_value(value)),
            MessagePart::ToolResult { result } => Some(stringify_message_value(&result.output)),
            MessagePart::Reasoning { .. }
            | MessagePart::ToolCall { .. }
            | MessagePart::Refusal { text: None, .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn stringify_message_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

fn tool_spec_to_chatgpt(spec: &ToolSpec) -> ChatGptTool {
    ChatGptTool {
        kind: "function",
        name: spec.name.clone(),
        description: spec.description.clone(),
        parameters: spec.parameters_schema.clone(),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ToolCallAccumulator {
    call_id: Option<String>,
    name: Option<String>,
    opened: bool,
    arguments: String,
    source_raw: Option<Value>,
}

fn completion_chunks_from_responses_event(
    data: &str,
    tool_calls: &mut BTreeMap<String, ToolCallAccumulator>,
) -> Result<Vec<CompletionChunk>> {
    if data.trim() == "[DONE]" {
        return Ok(attach_raw_to_all(
            flush_tool_calls(tool_calls),
            Value::String("[DONE]".to_owned()),
        ));
    }

    let raw_json: Value = serde_json::from_str(data)?;
    let kind = raw_json
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut chunks = Vec::new();

    match kind {
        "response.output_text.delta" => {
            if let Some(delta) = raw_json.get("delta").and_then(Value::as_str)
                && !delta.is_empty()
            {
                chunks.push(text_chunk(CompletionDelta::AppendText {
                    text: delta.to_owned(),
                }));
            }
        }
        "response.refusal.delta" => {
            if let Some(delta) = raw_json.get("delta").and_then(Value::as_str)
                && !delta.is_empty()
            {
                chunks.push(text_chunk(CompletionDelta::AppendRefusal {
                    text: Some(delta.to_owned()),
                    provider_reason: Some("refusal".to_owned()),
                    opaque_metadata: None,
                }));
            }
        }
        "response.reasoning_summary_text.delta" => {
            if let Some(delta) = raw_json.get("delta").and_then(Value::as_str)
                && !delta.is_empty()
            {
                chunks.push(text_chunk(CompletionDelta::AppendReasoning {
                    text: Some(delta.to_owned()),
                    redacted: false,
                    opaque_replay: None,
                }));
            }
        }
        "response.output_item.added" => {
            if let Some(item) = raw_json.get("item") {
                chunks.extend(apply_output_item_added(tool_calls, item, &raw_json));
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(item_id) = raw_json.get("item_id").and_then(Value::as_str)
                && let Some(delta) = raw_json.get("delta").and_then(Value::as_str)
                && !delta.is_empty()
            {
                chunks.extend(apply_function_call_argument_delta(
                    tool_calls, item_id, delta, &raw_json,
                ));
            }
        }
        "response.function_call_arguments.done" => {
            if let Some(item_id) = raw_json.get("item_id").and_then(Value::as_str)
                && let Some(arguments) = raw_json.get("arguments").and_then(Value::as_str)
            {
                chunks.extend(reconcile_function_call_arguments(
                    tool_calls, item_id, arguments, &raw_json,
                ));
            }
        }
        "response.output_item.done" => {
            if let Some(item) = raw_json.get("item") {
                chunks.extend(apply_output_item_done(tool_calls, item, &raw_json));
            }
        }
        "response.completed" => {
            let usage = raw_json
                .get("response")
                .and_then(|response| response.get("usage"))
                .and_then(token_usage_from_chatgpt);
            if let Some(usage) = usage {
                chunks.push(CompletionChunk {
                    llm_call_ordinal: None,
                    deltas: Vec::new(),
                    usage: Some(usage),
                    raw: None,
                });
            }
        }
        "response.failed" | "error" => {
            return Err(BelltowerError::Provider(format!(
                "ChatGPT Responses stream failed: {}",
                compact_body(data)
            )));
        }
        _ => {}
    }

    if chunks.is_empty() {
        chunks.push(CompletionChunk {
            llm_call_ordinal: None,
            deltas: Vec::new(),
            usage: None,
            raw: None,
        });
    }

    Ok(attach_raw_to_all(chunks, raw_json))
}

fn apply_output_item_added(
    tool_calls: &mut BTreeMap<String, ToolCallAccumulator>,
    item: &Value,
    raw_json: &Value,
) -> Vec<CompletionChunk> {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return Vec::new();
    }

    let Some(item_id) = item
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            item.get("call_id")
                .and_then(Value::as_str)
                .map(function_item_id)
        })
    else {
        return Vec::new();
    };

    let accumulator = tool_calls.entry(item_id).or_default();
    accumulator.source_raw = Some(raw_json.clone());
    if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
        accumulator.call_id = Some(call_id.to_owned());
    }
    if let Some(name) = item.get("name").and_then(Value::as_str) {
        accumulator.name = Some(name.to_owned());
    }
    if let Some(arguments) = item.get("arguments").and_then(Value::as_str)
        && !arguments.is_empty()
    {
        accumulator.arguments.push_str(arguments);
    }

    semantic_tool_call_chunks(open_tool_call_if_ready(accumulator))
}

fn apply_function_call_argument_delta(
    tool_calls: &mut BTreeMap<String, ToolCallAccumulator>,
    item_id: &str,
    delta: &str,
    raw_json: &Value,
) -> Vec<CompletionChunk> {
    let Some(accumulator) = tool_calls.get_mut(item_id) else {
        return Vec::new();
    };
    accumulator.source_raw = Some(raw_json.clone());
    accumulator.arguments.push_str(delta);

    let mut deltas = open_tool_call_if_ready(accumulator);
    if accumulator.opened
        && let Some(call_id) = accumulator.call_id.as_ref()
    {
        deltas.push(CompletionDelta::AppendToolCallArguments {
            call_id: call_id.clone(),
            partial_json: delta.to_owned(),
        });
    }
    semantic_tool_call_chunks(deltas)
}

fn reconcile_function_call_arguments(
    tool_calls: &mut BTreeMap<String, ToolCallAccumulator>,
    item_id: &str,
    arguments: &str,
    raw_json: &Value,
) -> Vec<CompletionChunk> {
    let Some(accumulator) = tool_calls.get_mut(item_id) else {
        return Vec::new();
    };
    accumulator.source_raw = Some(raw_json.clone());

    let mut deltas = open_tool_call_if_ready(accumulator);
    if arguments.starts_with(&accumulator.arguments) {
        let suffix = &arguments[accumulator.arguments.len()..];
        if !suffix.is_empty()
            && let Some(call_id) = accumulator.call_id.as_ref()
        {
            deltas.push(CompletionDelta::AppendToolCallArguments {
                call_id: call_id.clone(),
                partial_json: suffix.to_owned(),
            });
        }
    } else if arguments != accumulator.arguments
        && let Some(call_id) = accumulator.call_id.as_ref()
    {
        deltas.push(CompletionDelta::AppendToolCallArguments {
            call_id: call_id.clone(),
            partial_json: arguments.to_owned(),
        });
    }

    accumulator.arguments.clear();
    accumulator.arguments.push_str(arguments);
    semantic_tool_call_chunks(deltas)
}

fn apply_output_item_done(
    tool_calls: &mut BTreeMap<String, ToolCallAccumulator>,
    item: &Value,
    raw_json: &Value,
) -> Vec<CompletionChunk> {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return Vec::new();
    }

    let Some(item_id) = item
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            item.get("call_id")
                .and_then(Value::as_str)
                .map(function_item_id)
        })
    else {
        return Vec::new();
    };

    let mut deltas = if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
        reconcile_function_call_arguments(tool_calls, &item_id, arguments, raw_json)
            .into_iter()
            .flat_map(|chunk| chunk.deltas)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    if let Some(accumulator) = tool_calls.remove(&item_id) {
        let mut final_deltas = finalize_tool_call(accumulator);
        deltas.append(&mut final_deltas);
    }

    semantic_tool_call_chunks(deltas)
}

fn open_tool_call_if_ready(accumulator: &mut ToolCallAccumulator) -> Vec<CompletionDelta> {
    if accumulator.opened {
        return Vec::new();
    }
    let (Some(call_id), Some(tool_name)) = (&accumulator.call_id, &accumulator.name) else {
        return Vec::new();
    };
    accumulator.opened = true;
    let mut deltas = vec![CompletionDelta::OpenToolCall {
        call_id: call_id.clone(),
        tool_name: tool_name.clone(),
        arguments: None,
    }];
    if !accumulator.arguments.is_empty() {
        deltas.push(CompletionDelta::AppendToolCallArguments {
            call_id: call_id.clone(),
            partial_json: accumulator.arguments.clone(),
        });
    }
    deltas
}

fn finalize_tool_call(mut accumulator: ToolCallAccumulator) -> Vec<CompletionDelta> {
    let Some(call_id) = accumulator.call_id.take() else {
        return Vec::new();
    };
    let Some(tool_name) = accumulator.name.take() else {
        return Vec::new();
    };
    let mut deltas = Vec::new();
    if !accumulator.opened {
        deltas.push(CompletionDelta::OpenToolCall {
            call_id: call_id.clone(),
            tool_name,
            arguments: None,
        });
        if !accumulator.arguments.is_empty() {
            deltas.push(CompletionDelta::AppendToolCallArguments {
                call_id: call_id.clone(),
                partial_json: accumulator.arguments,
            });
        }
    }
    deltas.push(CompletionDelta::CloseToolCall { call_id });
    deltas
}

fn flush_tool_calls(
    tool_calls: &mut BTreeMap<String, ToolCallAccumulator>,
) -> Vec<CompletionChunk> {
    let mut chunks = Vec::new();
    for accumulator in std::mem::take(tool_calls).into_values() {
        let source_raw = accumulator.source_raw.clone();
        let deltas = finalize_tool_call(accumulator);
        if !deltas.is_empty() {
            chunks.push(CompletionChunk {
                llm_call_ordinal: None,
                deltas,
                usage: None,
                raw: source_raw,
            });
        }
    }
    chunks
}

fn semantic_tool_call_chunks(deltas: Vec<CompletionDelta>) -> Vec<CompletionChunk> {
    if deltas.is_empty() {
        return Vec::new();
    }
    vec![CompletionChunk {
        llm_call_ordinal: None,
        deltas,
        usage: None,
        raw: None,
    }]
}

fn text_chunk(delta: CompletionDelta) -> CompletionChunk {
    CompletionChunk {
        llm_call_ordinal: None,
        deltas: vec![delta],
        usage: None,
        raw: None,
    }
}

fn attach_raw_to_all(mut chunks: Vec<CompletionChunk>, raw: Value) -> Vec<CompletionChunk> {
    for chunk in &mut chunks {
        if chunk.raw.is_none() {
            chunk.raw = Some(raw.clone());
        }
    }
    chunks
}

fn token_usage_from_chatgpt(value: &Value) -> Option<TokenUsage> {
    let prompt_tokens = value.get("input_tokens").and_then(Value::as_u64)?;
    let completion_tokens = value.get("output_tokens").and_then(Value::as_u64)?;
    let total_tokens = value
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(prompt_tokens + completion_tokens);
    let cache_read_tokens = value
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64);
    let reasoning_tokens = value
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64);

    Some(TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
        cache_read_tokens,
        cache_write_tokens: None,
        reasoning_tokens,
    })
}

fn parse_chatgpt_model_ids(payload: &Value) -> Vec<String> {
    payload
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let supported = model
                .get("supported_in_api")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if !supported {
                return None;
            }
            model
                .get("slug")
                .and_then(Value::as_str)
                .or_else(|| model.get("id").and_then(Value::as_str))
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn compact_body(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "empty response".to_owned()
    } else {
        compact
    }
}

fn reqwest_error(error: reqwest::Error) -> BelltowerError {
    BelltowerError::Provider(error.to_string())
}

pub fn chatgpt_account_id(metadata: &CredentialMetadata) -> Result<String> {
    metadata.account_id.clone().ok_or_else(|| {
        BelltowerError::Auth(
            "ChatGPT credentials are missing `chatgpt_account_id`; rerun `belltower login chatgpt`"
                .to_owned(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAiChatGptProvider, chatgpt_account_id, completion_chunks_from_responses_event,
        parse_chatgpt_model_ids, request_input_items, request_instructions,
    };
    use bt_core::{
        CompletionDelta, CompletionRequest, ConnectionId, CredentialMetadata, Message, MessagePart,
        Role, ThinkingConfig, ThinkingEffort, ToolCall, ToolCallId, ToolResultEnvelope,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use url::Url;

    #[test]
    fn request_translation_uses_responses_items() {
        let request = CompletionRequest {
            connection_id: ConnectionId::new("chatgpt"),
            model: "gpt-5.4".to_owned(),
            system_prompt: Some("stay grounded".to_owned()),
            messages: vec![
                Message::text(Role::User, "hello"),
                Message::new(
                    Role::Assistant,
                    vec![
                        MessagePart::Text {
                            text: "checking".to_owned(),
                        },
                        MessagePart::ToolCall {
                            call: ToolCall {
                                tool_name: "list".to_owned(),
                                call_id: "call-1".to_owned(),
                                arguments: json!({"path":"."}),
                            },
                        },
                    ],
                ),
                Message::from_part(
                    Role::Tool,
                    MessagePart::ToolResult {
                        result: ToolResultEnvelope {
                            call_id: ToolCallId::new("call-1"),
                            tool_name: "list".to_owned(),
                            is_error: false,
                            output: json!({"entries": 3}),
                            duration_ms: Some(10),
                        },
                    },
                ),
            ],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(512),
            temperature: Some(0.0),
            thinking: None,
        };

        let instructions = request_instructions(&request);
        let items = request_input_items(&request);
        assert_eq!(instructions, "stay grounded");
        assert_eq!(items[0]["type"], "message");
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[1]["type"], "message");
        assert_eq!(items[2]["type"], "function_call");
        assert_eq!(items[3]["type"], "function_call_output");
    }

    #[test]
    fn request_translation_sends_reasoning_effort() {
        let provider = OpenAiChatGptProvider::new(
            "chatgpt",
            Url::parse("https://chatgpt.com/backend-api").expect("url"),
            "token",
            "acct",
        )
        .expect("provider");
        let request = CompletionRequest {
            connection_id: ConnectionId::new("chatgpt"),
            model: "gpt-5.4-mini".to_owned(),
            system_prompt: None,
            messages: vec![Message::text(Role::User, "hello")],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(512),
            temperature: None,
            thinking: Some(ThinkingConfig {
                enabled: true,
                effort: Some(ThinkingEffort::XHigh),
                budget_tokens: None,
                include_summaries: true,
            }),
        };

        let translated = serde_json::to_value(provider.build_request(request))
            .expect("request should serialize");
        assert_eq!(translated["reasoning"]["effort"], "xhigh");
        assert_eq!(translated["reasoning"]["summary"], "auto");
    }

    #[test]
    fn request_translation_merges_system_messages_into_instructions() {
        let request = CompletionRequest {
            connection_id: ConnectionId::new("chatgpt"),
            model: "gpt-5.4".to_owned(),
            system_prompt: Some("base".to_owned()),
            messages: vec![
                Message::text(Role::System, "overlay"),
                Message::text(Role::User, "hello"),
            ],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(512),
            temperature: Some(0.0),
            thinking: None,
        };

        assert_eq!(request_instructions(&request), "base\n\noverlay");
        let items = request_input_items(&request);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "message");
        assert_eq!(items[0]["role"], "user");
    }

    #[test]
    fn model_discovery_uses_codex_slug() {
        let payload = json!({
            "models": [
                { "slug": "gpt-5.4", "supported_in_api": true },
                { "slug": "hidden-model", "supported_in_api": false }
            ]
        });

        assert_eq!(parse_chatgpt_model_ids(&payload), vec!["gpt-5.4"]);
    }

    #[test]
    fn models_url_uses_codex_compatibility_version() {
        let provider = OpenAiChatGptProvider::new(
            "chatgpt",
            Url::parse("https://chatgpt.com/backend-api").expect("url"),
            "token",
            "acct",
        )
        .expect("provider");

        let url = provider.models_url().expect("models url");
        assert_eq!(
            url.as_str(),
            "https://chatgpt.com/backend-api/codex/models?client_version=0.124.0"
        );
    }

    #[test]
    fn responses_stream_emits_text_and_tool_semantics() {
        let mut tool_calls = BTreeMap::new();
        let added = completion_chunks_from_responses_event(
            &json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "list",
                    "arguments": "{\"path\":\"."
                }
            })
            .to_string(),
            &mut tool_calls,
        )
        .expect("added");
        assert!(added.iter().any(|chunk| matches!(
            chunk.deltas.first(),
            Some(CompletionDelta::OpenToolCall { call_id, tool_name, .. })
                if call_id == "call_1" && tool_name == "list"
        )));
        assert!(added.iter().all(|chunk| chunk.raw.is_some()));

        let delta = completion_chunks_from_responses_event(
            &json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_1",
                "delta": "\"}"
            })
            .to_string(),
            &mut tool_calls,
        )
        .expect("delta");
        assert!(delta.iter().any(|chunk| matches!(
            chunk.deltas.last(),
            Some(CompletionDelta::AppendToolCallArguments { call_id, partial_json })
                if call_id == "call_1" && partial_json == "\"}"
        )));
        assert!(delta.iter().all(|chunk| chunk.raw.is_some()));

        let done = completion_chunks_from_responses_event(
            &json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "list",
                    "arguments": "{\"path\":\".\"}"
                }
            })
            .to_string(),
            &mut tool_calls,
        )
        .expect("done");
        assert!(done.iter().any(|chunk| matches!(
            chunk.deltas.last(),
            Some(CompletionDelta::CloseToolCall { call_id }) if call_id == "call_1"
        )));
        assert!(done.iter().all(|chunk| chunk.raw.is_some()));

        let text = completion_chunks_from_responses_event(
            &json!({
                "type": "response.output_text.delta",
                "delta": "Hello"
            })
            .to_string(),
            &mut tool_calls,
        )
        .expect("text");
        assert!(text.iter().any(|chunk| matches!(
            chunk.deltas.first(),
            Some(CompletionDelta::AppendText { text }) if text == "Hello"
        )));
        assert!(text.iter().all(|chunk| chunk.raw.is_some()));
    }

    #[test]
    fn provider_builds_with_account_metadata() {
        let provider = OpenAiChatGptProvider::new(
            "chatgpt",
            Url::parse("https://chatgpt.com/backend-api").expect("url"),
            "access-token",
            chatgpt_account_id(&CredentialMetadata {
                account_id: Some("acct_123".to_owned()),
                plan_type: None,
                workspace_id: None,
            })
            .expect("account id"),
        );
        assert!(provider.is_ok());
    }
}
