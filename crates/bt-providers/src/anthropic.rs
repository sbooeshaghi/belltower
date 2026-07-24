use crate::{SseEvent, SseParser};
use async_stream::try_stream;
use bt_core::{
    BelltowerError, CompletionChunk, CompletionDelta, CompletionRequest, CompletionSummary,
    ConnectionStatus, Message, MessagePart, ModelPricing, Result, Role, ThinkingConfig, TokenUsage,
    ToolSpec,
    traits::{BoxFuture, BoxStream, Provider},
};
use futures_util::StreamExt;
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use url::Url;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u64 = 4096;

#[derive(Clone)]
pub struct AnthropicProvider {
    provider_id: String,
    base_url: Url,
    api_key: Option<String>,
    http: Client,
}

impl AnthropicProvider {
    pub fn new(
        provider_id: impl Into<String>,
        base_url: Url,
        api_key: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            provider_id: provider_id.into(),
            base_url,
            api_key,
            http: crate::shared_http_client(),
        })
    }

    fn messages_url(&self) -> Result<Url> {
        self.endpoint("v1/messages")
    }

    fn models_url(&self) -> Result<Url> {
        self.endpoint("v1/models")
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        let mut base = self.base_url.clone();
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(base.join(path)?)
    }

    fn build_request(&self, request: CompletionRequest) -> AnthropicMessagesRequest {
        let CompletionRequest {
            model,
            system_prompt,
            messages,
            tools,
            structured_output: _,
            max_tokens,
            temperature,
            thinking,
            ..
        } = request;
        let (system, messages) = messages_to_anthropic(system_prompt, messages);

        AnthropicMessagesRequest {
            model,
            system,
            messages,
            max_tokens: max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            stream: true,
            temperature,
            tools: (!tools.is_empty()).then(|| tools.iter().map(tool_spec_to_anthropic).collect()),
            thinking: thinking.and_then(thinking_to_anthropic),
        }
    }
}

impl Provider for AnthropicProvider {
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
                Ok(ConnectionStatus::Degraded {
                    reason: format!("unexpected status {}", response.status()),
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
                return Err(BelltowerError::Provider(format!(
                    "model discovery failed with status {status}"
                )));
            }
            let payload = response.json::<Value>().await.map_err(reqwest_error)?;
            Ok(parse_anthropic_model_ids(payload))
        })
    }

    fn stream_completion(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<BoxStream<Result<CompletionChunk>>>> {
        Box::pin(async move {
            if request.structured_output.is_some() {
                return Err(BelltowerError::Unsupported(
                    "structured output is not supported for anthropic yet".to_owned(),
                ));
            }
            let body = self.build_request(request);
            let response = self
                .authorized(self.http.post(self.messages_url()?))
                .json(&body)
                .send()
                .await
                .map_err(reqwest_error)?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(BelltowerError::Provider(format!(
                    "streaming completion failed with status {status}: {body}"
                )));
            }

            let mut bytes_stream = response.bytes_stream();
            let stream = try_stream! {
                let mut parser = SseParser::new();
                let mut state = AnthropicStreamState::default();

                while let Some(chunk) = bytes_stream.next().await {
                    let chunk = chunk.map_err(reqwest_error)?;
                    for event in parser.push(chunk.as_ref()) {
                        for output in completion_chunks_from_sse_event(&event, &mut state)? {
                            yield output;
                        }
                    }
                }

                for output in state.flush_all() {
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

fn parse_anthropic_model_ids(payload: Value) -> Vec<String> {
    payload
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect()
}

impl AnthropicProvider {
    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let request = request.header("anthropic-version", ANTHROPIC_VERSION);
        if let Some(api_key) = &self.api_key {
            request.header("x-api-key", api_key)
        } else {
            request
        }
    }
}

fn reqwest_error(error: reqwest::Error) -> BelltowerError {
    BelltowerError::Provider(error.to_string())
}

#[derive(Debug, Serialize)]
struct AnthropicMessagesRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    max_tokens: u64,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<Value>,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Serialize)]
struct AnthropicThinking {
    #[serde(rename = "type")]
    kind: &'static str,
    budget_tokens: u64,
}

fn messages_to_anthropic(
    system_prompt: Option<String>,
    messages: Vec<Message>,
) -> (Option<String>, Vec<AnthropicMessage>) {
    let mut system_parts = Vec::new();
    if let Some(system_prompt) = system_prompt.filter(|value| !value.trim().is_empty()) {
        system_parts.push(system_prompt);
    }

    let mut translated = Vec::<AnthropicMessage>::new();
    for message in messages {
        if message.role == Role::System {
            system_parts.push(render_system_message(&message.parts));
            continue;
        }

        let role = anthropic_role(&message.role);
        let content = message_to_anthropic_blocks(&message.parts);
        if content.is_empty() {
            continue;
        }

        if let Some(last) = translated.last_mut()
            && last.role == role
        {
            last.content.extend(content);
            continue;
        }

        translated.push(AnthropicMessage { role, content });
    }

    let system = (!system_parts.is_empty()).then(|| system_parts.join("\n\n"));
    (system, translated)
}

fn render_system_message(parts: &[MessagePart]) -> String {
    parts
        .iter()
        .filter_map(render_message_part_as_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn anthropic_role(role: &Role) -> String {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "user",
        Role::System => "user",
    }
    .to_owned()
}

fn message_to_anthropic_blocks(parts: &[MessagePart]) -> Vec<Value> {
    parts
        .iter()
        .flat_map(message_part_to_anthropic_blocks)
        .collect()
}

fn message_part_to_anthropic_blocks(part: &MessagePart) -> Vec<Value> {
    match part {
        MessagePart::Text { text } => vec![json!({
            "type": "text",
            "text": text,
        })],
        MessagePart::ToolCall { call } => vec![json!({
            "type": "tool_use",
            "id": call.call_id,
            "name": call.tool_name,
            "input": call.arguments,
        })],
        MessagePart::ToolResult { result } => vec![json!({
            "type": "tool_result",
            "tool_use_id": result.call_id,
            "is_error": result.is_error,
            "content": render_value_as_text(&result.output),
        })],
        MessagePart::Refusal {
            text: Some(text), ..
        } => vec![json!({
            "type": "text",
            "text": text,
        })],
        MessagePart::Structured { value, .. } => vec![json!({
            "type": "text",
            "text": render_value_as_text(value),
        })],
        MessagePart::Reasoning { .. } | MessagePart::Refusal { text: None, .. } => Vec::new(),
    }
}

fn render_message_part_as_text(part: &MessagePart) -> Option<String> {
    match part {
        MessagePart::Text { text } => Some(text.clone()),
        MessagePart::ToolCall { call } => Some(format!(
            "Tool call {}({})",
            call.tool_name,
            render_value_as_text(&call.arguments)
        )),
        MessagePart::ToolResult { result } => Some(render_value_as_text(&result.output)),
        MessagePart::Refusal {
            text: Some(text), ..
        } => Some(text.clone()),
        MessagePart::Structured { value, .. } => Some(render_value_as_text(value)),
        MessagePart::Reasoning { .. } | MessagePart::Refusal { text: None, .. } => None,
    }
}

fn render_value_as_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
    }
}

fn tool_spec_to_anthropic(spec: &ToolSpec) -> AnthropicTool {
    AnthropicTool {
        name: spec.name.clone(),
        description: spec.description.clone(),
        input_schema: spec.parameters_schema.clone(),
    }
}

fn thinking_to_anthropic(thinking: ThinkingConfig) -> Option<AnthropicThinking> {
    if !thinking.enabled {
        return None;
    }

    Some(AnthropicThinking {
        kind: "enabled",
        budget_tokens: thinking.budget_tokens.unwrap_or(1_024),
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
struct AnthropicToolUseAccumulator {
    id: Option<String>,
    name: Option<String>,
    source_raw: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct AnthropicStreamState {
    tool_uses: BTreeMap<usize, AnthropicToolUseAccumulator>,
    prompt_tokens: u64,
    completion_tokens: u64,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
}

impl AnthropicStreamState {
    fn flush_all(&mut self) -> Vec<CompletionChunk> {
        let indices = self.tool_uses.keys().copied().collect::<Vec<_>>();
        indices
            .into_iter()
            .filter_map(|index| {
                self.finish_tool_use(index)
                    .map(|(delta, source_raw)| CompletionChunk {
                        llm_call_ordinal: None,
                        deltas: vec![delta],
                        usage: None,
                        raw: source_raw,
                    })
            })
            .collect()
    }

    fn start_tool_use(
        &mut self,
        index: usize,
        block: &Value,
        raw_json: &Value,
    ) -> Option<CompletionDelta> {
        let accumulator = self.tool_uses.entry(index).or_default();
        accumulator.source_raw = Some(raw_json.clone());
        accumulator.id = block.get("id").and_then(Value::as_str).map(str::to_owned);
        accumulator.name = block.get("name").and_then(Value::as_str).map(str::to_owned);
        let (Some(call_id), Some(tool_name)) = (&accumulator.id, &accumulator.name) else {
            return None;
        };
        Some(CompletionDelta::OpenToolCall {
            call_id: call_id.clone(),
            tool_name: tool_name.clone(),
            arguments: block.get("input").cloned(),
        })
    }

    fn push_tool_input(
        &mut self,
        index: usize,
        partial_json: &str,
        raw_json: &Value,
    ) -> Option<CompletionDelta> {
        let accumulator = self.tool_uses.entry(index).or_default();
        accumulator.source_raw = Some(raw_json.clone());
        let call_id = accumulator.id.clone()?;
        Some(CompletionDelta::AppendToolCallArguments {
            call_id,
            partial_json: partial_json.to_owned(),
        })
    }

    fn finish_tool_use(&mut self, index: usize) -> Option<(CompletionDelta, Option<Value>)> {
        let accumulator = self.tool_uses.remove(&index)?;
        let id = accumulator.id?;
        Some((
            CompletionDelta::CloseToolCall { call_id: id },
            accumulator.source_raw,
        ))
    }

    fn update_usage(&mut self, usage: &Value) {
        if let Some(prompt_tokens) = usage.get("input_tokens").and_then(Value::as_u64) {
            self.prompt_tokens = prompt_tokens;
        }
        if let Some(completion_tokens) = usage.get("output_tokens").and_then(Value::as_u64) {
            self.completion_tokens = completion_tokens;
        }
        if let Some(cache_read_tokens) =
            usage.get("cache_read_input_tokens").and_then(Value::as_u64)
        {
            self.cache_read_tokens = Some(cache_read_tokens);
        }
        if let Some(cache_write_tokens) = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
        {
            self.cache_write_tokens = Some(cache_write_tokens);
        }
        if let Some(reasoning_tokens) = usage
            .get("output_tokens_details")
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64)
        {
            self.reasoning_tokens = Some(reasoning_tokens);
        }
    }

    fn usage_snapshot(&self) -> Option<TokenUsage> {
        let total_tokens = self.prompt_tokens + self.completion_tokens;
        if total_tokens == 0
            && self.cache_read_tokens.is_none()
            && self.cache_write_tokens.is_none()
            && self.reasoning_tokens.is_none()
        {
            return None;
        }

        Some(TokenUsage {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            reasoning_tokens: self.reasoning_tokens,
        })
    }
}

fn completion_chunks_from_sse_event(
    event: &SseEvent,
    state: &mut AnthropicStreamState,
) -> Result<Vec<CompletionChunk>> {
    let raw_json: Value = serde_json::from_str(&event.data)?;
    let event_type = raw_json
        .get("type")
        .and_then(Value::as_str)
        .or(event.event.as_deref())
        .unwrap_or_default();

    let mut chunks = Vec::new();
    match event_type {
        "message_start" => {
            if let Some(usage) = raw_json
                .get("message")
                .and_then(|message| message.get("usage"))
            {
                state.update_usage(usage);
            }
        }
        "message_delta" => {
            if let Some(usage) = raw_json.get("usage") {
                state.update_usage(usage);
            }
            if let Some("refusal") = raw_json
                .get("delta")
                .and_then(|delta| delta.get("stop_reason"))
                .and_then(Value::as_str)
            {
                chunks.push(CompletionChunk {
                    llm_call_ordinal: None,
                    deltas: vec![CompletionDelta::AppendRefusal {
                        text: None,
                        provider_reason: Some("refusal".to_owned()),
                        opaque_metadata: raw_json.get("delta").cloned(),
                    }],
                    usage: None,
                    raw: None,
                });
            }
        }
        "content_block_start" => {
            let index = raw_json
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize;
            if let Some(block) = raw_json.get("content_block") {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            chunks.push(CompletionChunk {
                                llm_call_ordinal: None,
                                deltas: vec![CompletionDelta::AppendText {
                                    text: text.to_owned(),
                                }],
                                usage: None,
                                raw: None,
                            });
                        }
                    }
                    Some("thinking") => {
                        if let Some(text) = block.get("thinking").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            chunks.push(CompletionChunk {
                                llm_call_ordinal: None,
                                deltas: vec![CompletionDelta::AppendReasoning {
                                    text: Some(text.to_owned()),
                                    redacted: false,
                                    opaque_replay: None,
                                }],
                                usage: None,
                                raw: None,
                            });
                        }
                    }
                    Some("redacted_thinking") => {
                        chunks.push(CompletionChunk {
                            llm_call_ordinal: None,
                            deltas: vec![CompletionDelta::AppendReasoning {
                                text: None,
                                redacted: true,
                                opaque_replay: None,
                            }],
                            usage: None,
                            raw: None,
                        });
                    }
                    Some("tool_use") => {
                        if let Some(delta) = state.start_tool_use(index, block, &raw_json) {
                            chunks.push(CompletionChunk {
                                llm_call_ordinal: None,
                                deltas: vec![delta],
                                usage: None,
                                raw: None,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_delta" => {
            let index = raw_json
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize;
            if let Some(delta) = raw_json.get("delta") {
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            chunks.push(CompletionChunk {
                                llm_call_ordinal: None,
                                deltas: vec![CompletionDelta::AppendText {
                                    text: text.to_owned(),
                                }],
                                usage: None,
                                raw: None,
                            });
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                            chunks.push(CompletionChunk {
                                llm_call_ordinal: None,
                                deltas: vec![CompletionDelta::AppendReasoning {
                                    text: Some(text.to_owned()),
                                    redacted: false,
                                    opaque_replay: None,
                                }],
                                usage: None,
                                raw: None,
                            });
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(partial_json) =
                            delta.get("partial_json").and_then(Value::as_str)
                            && let Some(delta) =
                                state.push_tool_input(index, partial_json, &raw_json)
                        {
                            chunks.push(CompletionChunk {
                                llm_call_ordinal: None,
                                deltas: vec![delta],
                                usage: None,
                                raw: None,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            let index = raw_json
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize;
            if let Some((chunk, _source_raw)) = state.finish_tool_use(index) {
                chunks.push(CompletionChunk {
                    llm_call_ordinal: None,
                    deltas: vec![chunk],
                    usage: None,
                    raw: None,
                });
            }
        }
        _ => {}
    }

    if let Some(usage) = state.usage_snapshot().filter(|_| {
        matches!(
            event_type,
            "message_start" | "message_delta" | "message_stop"
        )
    }) {
        chunks.push(CompletionChunk {
            llm_call_ordinal: None,
            deltas: Vec::new(),
            usage: Some(usage),
            raw: None,
        });
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

fn attach_raw_to_all(mut chunks: Vec<CompletionChunk>, raw: Value) -> Vec<CompletionChunk> {
    for chunk in &mut chunks {
        if chunk.raw.is_none() {
            chunk.raw = Some(raw.clone());
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::{AnthropicProvider, completion_chunks_from_sse_event, messages_to_anthropic};
    use crate::SseEvent;
    use axum::{
        Json, Router,
        extract::State,
        response::sse::{Event, Sse},
        routing::{get, post},
    };
    use bt_core::{
        CompletionDelta, CompletionRequest, ConnectionId, Message, MessagePart, Role,
        ThinkingConfig, ToolCall, ToolDisplayGroup, ToolInterruptBehavior, ToolMetadata,
        ToolResultEnvelope, ToolRiskClass, ToolSpec, traits::Provider,
    };
    use futures_util::StreamExt;
    use serde_json::{Value, json};
    use std::{
        convert::Infallible,
        sync::{Arc, Mutex},
    };
    use tokio::net::TcpListener;
    use url::Url;

    #[test]
    fn request_translation_merges_roles_and_maps_tools() {
        let (system, messages) = messages_to_anthropic(
            Some("system prompt".to_owned()),
            vec![
                Message::text(Role::User, "hello"),
                Message {
                    message_id: bt_core::MessageId::new(),
                    role: Role::Assistant,
                    parts: vec![MessagePart::ToolCall {
                        call: ToolCall {
                            tool_name: "read".to_owned(),
                            call_id: "call-1".to_owned(),
                            arguments: json!({ "path": "src/lib.rs" }),
                        },
                    }],
                    created_at: time::OffsetDateTime::now_utc(),
                },
                Message {
                    message_id: bt_core::MessageId::new(),
                    role: Role::Tool,
                    parts: vec![MessagePart::ToolResult {
                        result: ToolResultEnvelope {
                            call_id: bt_core::ToolCallId::new("call-1"),
                            tool_name: "read".to_owned(),
                            is_error: false,
                            output: json!({ "content": "fn main() {}" }),
                            duration_ms: Some(1),
                        },
                    }],
                    created_at: time::OffsetDateTime::now_utc(),
                },
            ],
        );

        assert_eq!(system.as_deref(), Some("system prompt"));
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content[0]["type"], "text");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content[0]["type"], "tool_use");
        assert_eq!(messages[2].role, "user");
        assert_eq!(messages[2].content[0]["type"], "tool_result");
        assert_eq!(messages[2].content[0]["tool_use_id"], "call-1");
    }

    #[test]
    fn response_translation_attaches_raw_payload_and_flushes_tool_use() {
        let mut state = super::AnthropicStreamState::default();
        let start = completion_chunks_from_sse_event(
            &SseEvent {
                event: Some("content_block_start".to_owned()),
                data: json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "read",
                        "input": {}
                    }
                })
                .to_string(),
            },
            &mut state,
        )
        .expect("start should parse");
        assert_eq!(start.len(), 1);
        assert!(start[0].raw.is_some());

        let delta = completion_chunks_from_sse_event(
            &SseEvent {
                event: Some("content_block_delta".to_owned()),
                data: json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": "{\"path\":\"src/lib.rs\"}"
                    }
                })
                .to_string(),
            },
            &mut state,
        )
        .expect("delta should parse");
        assert_eq!(delta.len(), 1);
        assert!(delta[0].raw.is_some());

        let stop = completion_chunks_from_sse_event(
            &SseEvent {
                event: Some("content_block_stop".to_owned()),
                data: json!({
                    "type": "content_block_stop",
                    "index": 0
                })
                .to_string(),
            },
            &mut state,
        )
        .expect("stop should parse");
        assert!(matches!(
            stop[0].deltas.as_slice(),
            [CompletionDelta::CloseToolCall { call_id }] if call_id == "toolu_1"
        ));
        assert!(stop[0].raw.is_some());
    }

    #[test]
    fn response_translation_carries_raw_payload_through_deferred_tool_flush() {
        let mut state = super::AnthropicStreamState::default();
        completion_chunks_from_sse_event(
            &SseEvent {
                event: Some("content_block_start".to_owned()),
                data: json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "read",
                        "input": {}
                    }
                })
                .to_string(),
            },
            &mut state,
        )
        .expect("start should parse");

        let flushed = state.flush_all();
        assert_eq!(flushed.len(), 1);
        assert!(matches!(
            flushed[0].deltas.as_slice(),
            [CompletionDelta::CloseToolCall { call_id }] if call_id == "toolu_1"
        ));
        assert!(flushed[0].raw.is_some());
    }

    #[test]
    fn response_translation_tracks_usage_snapshots() {
        let mut state = super::AnthropicStreamState::default();
        let start = completion_chunks_from_sse_event(
            &SseEvent {
                event: Some("message_start".to_owned()),
                data: json!({
                    "type": "message_start",
                    "message": {
                        "usage": {
                            "input_tokens": 14,
                            "cache_read_input_tokens": 4
                        }
                    }
                })
                .to_string(),
            },
            &mut state,
        )
        .expect("start should parse");
        let start_usage = start[0].usage.as_ref().expect("usage chunk");
        assert_eq!(start_usage.prompt_tokens, 14);
        assert_eq!(start_usage.cache_read_tokens, Some(4));

        let delta = completion_chunks_from_sse_event(
            &SseEvent {
                event: Some("message_delta".to_owned()),
                data: json!({
                    "type": "message_delta",
                    "usage": {
                        "output_tokens": 9,
                        "output_tokens_details": {
                            "reasoning_tokens": 7
                        }
                    }
                })
                .to_string(),
            },
            &mut state,
        )
        .expect("delta should parse");
        let usage = delta[0].usage.as_ref().expect("usage chunk");
        assert_eq!(usage.prompt_tokens, 14);
        assert_eq!(usage.completion_tokens, 9);
        assert_eq!(usage.total_tokens, 23);
        assert_eq!(usage.reasoning_tokens, Some(7));
    }

    #[test]
    fn response_translation_emits_refusal_delta_from_stop_reason() {
        let mut state = super::AnthropicStreamState::default();
        let chunks = completion_chunks_from_sse_event(
            &SseEvent {
                event: Some("message_delta".to_owned()),
                data: json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": "refusal"
                    }
                })
                .to_string(),
            },
            &mut state,
        )
        .expect("refusal delta should parse");

        assert!(chunks.iter().any(|chunk| {
            chunk.deltas.iter().any(|delta| {
                matches!(
                    delta,
                    CompletionDelta::AppendRefusal {
                        text: None,
                        provider_reason: Some(reason),
                        ..
                    } if reason == "refusal"
                )
            })
        }));
        assert!(chunks.iter().all(|chunk| chunk.raw.is_some()));
    }

    #[tokio::test]
    async fn provider_sends_expected_headers_and_streams_sse() {
        #[derive(Clone, Default)]
        struct Capture {
            body: Arc<Mutex<Option<Value>>>,
            headers: Arc<Mutex<Vec<(String, String)>>>,
        }

        async fn models() -> Json<Value> {
            Json(json!({ "data": [] }))
        }

        async fn messages(
            State(capture): State<Capture>,
            headers: axum::http::HeaderMap,
            body: String,
        ) -> Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>> {
            *capture.body.lock().expect("body lock") =
                Some(serde_json::from_str(&body).expect("request body"));
            *capture.headers.lock().expect("headers lock") = headers
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_owned(),
                        value.to_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect();

            let events = vec![
                Event::default()
                    .event("content_block_delta")
                    .data("{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hel\"}}"),
                Event::default()
                    .event("content_block_delta")
                    .data("{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}"),
                Event::default()
                    .event("content_block_start")
                    .data("{\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\",\"input\":{}}}"),
                Event::default()
                    .event("content_block_delta")
                    .data("{\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"src\"}}"),
                Event::default()
                    .event("content_block_delta")
                    .data("{\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"/lib.rs\\\"}\"}}"),
                Event::default()
                    .event("content_block_stop")
                    .data("{\"type\":\"content_block_stop\",\"index\":1}"),
                Event::default()
                    .event("message_stop")
                    .data("{\"type\":\"message_stop\"}"),
            ];

            Sse::new(futures_util::stream::iter(
                events.into_iter().map(Ok::<_, Infallible>),
            ))
        }

        let capture = Capture::default();
        let app = Router::new()
            .route("/v1/models", get(models))
            .route("/v1/messages", post(messages))
            .with_state(capture.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let provider = AnthropicProvider::new(
            "anthropic",
            Url::parse(&format!("http://{addr}/")).expect("url"),
            Some("secret-key".to_owned()),
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("anthropic"),
            model: "claude-sonnet-4-20250514".to_owned(),
            system_prompt: Some("system prompt".to_owned()),
            messages: vec![Message::text(Role::User, "hello")],
            tools: vec![ToolSpec {
                name: "read".to_owned(),
                description: "Read a file".to_owned(),
                parameters_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }),
                metadata: ToolMetadata {
                    risk_class: ToolRiskClass::Safe,
                    is_read_only: true,
                    is_concurrency_safe: true,
                    interrupt_behavior: ToolInterruptBehavior::Immediate,
                    execution_mode: bt_core::ToolExecutionMode::Immediate,
                    should_defer: false,
                    catalogue_tags: vec!["file".to_owned(), "read".to_owned()],
                    display_group: ToolDisplayGroup::Codebase,
                },
            }],
            structured_output: None,
            max_tokens: Some(512),
            temperature: Some(0.1),
            thinking: Some(ThinkingConfig {
                enabled: true,
                effort: None,
                budget_tokens: Some(256),
                include_summaries: true,
            }),
        };

        let mut stream = provider
            .stream_completion(request)
            .await
            .expect("stream completion");
        let mut chunks = Vec::new();
        while let Some(chunk) = stream.next().await {
            chunks.push(chunk.expect("chunk"));
        }

        server.abort();

        let body = capture
            .body
            .lock()
            .expect("body lock")
            .clone()
            .expect("request body should be captured");
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["system"], "system prompt");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 256);

        let headers = capture.headers.lock().expect("headers lock").clone();
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "x-api-key" && value == "secret-key")
        );
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "anthropic-version" && value == "2023-06-01")
        );

        assert!(chunks.iter().any(|chunk| {
            chunk
                .deltas
                .iter()
                .any(|delta| matches!(delta, CompletionDelta::AppendText { text } if text == "hel"))
        }));
        assert!(chunks.iter().any(|chunk| {
            chunk
                .deltas
                .iter()
                .any(|delta| matches!(delta, CompletionDelta::AppendText { text } if text == "lo"))
        }));
        assert!(chunks.iter().any(|chunk| chunk.raw.is_some()));
        assert!(
            chunks
                .iter()
                .flat_map(|chunk| chunk.deltas.iter())
                .any(|delta| {
                    matches!(
                        delta,
                        CompletionDelta::OpenToolCall {
                            tool_name,
                            call_id,
                            ..
                        } if tool_name == "read" && call_id == "toolu_1"
                    )
                })
        );
        assert!(
            chunks
                .iter()
                .flat_map(|chunk| chunk.deltas.iter())
                .any(|delta| {
                    matches!(
                        delta,
                        CompletionDelta::AppendToolCallArguments { partial_json, .. }
                            if partial_json.contains("src")
                    )
                })
        );
        assert!(chunks.iter().flat_map(|chunk| chunk.deltas.iter()).any(|delta| {
            matches!(delta, CompletionDelta::CloseToolCall { call_id } if call_id == "toolu_1")
        }));
    }
}
