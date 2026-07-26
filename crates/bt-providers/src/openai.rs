mod request;

use self::request::{OpenAiChatRequest, build_chat_request, supports_structured_output};
use crate::SseParser;
use async_stream::try_stream;
use bt_core::{
    BelltowerError, CompletionChunk, CompletionDelta, CompletionRequest, CompletionSummary,
    ConnectionStatus, ModelPricing, Result, StructuredOutputSpec, TokenUsage,
    traits::{BoxFuture, BoxStream, Provider},
};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

#[derive(Clone)]
pub struct OpenAiCompatibleProvider {
    provider_id: String,
    base_url: Url,
    auth_token: Option<String>,
    http: Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        provider_id: impl Into<String>,
        base_url: Url,
        auth_token: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            provider_id: provider_id.into(),
            base_url,
            auth_token,
            http: crate::shared_http_client(),
        })
    }

    fn chat_completions_url(&self) -> Result<Url> {
        self.endpoint("chat/completions")
    }

    fn models_url(&self) -> Result<Url> {
        self.endpoint("models")
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        let mut base = self.base_url.clone();
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(base.join(path)?)
    }

    fn build_request(&self, request: CompletionRequest) -> OpenAiChatRequest {
        build_chat_request(&self.provider_id, request)
    }
}

impl Provider for OpenAiCompatibleProvider {
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
            Ok(parse_openai_model_ids(payload))
        })
    }

    fn stream_completion(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<BoxStream<Result<CompletionChunk>>>> {
        Box::pin(async move {
            if request.structured_output.is_some() && !supports_structured_output(&self.provider_id)
            {
                return Err(BelltowerError::Unsupported(format!(
                    "structured output is not supported for provider `{}`",
                    self.provider_id
                )));
            }
            if request.structured_output.is_some() && !request.tools.is_empty() {
                return Err(BelltowerError::Unsupported(
                    "structured output is not supported together with tool calling yet".to_owned(),
                ));
            }
            let mut structured_output = request
                .structured_output
                .clone()
                .map(StructuredOutputAccumulator::new);
            let body = self.build_request(request);
            let response = self
                .authorized(self.http.post(self.chat_completions_url()?))
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
                let mut tool_calls = Vec::new();
                while let Some(chunk) = bytes_stream.next().await {
                    let chunk = chunk.map_err(reqwest_error)?;
                    for event in parser.push(chunk.as_ref()) {
                        for output in completion_chunks_from_sse_event(&event.data, &mut tool_calls)? {
                            if let Some(structured_output) = structured_output.as_mut() {
                                if let Some(output) =
                                    capture_structured_output_chunk(output, structured_output)?
                                {
                                    yield output;
                                }
                            } else {
                                yield output;
                            }
                        }
                    }
                }

                for output in flush_tool_calls(&mut tool_calls) {
                    yield output;
                }

                if let Some(structured_output) = structured_output.take() {
                    let finished_output = structured_output.finish()?;
                    if let Some(output) = finished_output {
                        yield output;
                    }
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

fn parse_openai_model_ids(payload: Value) -> Vec<String> {
    payload
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect()
}

fn reqwest_error(error: reqwest::Error) -> BelltowerError {
    BelltowerError::Provider(error.to_string())
}

impl OpenAiCompatibleProvider {
    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(auth_token) = &self.auth_token {
            request.bearer_auth(auth_token)
        } else {
            request
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiStreamResponse {
    #[serde(default)]
    choices: Vec<OpenAiStreamChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiStreamChoice {
    #[serde(default)]
    delta: OpenAiStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct OpenAiStreamDelta {
    content: Option<String>,
    refusal: Option<String>,
    // Reasoning field names vary by backend: ollama's OpenAI-compat layer
    // streams `reasoning`, DeepSeek/vLLM stream `reasoning_content`.
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiToolCallFunctionDelta>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiToolCallFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
    #[serde(default)]
    completion_tokens_details: Option<OpenAiCompletionTokensDetails>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ToolCallAccumulator {
    id: Option<String>,
    name: Option<String>,
    opened: bool,
    buffered_arguments: Vec<String>,
    source_raw: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
struct StructuredOutputAccumulator {
    schema_name: Option<String>,
    buffered_text: String,
    saw_refusal: bool,
    raw_evidence: Vec<Value>,
}

impl StructuredOutputAccumulator {
    fn new(spec: StructuredOutputSpec) -> Self {
        Self {
            schema_name: spec.schema_name,
            buffered_text: String::new(),
            saw_refusal: false,
            raw_evidence: Vec::new(),
        }
    }

    fn finish(self) -> Result<Option<CompletionChunk>> {
        if self.saw_refusal || self.buffered_text.trim().is_empty() {
            return Ok(None);
        }

        let value = serde_json::from_str(&self.buffered_text).map_err(|error| {
            BelltowerError::Provider(format!("structured output did not parse as JSON: {error}"))
        })?;
        let source_raw = self.raw_evidence.last().cloned();

        Ok(Some(CompletionChunk {
            llm_call_ordinal: None,
            deltas: vec![CompletionDelta::SetStructuredOutput {
                schema_name: self.schema_name,
                value,
            }],
            usage: None,
            raw: source_raw,
        }))
    }
}

fn capture_structured_output_chunk(
    mut chunk: CompletionChunk,
    structured_output: &mut StructuredOutputAccumulator,
) -> Result<Option<CompletionChunk>> {
    let mut filtered_deltas = Vec::new();
    let mut captured_text = false;
    for delta in chunk.deltas {
        match delta {
            CompletionDelta::AppendText { text } => {
                structured_output.buffered_text.push_str(&text);
                captured_text = true;
            }
            CompletionDelta::AppendRefusal { .. } => {
                structured_output.saw_refusal = true;
                filtered_deltas.push(delta);
            }
            other => filtered_deltas.push(other),
        }
    }
    if captured_text && let Some(raw) = chunk.raw.as_ref() {
        structured_output.raw_evidence.push(raw.clone());
    }
    chunk.deltas = filtered_deltas;
    if chunk.deltas.is_empty() && chunk.usage.is_none() && chunk.raw.is_none() {
        return Ok(None);
    }
    Ok(Some(chunk))
}

fn completion_chunks_from_sse_event(
    data: &str,
    tool_calls: &mut Vec<ToolCallAccumulator>,
) -> Result<Vec<CompletionChunk>> {
    if data.trim() == "[DONE]" {
        return Ok(attach_raw_to_all(
            flush_tool_calls(tool_calls),
            Value::String("[DONE]".to_owned()),
        ));
    }

    let raw_json: Value = serde_json::from_str(data)?;
    let parsed: OpenAiStreamResponse = serde_json::from_value(raw_json.clone())?;
    let usage = parsed.usage.as_ref().map(token_usage_from_openai);
    let mut chunks = Vec::new();
    let mut should_flush_tool_calls = false;

    for choice in parsed.choices {
        let reasoning = choice
            .delta
            .reasoning
            .into_iter()
            .chain(choice.delta.reasoning_content)
            .find(|value| !value.is_empty());
        if let Some(reasoning) = reasoning {
            chunks.push(CompletionChunk {
                llm_call_ordinal: None,
                deltas: vec![CompletionDelta::AppendReasoning {
                    text: Some(reasoning),
                    redacted: false,
                    opaque_replay: None,
                }],
                usage: None,
                raw: None,
            });
        }

        if let Some(content) = choice.delta.content.filter(|value| !value.is_empty()) {
            chunks.push(CompletionChunk {
                llm_call_ordinal: None,
                deltas: vec![CompletionDelta::AppendText { text: content }],
                usage: None,
                raw: None,
            });
        }

        if let Some(refusal) = choice.delta.refusal.filter(|value| !value.is_empty()) {
            chunks.push(CompletionChunk {
                llm_call_ordinal: None,
                deltas: vec![CompletionDelta::AppendRefusal {
                    text: Some(refusal),
                    provider_reason: Some("refusal".to_owned()),
                    opaque_metadata: None,
                }],
                usage: None,
                raw: None,
            });
        }

        if let Some(deltas) = choice.delta.tool_calls {
            for delta in deltas {
                for semantic_delta in apply_tool_call_delta(tool_calls, delta, &raw_json) {
                    chunks.push(CompletionChunk {
                        llm_call_ordinal: None,
                        deltas: vec![semantic_delta],
                        usage: None,
                        raw: None,
                    });
                }
            }
        }

        if matches!(choice.finish_reason.as_deref(), Some("tool_calls")) {
            should_flush_tool_calls = true;
        }
    }

    if should_flush_tool_calls {
        chunks.extend(attach_raw_to_all(
            flush_tool_calls(tool_calls),
            raw_json.clone(),
        ));
    }

    if let Some(usage) = usage {
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

fn apply_tool_call_delta(
    accumulators: &mut Vec<ToolCallAccumulator>,
    delta: OpenAiToolCallDelta,
    raw_json: &Value,
) -> Vec<CompletionDelta> {
    while accumulators.len() <= delta.index {
        accumulators.push(ToolCallAccumulator::default());
    }

    let accumulator = &mut accumulators[delta.index];
    accumulator.source_raw = Some(raw_json.clone());
    let mut emitted = Vec::new();
    if let Some(id) = delta.id {
        accumulator.id = Some(id);
    }
    if let Some(function) = delta.function {
        if let Some(name) = function.name {
            accumulator.name = Some(name);
        }
        maybe_emit_tool_call_open(accumulator, &mut emitted);
        if let Some(arguments) = function.arguments {
            if accumulator.opened {
                if let Some(call_id) = &accumulator.id {
                    emitted.push(CompletionDelta::AppendToolCallArguments {
                        call_id: call_id.clone(),
                        partial_json: arguments,
                    });
                }
            } else {
                accumulator.buffered_arguments.push(arguments);
            }
        }
    }
    maybe_emit_tool_call_open(accumulator, &mut emitted);
    emitted
}

fn flush_tool_calls(accumulators: &mut Vec<ToolCallAccumulator>) -> Vec<CompletionChunk> {
    let mut chunks = Vec::new();
    for accumulator in accumulators.drain(..) {
        let Some(id) = accumulator.id else {
            continue;
        };
        let Some(name) = accumulator.name else {
            continue;
        };
        let mut deltas = Vec::new();
        if !accumulator.opened {
            deltas.push(CompletionDelta::OpenToolCall {
                call_id: id.clone(),
                tool_name: name,
                arguments: None,
            });
        }
        for buffered in accumulator.buffered_arguments {
            deltas.push(CompletionDelta::AppendToolCallArguments {
                call_id: id.clone(),
                partial_json: buffered,
            });
        }
        deltas.push(CompletionDelta::CloseToolCall { call_id: id });
        chunks.push(CompletionChunk {
            llm_call_ordinal: None,
            deltas,
            usage: None,
            raw: accumulator.source_raw,
        });
    }
    chunks
}

fn attach_raw_to_all(mut chunks: Vec<CompletionChunk>, raw: Value) -> Vec<CompletionChunk> {
    for chunk in &mut chunks {
        if chunk.raw.is_none() {
            chunk.raw = Some(raw.clone());
        }
    }
    chunks
}

fn maybe_emit_tool_call_open(
    accumulator: &mut ToolCallAccumulator,
    emitted: &mut Vec<CompletionDelta>,
) {
    if accumulator.opened {
        return;
    }
    let (Some(call_id), Some(tool_name)) = (&accumulator.id, &accumulator.name) else {
        return;
    };
    emitted.push(CompletionDelta::OpenToolCall {
        call_id: call_id.clone(),
        tool_name: tool_name.clone(),
        arguments: None,
    });
    accumulator.opened = true;
    for buffered in accumulator.buffered_arguments.drain(..) {
        emitted.push(CompletionDelta::AppendToolCallArguments {
            call_id: call_id.clone(),
            partial_json: buffered,
        });
    }
}

fn token_usage_from_openai(usage: &OpenAiUsage) -> TokenUsage {
    let total_tokens = if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.prompt_tokens + usage.completion_tokens
    };
    TokenUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens,
        cache_read_tokens: usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens),
        cache_write_tokens: None,
        reasoning_tokens: usage
            .completion_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens),
    }
}

#[cfg(test)]
mod tests {
    use super::request::message_to_openai;
    use super::{
        OpenAiCompatibleProvider, OpenAiStreamChoice, OpenAiStreamDelta, OpenAiStreamResponse,
        OpenAiToolCallDelta, OpenAiToolCallFunctionDelta, StructuredOutputAccumulator,
        capture_structured_output_chunk, completion_chunks_from_sse_event,
    };
    use bt_core::{
        CompletionDelta, CompletionRequest, ConnectionId, Message, MessagePart, Role,
        StructuredOutputSpec, ThinkingConfig, ThinkingEffort, ToolDisplayGroup,
        ToolInterruptBehavior, ToolMetadata, ToolRiskClass, ToolSpec, traits::Provider,
    };
    use futures_util::StreamExt;
    use serde_json::{Value, json};
    use std::convert::Infallible;
    use tokio::net::TcpListener;
    use url::Url;

    #[test]
    fn request_translation_keeps_messages() {
        let provider = OpenAiCompatibleProvider::new(
            "openai-compatible",
            Url::parse("http://127.0.0.1:11434/v1/").expect("url"),
            None,
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("local"),
            model: "qwen3:latest".to_owned(),
            system_prompt: None,
            messages: vec![Message::text(Role::User, "hello")],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(256),
            temperature: Some(0.0),
            thinking: None,
        };

        let translated = provider.build_request(request);
        assert_eq!(translated.model, "qwen3:latest");
        assert_eq!(translated.messages.len(), 1);
        assert_eq!(translated.max_tokens, Some(256));
        assert_eq!(translated.max_completion_tokens, None);
        assert_eq!(translated.reasoning_effort, None);
    }

    #[test]
    fn request_translation_sends_reasoning_effort_for_openai_reasoning_models() {
        let provider = OpenAiCompatibleProvider::new(
            "openai",
            Url::parse("https://api.openai.com/v1/").expect("url"),
            Some("secret".to_owned()),
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("openai"),
            model: "gpt-5.4".to_owned(),
            system_prompt: None,
            messages: vec![Message::text(Role::User, "hello")],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(256),
            temperature: None,
            thinking: Some(ThinkingConfig {
                enabled: true,
                effort: Some(ThinkingEffort::High),
                budget_tokens: None,
                include_summaries: true,
            }),
        };

        let translated = provider.build_request(request);
        assert_eq!(translated.max_tokens, None);
        assert_eq!(translated.max_completion_tokens, Some(256));
        assert_eq!(translated.reasoning_effort, Some(ThinkingEffort::High));
    }

    #[test]
    fn request_translation_clamps_xhigh_to_high_on_chat_completions() {
        let provider = OpenAiCompatibleProvider::new(
            "openai",
            Url::parse("https://api.openai.com/v1/").expect("url"),
            Some("secret".to_owned()),
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("openai"),
            model: "gpt-5.4-mini".to_owned(),
            system_prompt: None,
            messages: vec![Message::text(Role::User, "hello")],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(256),
            temperature: None,
            thinking: Some(ThinkingConfig {
                enabled: true,
                effort: Some(ThinkingEffort::XHigh),
                budget_tokens: None,
                include_summaries: true,
            }),
        };

        let translated = provider.build_request(request);
        assert_eq!(translated.reasoning_effort, Some(ThinkingEffort::High));
    }

    #[test]
    fn request_translation_prepends_system_prompt() {
        let provider = OpenAiCompatibleProvider::new(
            "openai-compatible",
            Url::parse("http://127.0.0.1:11434/v1/").expect("url"),
            None,
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("local"),
            model: "qwen3:latest".to_owned(),
            system_prompt: Some("follow instructions".to_owned()),
            messages: vec![Message::text(Role::User, "hello")],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(256),
            temperature: Some(0.0),
            thinking: None,
        };

        let translated = provider.build_request(request);
        assert_eq!(translated.messages.len(), 2);
        assert_eq!(translated.messages[0].role, "system");
        assert_eq!(
            translated.messages[0].content,
            Some(json!("follow instructions"))
        );
        assert_eq!(translated.messages[1].role, "user");
    }

    #[test]
    fn assistant_tool_call_messages_use_openai_tool_calls_field() {
        let translated = message_to_openai(
            "openai",
            &Message {
                message_id: bt_core::MessageId::new(),
                role: Role::Assistant,
                parts: vec![MessagePart::ToolCall {
                    call: bt_core::ToolCall {
                        tool_name: "shell".to_owned(),
                        call_id: "call-1".to_owned(),
                        arguments: json!({"command": "pwd"}),
                    },
                }],
                created_at: time::OffsetDateTime::now_utc(),
            },
        );

        assert_eq!(translated.role, "assistant");
        assert_eq!(translated.content, Some(json!("")));
        assert_eq!(translated.tool_call_id, None);
        let tool_calls = translated.tool_calls.expect("tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call-1");
        assert_eq!(tool_calls[0].kind, "function");
        assert_eq!(tool_calls[0].function.name, "shell");
        assert_eq!(tool_calls[0].function.arguments, "{\"command\":\"pwd\"}");
    }

    #[test]
    fn assistant_tool_call_messages_for_openai_compatible_backends_include_empty_content() {
        let translated = message_to_openai(
            "local",
            &Message {
                message_id: bt_core::MessageId::new(),
                role: Role::Assistant,
                parts: vec![MessagePart::ToolCall {
                    call: bt_core::ToolCall {
                        tool_name: "shell".to_owned(),
                        call_id: "call-1".to_owned(),
                        arguments: json!({"command": "pwd"}),
                    },
                }],
                created_at: time::OffsetDateTime::now_utc(),
            },
        );

        assert_eq!(translated.role, "assistant");
        assert_eq!(translated.content, Some(json!("")));
        assert_eq!(translated.tool_call_id, None);
        let tool_calls = translated.tool_calls.expect("tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call-1");
        assert_eq!(tool_calls[0].kind, "function");
        assert_eq!(tool_calls[0].function.name, "shell");
        assert_eq!(tool_calls[0].function.arguments, "{\"command\":\"pwd\"}");
    }

    #[test]
    fn reasoning_only_assistant_messages_for_openai_compatible_backends_include_empty_content() {
        let translated = message_to_openai(
            "local",
            &Message {
                message_id: bt_core::MessageId::new(),
                role: Role::Assistant,
                parts: vec![MessagePart::Reasoning {
                    text: Some("thinking".to_owned()),
                    redacted: false,
                    opaque_replay: None,
                }],
                created_at: time::OffsetDateTime::now_utc(),
            },
        );

        assert_eq!(translated.role, "assistant");
        assert_eq!(translated.content, Some(json!("")));
        assert!(translated.tool_calls.is_none());
        assert!(translated.tool_call_id.is_none());
    }

    #[test]
    fn reasoning_only_assistant_messages_for_openai_include_empty_content() {
        let translated = message_to_openai(
            "openai",
            &Message {
                message_id: bt_core::MessageId::new(),
                role: Role::Assistant,
                parts: vec![MessagePart::Reasoning {
                    text: Some("thinking".to_owned()),
                    redacted: false,
                    opaque_replay: None,
                }],
                created_at: time::OffsetDateTime::now_utc(),
            },
        );

        assert_eq!(translated.role, "assistant");
        assert_eq!(translated.content, Some(json!("")));
        assert!(translated.tool_calls.is_none());
        assert!(translated.tool_call_id.is_none());
    }

    #[test]
    fn tool_result_messages_include_tool_call_id_and_stringify_output() {
        let translated = message_to_openai(
            "openai",
            &Message {
                message_id: bt_core::MessageId::new(),
                role: Role::Tool,
                parts: vec![MessagePart::ToolResult {
                    result: bt_core::ToolResultEnvelope {
                        call_id: bt_core::ToolCallId::new("call-1"),
                        tool_name: "shell".to_owned(),
                        is_error: false,
                        output: json!({"stdout":"5","status":0}),
                        duration_ms: Some(5),
                    },
                }],
                created_at: time::OffsetDateTime::now_utc(),
            },
        );

        assert_eq!(translated.role, "tool");
        assert_eq!(translated.tool_call_id.as_deref(), Some("call-1"));
        assert!(translated.tool_calls.is_none());
        assert_eq!(
            translated.content,
            Some(json!("{\"status\":0,\"stdout\":\"5\"}"))
        );
    }

    #[test]
    fn local_reasoning_delta_fields_capture_reasoning() {
        for field in ["reasoning", "reasoning_content"] {
            let mut tool_calls = Vec::new();
            let data = format!(
                r#"{{"choices":[{{"delta":{{"role":"assistant","content":"","{field}":"thinking about it"}}}}]}}"#
            );
            let chunks =
                completion_chunks_from_sse_event(&data, &mut tool_calls).expect("translate");
            let captured = chunks.iter().flat_map(|chunk| &chunk.deltas).any(|delta| {
                matches!(
                    delta,
                    CompletionDelta::AppendReasoning {
                        text: Some(text),
                        redacted: false,
                        opaque_replay: None,
                    } if text == "thinking about it"
                )
            });
            assert!(captured, "delta.{field} must become reasoning");
        }
    }

    #[test]
    fn declared_tools_request_auto_tool_choice() {
        let provider = OpenAiCompatibleProvider::new(
            "openai-compatible",
            Url::parse("http://127.0.0.1:11434/v1/").expect("url"),
            None,
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("local"),
            model: "qwen3:latest".to_owned(),
            system_prompt: None,
            messages: vec![Message::text(Role::User, "count directories")],
            tools: vec![ToolSpec {
                name: "list".to_owned(),
                description: "List files and directories under the project root.".to_owned(),
                parameters_schema: json!({"type":"object"}),
                metadata: ToolMetadata {
                    risk_class: ToolRiskClass::Safe,
                    is_read_only: true,
                    is_concurrency_safe: true,
                    interrupt_behavior: ToolInterruptBehavior::Immediate,
                    execution_mode: bt_core::ToolExecutionMode::Immediate,
                    should_defer: false,
                    catalogue_tags: vec!["files".to_owned()],
                    display_group: ToolDisplayGroup::Codebase,
                },
            }],
            structured_output: None,
            max_tokens: Some(256),
            temperature: Some(0.0),
            thinking: None,
        };

        let translated = provider.build_request(request);
        assert_eq!(translated.tool_choice.as_deref(), Some("auto"));
        assert_eq!(translated.tools.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn openai_reasoning_models_use_max_completion_tokens() {
        let provider = OpenAiCompatibleProvider::new(
            "openai",
            Url::parse("https://api.openai.com/v1/").expect("url"),
            Some("test-key".to_owned()),
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("openai"),
            model: "o4-mini".to_owned(),
            system_prompt: None,
            messages: vec![Message::text(Role::User, "hello")],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(256),
            temperature: None,
            thinking: None,
        };

        let translated = provider.build_request(request);
        assert_eq!(translated.max_tokens, None);
        assert_eq!(translated.max_completion_tokens, Some(256));
    }

    #[test]
    fn hosted_openai_requests_enable_stream_usage() {
        let provider = OpenAiCompatibleProvider::new(
            "openai",
            Url::parse("https://api.openai.com/v1/").expect("url"),
            Some("test-key".to_owned()),
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("openai"),
            model: "o4-mini".to_owned(),
            system_prompt: None,
            messages: vec![Message::text(Role::User, "hello")],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(256),
            temperature: None,
            thinking: None,
        };

        let translated = provider.build_request(request);
        assert!(translated.stream_options.is_some());
    }

    #[test]
    fn hosted_openai_requests_attach_structured_output_response_format() {
        let provider = OpenAiCompatibleProvider::new(
            "openai",
            Url::parse("https://api.openai.com/v1/").expect("url"),
            Some("test-key".to_owned()),
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("openai"),
            model: "gpt-5.1".to_owned(),
            system_prompt: None,
            messages: vec![Message::text(Role::User, "return json")],
            tools: Vec::new(),
            structured_output: Some(StructuredOutputSpec {
                schema_name: Some("demo_output".to_owned()),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string" }
                    },
                    "required": ["answer"],
                    "additionalProperties": false
                }),
            }),
            max_tokens: Some(256),
            temperature: None,
            thinking: None,
        };

        let translated = provider.build_request(request);
        let body = serde_json::to_value(translated).expect("serialize request");
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["name"],
            "demo_output"
        );
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["type"],
            "object"
        );
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    }

    #[test]
    fn structured_output_capture_suppresses_text_deltas_and_emits_final_value() {
        let mut structured_output = StructuredOutputAccumulator::new(StructuredOutputSpec {
            schema_name: Some("demo_output".to_owned()),
            schema: json!({"type":"object"}),
        });

        let first = capture_structured_output_chunk(
            bt_core::CompletionChunk {
                llm_call_ordinal: None,
                deltas: vec![CompletionDelta::AppendText {
                    text: "{\"answer\":".to_owned(),
                }],
                usage: None,
                raw: Some(json!({"id":"chunk-1"})),
            },
            &mut structured_output,
        )
        .expect("first chunk should capture")
        .expect("raw chunk should be retained");
        assert!(first.deltas.is_empty());
        assert_eq!(first.raw, Some(json!({"id":"chunk-1"})));

        let second = capture_structured_output_chunk(
            bt_core::CompletionChunk {
                llm_call_ordinal: None,
                deltas: vec![CompletionDelta::AppendText {
                    text: "\"hello\"}".to_owned(),
                }],
                usage: Some(bt_core::TokenUsage {
                    prompt_tokens: 1,
                    completion_tokens: 2,
                    total_tokens: 3,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
                raw: Some(json!({"id":"chunk-2"})),
            },
            &mut structured_output,
        )
        .expect("second chunk should capture")
        .expect("usage chunk should be retained");
        assert!(second.deltas.is_empty());
        assert_eq!(
            second.usage.as_ref().map(|usage| usage.total_tokens),
            Some(3)
        );

        let usage_only = capture_structured_output_chunk(
            bt_core::CompletionChunk {
                llm_call_ordinal: None,
                deltas: Vec::new(),
                usage: Some(bt_core::TokenUsage {
                    prompt_tokens: 2,
                    completion_tokens: 3,
                    total_tokens: 5,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
                raw: Some(json!({"id":"usage"})),
            },
            &mut structured_output,
        )
        .expect("usage-only chunk should capture")
        .expect("usage-only chunk should be retained");
        assert!(usage_only.deltas.is_empty());

        let final_chunk = structured_output
            .finish()
            .expect("structured output should parse")
            .expect("final structured delta");
        assert_eq!(final_chunk.raw, Some(json!({"id":"chunk-2"})));
        assert!(matches!(
            final_chunk.deltas.as_slice(),
            [CompletionDelta::SetStructuredOutput {
                schema_name: Some(name),
                value
            }] if name == "demo_output" && value == &json!({"answer":"hello"})
        ));
    }

    #[test]
    fn structured_output_capture_does_not_emit_value_after_refusal() {
        let mut structured_output = StructuredOutputAccumulator::new(StructuredOutputSpec {
            schema_name: Some("demo_output".to_owned()),
            schema: json!({"type":"object"}),
        });

        let chunk = capture_structured_output_chunk(
            bt_core::CompletionChunk {
                llm_call_ordinal: None,
                deltas: vec![CompletionDelta::AppendRefusal {
                    text: Some("I can't comply.".to_owned()),
                    provider_reason: Some("refusal".to_owned()),
                    opaque_metadata: Some(Value::Null),
                }],
                usage: None,
                raw: None,
            },
            &mut structured_output,
        )
        .expect("refusal should capture")
        .expect("refusal chunk should still be emitted");
        assert!(matches!(
            chunk.deltas.as_slice(),
            [CompletionDelta::AppendRefusal { .. }]
        ));
        assert!(structured_output.finish().expect("finish").is_none());
    }

    #[test]
    fn response_translation_attaches_raw_payload_to_a_chunk() {
        let mut tool_calls = Vec::new();
        let response = serde_json::to_string(&OpenAiStreamResponse {
            choices: vec![OpenAiStreamChoice {
                delta: OpenAiStreamDelta {
                    reasoning: None,
                    reasoning_content: None,
                    content: Some("hello".to_owned()),
                    refusal: None,
                    tool_calls: Some(vec![OpenAiToolCallDelta {
                        index: 0,
                        id: Some("call-1".to_owned()),
                        function: Some(OpenAiToolCallFunctionDelta {
                            name: Some("read".to_owned()),
                            arguments: Some("{\"path\":\"src/lib.rs\"}".to_owned()),
                        }),
                    }]),
                },
                finish_reason: Some("tool_calls".to_owned()),
            }],
            usage: None,
        })
        .expect("serialize");

        let chunks = completion_chunks_from_sse_event(&response, &mut tool_calls).expect("chunks");
        assert_eq!(chunks.len(), 4);
        assert!(chunks.iter().all(|chunk| chunk.raw.is_some()));
    }

    #[test]
    fn response_translation_extracts_usage_chunks() {
        let mut tool_calls = Vec::new();
        let response = json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 7,
                "total_tokens": 19,
                "prompt_tokens_details": {
                    "cached_tokens": 3
                },
                "completion_tokens_details": {
                    "reasoning_tokens": 2
                }
            }
        })
        .to_string();

        let chunks = completion_chunks_from_sse_event(&response, &mut tool_calls).expect("chunks");
        assert_eq!(chunks.len(), 1);
        let usage = chunks[0].usage.as_ref().expect("usage chunk");
        assert_eq!(usage.prompt_tokens, 12);
        assert_eq!(usage.completion_tokens, 7);
        assert_eq!(usage.total_tokens, 19);
        assert_eq!(usage.cache_read_tokens, Some(3));
        assert_eq!(usage.reasoning_tokens, Some(2));
        assert!(chunks[0].raw.is_some());
    }

    #[test]
    fn response_translation_drops_empty_text_deltas() {
        let mut tool_calls = Vec::new();
        let response = json!({
            "choices": [
                {
                    "delta": {
                        "content": ""
                    },
                    "finish_reason": null
                }
            ]
        })
        .to_string();

        let chunks = completion_chunks_from_sse_event(&response, &mut tool_calls).expect("chunks");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].deltas.is_empty());
        assert!(chunks[0].raw.is_some());
    }

    #[tokio::test]
    async fn provider_streams_sse_incrementally_and_assembles_tool_calls() {
        use axum::Router;
        use axum::response::sse::{Event, Sse};
        use axum::routing::{get, post};
        async fn models() -> &'static str {
            "{\"data\":[]}"
        }

        async fn completions()
        -> Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>> {
            let events = vec![
                Event::default().data("{\"choices\":[{\"delta\":{\"content\":\"hel\"},\"finish_reason\":null}]}"),
                Event::default().data("{\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}"),
                Event::default().data("{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"src\"}}]},\"finish_reason\":null}]}"),
                Event::default().data("{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"/lib.rs\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}"),
                Event::default().data("[DONE]"),
            ];
            Sse::new(futures_util::stream::iter(
                events.into_iter().map(Ok::<_, Infallible>),
            ))
        }

        let app = Router::new()
            .route("/v1/models", get(models))
            .route("/v1/chat/completions", post(completions));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let provider = OpenAiCompatibleProvider::new(
            "openai-compatible",
            Url::parse(&format!("http://{addr}/v1/")).expect("url"),
            None,
        )
        .expect("provider");

        let request = CompletionRequest {
            connection_id: ConnectionId::new("local"),
            model: "qwen3:latest".to_owned(),
            system_prompt: None,
            messages: vec![Message::text(Role::User, "hello")],
            tools: Vec::new(),
            structured_output: None,
            max_tokens: Some(256),
            temperature: Some(0.0),
            thinking: None,
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
        assert!(
            chunks
                .iter()
                .filter(|chunk| !chunk.deltas.is_empty())
                .all(|chunk| chunk.raw.is_some())
        );
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
                        } if tool_name == "read" && call_id == "call-1"
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
            matches!(delta, CompletionDelta::CloseToolCall { call_id } if call_id == "call-1")
        }));
    }
}
