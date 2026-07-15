#![forbid(unsafe_code)]

use bt_core::{
    ApprovalDecision, ApprovalRequest, ApprovalRequirement, CompletionChunk, CompletionDelta,
    CompletionRequest, CompletionSummary, FinishReason, Message, MessagePart, Result, Role,
    SessionId, TokenUsage, ToolCall, ToolCallId, ToolContext, ToolExecutionMode,
    ToolResultEnvelope, push_message_part,
    traits::{ApprovalEvaluator, Provider},
};
use bt_tools::BuiltInToolRegistry;
use camino::Utf8PathBuf;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::{Instrument, info_span};

pub struct TurnLoop {
    provider: Arc<dyn Provider>,
    tools: BuiltInToolRegistry,
    approvals: Arc<dyn ApprovalEvaluator>,
}

#[derive(Clone, Debug, PartialEq)]
struct CompletedToolCall {
    call_id: String,
    tool_name: String,
    arguments: Value,
}

#[derive(Clone, Debug, PartialEq)]
struct StreamingToolCallAccumulator {
    call_id: String,
    tool_name: String,
    initial_arguments: Option<Value>,
    partial_json: String,
}

#[derive(Clone, Debug)]
pub struct TurnRequest {
    pub session_id: SessionId,
    pub project_root: Utf8PathBuf,
    pub request: CompletionRequest,
}

#[derive(Clone, Debug)]
pub struct TurnResult {
    pub messages: Vec<Message>,
    pub tool_results: Vec<ToolResultEnvelope>,
    pub completion_chunks: Vec<CompletionChunk>,
    pub usage: Option<TokenUsage>,
    pub approvals: Vec<TurnApproval>,
    pub approval_requests: Vec<TurnApprovalRequest>,
    pub input_requests: Vec<TurnInputRequest>,
    pub finish_reason: FinishReason,
}

#[derive(Clone, Debug)]
pub struct TurnApproval {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub request: ApprovalRequest,
    pub decision: ApprovalDecision,
}

#[derive(Clone, Debug)]
pub struct TurnApprovalRequest {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub request: ApprovalRequest,
}

#[derive(Clone, Debug)]
pub struct TurnInputRequest {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub prompt: String,
    pub choices: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TurnToolCallRequest {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub arguments: Value,
    pub metadata: bt_core::ToolMetadata,
}

/// Synchronously observes tool lifecycle boundaries before the turn can advance.
///
/// Returning an error aborts the turn. In particular, a request error prevents
/// execution and a result error prevents the result from entering model context.
pub trait ToolLifecycleObserver {
    fn tool_call_requested(
        &mut self,
        request: &TurnToolCallRequest,
        message: &Message,
    ) -> Result<()>;

    fn tool_approval_requested(&mut self, request: &TurnApprovalRequest) -> Result<()>;

    fn tool_approval_resolved(&mut self, approval: &TurnApproval) -> Result<()>;

    fn tool_execution_finished(
        &mut self,
        result: &ToolResultEnvelope,
        message: &Message,
    ) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct LlmCallStarted {
    pub ordinal: u32,
    pub provider: String,
    pub model: String,
    pub message_count: u32,
    pub request: CompletionRequest,
}

#[derive(Clone, Debug)]
pub struct LlmCallFinished {
    pub ordinal: u32,
    pub summary: CompletionSummary,
}

struct NoopToolLifecycleObserver;

impl ToolLifecycleObserver for NoopToolLifecycleObserver {
    fn tool_call_requested(
        &mut self,
        _request: &TurnToolCallRequest,
        _message: &Message,
    ) -> Result<()> {
        Ok(())
    }

    fn tool_approval_requested(&mut self, _request: &TurnApprovalRequest) -> Result<()> {
        Ok(())
    }

    fn tool_approval_resolved(&mut self, _approval: &TurnApproval) -> Result<()> {
        Ok(())
    }

    fn tool_execution_finished(
        &mut self,
        _result: &ToolResultEnvelope,
        _message: &Message,
    ) -> Result<()> {
        Ok(())
    }
}

impl TurnLoop {
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: BuiltInToolRegistry,
        approvals: Arc<dyn ApprovalEvaluator>,
    ) -> Self {
        Self {
            provider,
            tools,
            approvals,
        }
    }

    pub async fn run_turn(&self, turn: TurnRequest) -> Result<TurnResult> {
        self.run_turn_with_callbacks(turn, |_| Ok(()), |_| Ok(()), |_| Ok(()))
            .await
    }

    pub async fn run_turn_with_chunk_callback<F>(
        &self,
        turn: TurnRequest,
        mut on_chunk: F,
    ) -> Result<TurnResult>
    where
        F: FnMut(&CompletionChunk) -> Result<()>,
    {
        self.run_turn_with_callbacks(turn, |_| Ok(()), |chunk| on_chunk(chunk), |_| Ok(()))
            .await
    }

    pub async fn run_turn_with_callbacks<FStart, FChunk, FFinish>(
        &self,
        turn: TurnRequest,
        on_llm_call_started: FStart,
        on_chunk: FChunk,
        on_llm_call_finished: FFinish,
    ) -> Result<TurnResult>
    where
        FStart: FnMut(&LlmCallStarted) -> Result<()>,
        FChunk: FnMut(&CompletionChunk) -> Result<()>,
        FFinish: FnMut(&LlmCallFinished) -> Result<()>,
    {
        let mut tool_lifecycle = NoopToolLifecycleObserver;
        self.run_turn_with_tool_observer(
            turn,
            on_llm_call_started,
            on_chunk,
            on_llm_call_finished,
            &mut tool_lifecycle,
        )
        .await
    }

    pub async fn run_turn_with_tool_observer<FStart, FChunk, FFinish, O>(
        &self,
        mut turn: TurnRequest,
        mut on_llm_call_started: FStart,
        mut on_chunk: FChunk,
        mut on_llm_call_finished: FFinish,
        tool_lifecycle: &mut O,
    ) -> Result<TurnResult>
    where
        FStart: FnMut(&LlmCallStarted) -> Result<()>,
        FChunk: FnMut(&CompletionChunk) -> Result<()>,
        FFinish: FnMut(&LlmCallFinished) -> Result<()>,
        O: ToolLifecycleObserver + ?Sized,
    {
        let mut emitted_messages = Vec::new();
        let mut tool_results = Vec::new();
        let mut completion_chunks = Vec::new();
        let mut usage = None;
        let mut approvals = Vec::new();
        let mut approval_requests = Vec::new();
        let mut input_requests = Vec::new();
        let mut llm_call_ordinal = 0_u32;

        loop {
            let provider_id = self.provider.provider_id().to_owned();
            let connection_id = turn.request.connection_id.to_string();
            let model_id = turn.request.model.clone();
            let message_count = turn.request.messages.len() as u32;
            let llm_span = info_span!(
                "llm_call",
                session_id = %turn.session_id,
                provider = %provider_id,
                connection_id = %connection_id,
                model = %model_id,
                message_count = turn.request.messages.len(),
                tool_count = turn.request.tools.len(),
            );
            let next_llm_call_ordinal = llm_call_ordinal + 1;
            on_llm_call_started(&LlmCallStarted {
                ordinal: next_llm_call_ordinal,
                provider: provider_id.clone(),
                model: model_id.clone(),
                message_count,
                request: turn.request.clone(),
            })?;
            let started_at = std::time::Instant::now();
            let response = async {
                let stream = self
                    .provider
                    .stream_completion(turn.request.clone())
                    .await?;
                self.collect_response(stream, next_llm_call_ordinal, &mut on_chunk)
                    .await
            }
            .instrument(llm_span)
            .await?;
            llm_call_ordinal = next_llm_call_ordinal;
            let summary = self.provider.completion_summary(CompletionSummary {
                provider: provider_id,
                model: model_id,
                finish_reason: response.finish_reason.clone(),
                usage: response.usage.clone().unwrap_or_else(zero_token_usage),
                cost: None,
                latency_ms: started_at.elapsed().as_millis() as u64,
            })?;
            on_llm_call_finished(&LlmCallFinished {
                ordinal: llm_call_ordinal,
                summary: summary.clone(),
            })?;
            completion_chunks.extend(response.chunks.iter().cloned());
            usage = merge_turn_usage(usage, Some(summary.usage.clone()));

            if !response.assistant_parts.is_empty() {
                let message = Message::new(Role::Assistant, response.assistant_parts);
                turn.request.messages.push(message.clone());
                emitted_messages.push(message);
            }

            if response.tool_calls.is_empty() {
                return Ok(TurnResult {
                    messages: emitted_messages,
                    tool_results,
                    completion_chunks,
                    usage,
                    approvals,
                    approval_requests,
                    input_requests,
                    finish_reason: response.finish_reason,
                });
            }

            for tool_call in response.tool_calls {
                let parsed = parse_tool_call(turn.session_id, &tool_call)?;
                let tool_call_message = tool_call_message(&parsed);

                let tool = self.tools.get(&parsed.tool_name).ok_or_else(|| {
                    bt_core::BelltowerError::Unsupported(format!(
                        "unknown tool `{}`",
                        parsed.tool_name
                    ))
                })?;
                let spec = tool.spec();
                let requirement = tool.approval_requirement(&parsed.arguments);
                let input_request =
                    matches!(spec.metadata.execution_mode, ToolExecutionMode::UserInput)
                        .then(|| parse_input_request(&parsed))
                        .transpose()?;
                tool_lifecycle.tool_call_requested(
                    &TurnToolCallRequest {
                        call_id: parsed.call_id.clone(),
                        tool_name: parsed.tool_name.clone(),
                        arguments: parsed.arguments.clone(),
                        metadata: spec.metadata.clone(),
                    },
                    &tool_call_message,
                )?;
                turn.request.messages.push(tool_call_message.clone());
                emitted_messages.push(tool_call_message);

                if let Some(input_request) = input_request {
                    input_requests.push(input_request);
                    return Ok(TurnResult {
                        messages: emitted_messages,
                        tool_results,
                        completion_chunks,
                        usage,
                        approvals,
                        approval_requests,
                        input_requests,
                        finish_reason: FinishReason::ToolUse,
                    });
                }

                let approval_outcome =
                    match self.approval_outcome(&parsed, requirement, spec.metadata) {
                        Ok(outcome) => outcome,
                        Err(error) => {
                            observe_tool_lifecycle_failure(tool_lifecycle, &parsed, &error)?;
                            return Err(error);
                        }
                    };
                let result = match approval_outcome {
                    ApprovalOutcome::NotNeeded => self
                        .execute_tool_call(&parsed, &turn.project_root)
                        .await
                        .unwrap_or_else(|error| tool_error_result(&parsed, error)),
                    ApprovalOutcome::Approved(request, decision) => {
                        let approval = TurnApproval {
                            call_id: parsed.call_id.clone(),
                            tool_name: parsed.tool_name.clone(),
                            request,
                            decision,
                        };
                        if let Err(error) = tool_lifecycle.tool_approval_resolved(&approval) {
                            observe_tool_lifecycle_failure(tool_lifecycle, &parsed, &error)?;
                            return Err(error);
                        }
                        approvals.push(approval);
                        self.execute_tool_call(&parsed, &turn.project_root)
                            .await
                            .unwrap_or_else(|error| tool_error_result(&parsed, error))
                    }
                    ApprovalOutcome::Denied(request, decision) => {
                        let approval = TurnApproval {
                            call_id: parsed.call_id.clone(),
                            tool_name: parsed.tool_name.clone(),
                            request,
                            decision: decision.clone(),
                        };
                        if let Err(error) = tool_lifecycle.tool_approval_resolved(&approval) {
                            observe_tool_lifecycle_failure(tool_lifecycle, &parsed, &error)?;
                            return Err(error);
                        }
                        approvals.push(approval);
                        tool_error_result(
                            &parsed,
                            bt_core::BelltowerError::InvalidState(format!(
                                "tool `{}` was denied",
                                parsed.tool_name
                            )),
                        )
                    }
                    ApprovalOutcome::Pending(request) => {
                        let approval_request = TurnApprovalRequest {
                            call_id: parsed.call_id.clone(),
                            tool_name: parsed.tool_name.clone(),
                            request,
                        };
                        if let Err(error) =
                            tool_lifecycle.tool_approval_requested(&approval_request)
                        {
                            observe_tool_lifecycle_failure(tool_lifecycle, &parsed, &error)?;
                            return Err(error);
                        }
                        approval_requests.push(approval_request);
                        return Ok(TurnResult {
                            messages: emitted_messages,
                            tool_results,
                            completion_chunks,
                            usage,
                            approvals,
                            approval_requests,
                            input_requests,
                            finish_reason: FinishReason::ToolUse,
                        });
                    }
                };
                let message = tool_result_message(result.clone());
                tool_lifecycle.tool_execution_finished(&result, &message)?;
                turn.request.messages.push(message.clone());
                emitted_messages.push(message);
                tool_results.push(result);
            }
        }
    }

    async fn collect_response<F>(
        &self,
        mut stream: bt_core::traits::BoxStream<Result<CompletionChunk>>,
        llm_call_ordinal: u32,
        on_chunk: &mut F,
    ) -> Result<CollectedResponse>
    where
        F: FnMut(&CompletionChunk) -> Result<()>,
    {
        let mut assistant_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut open_tool_calls = Vec::new();
        let mut chunks = Vec::new();
        let mut usage = None;

        while let Some(chunk) = stream.next().await {
            let mut chunk = chunk?;
            chunk.llm_call_ordinal = Some(llm_call_ordinal);
            on_chunk(&chunk)?;
            chunks.push(chunk.clone());
            if let Some(chunk_usage) = chunk.usage.clone() {
                usage = Some(chunk_usage);
            }
            for delta in &chunk.deltas {
                match delta {
                    CompletionDelta::AppendText { text } => {
                        push_message_part(
                            &mut assistant_parts,
                            MessagePart::Text { text: text.clone() },
                        );
                    }
                    CompletionDelta::AppendReasoning {
                        text,
                        redacted,
                        opaque_replay,
                    } => {
                        push_message_part(
                            &mut assistant_parts,
                            MessagePart::Reasoning {
                                text: text.clone(),
                                redacted: *redacted,
                                opaque_replay: opaque_replay.clone(),
                            },
                        );
                    }
                    CompletionDelta::AppendRefusal {
                        text,
                        provider_reason,
                        opaque_metadata,
                    } => {
                        push_message_part(
                            &mut assistant_parts,
                            MessagePart::Refusal {
                                text: text.clone(),
                                provider_reason: provider_reason.clone(),
                                opaque_metadata: opaque_metadata.clone(),
                            },
                        );
                    }
                    CompletionDelta::SetStructuredOutput { schema_name, value } => {
                        assistant_parts.push(MessagePart::Structured {
                            schema_name: schema_name.clone(),
                            value: value.clone(),
                        });
                    }
                    CompletionDelta::OpenToolCall {
                        call_id,
                        tool_name,
                        arguments,
                    } => open_tool_call(
                        &mut open_tool_calls,
                        call_id.clone(),
                        tool_name.clone(),
                        arguments.clone(),
                    ),
                    CompletionDelta::AppendToolCallArguments {
                        call_id,
                        partial_json,
                    } => append_tool_call_arguments(&mut open_tool_calls, call_id, partial_json)?,
                    CompletionDelta::CloseToolCall { call_id } => {
                        if let Some(tool_call) = close_tool_call(&mut open_tool_calls, call_id)? {
                            tool_calls.push(tool_call);
                        }
                    }
                }
            }
        }

        tool_calls.extend(finish_open_tool_calls(open_tool_calls)?);

        let finish_reason = if tool_calls.is_empty() {
            FinishReason::Stop
        } else {
            FinishReason::ToolUse
        };

        Ok(CollectedResponse {
            assistant_parts,
            tool_calls,
            chunks,
            usage,
            finish_reason,
        })
    }

    async fn execute_tool_call(
        &self,
        tool_call: &ParsedToolCall,
        project_root: &Utf8PathBuf,
    ) -> Result<ToolResultEnvelope> {
        let tool_span = info_span!(
            "tool_execution",
            session_id = %tool_call.session_id,
            tool_name = %tool_call.tool_name,
            call_id = %tool_call.call_id,
            project_root = %project_root,
        );
        async {
            let tool = self.tools.get(&tool_call.tool_name).ok_or_else(|| {
                bt_core::BelltowerError::Unsupported(format!(
                    "unknown tool `{}`",
                    tool_call.tool_name
                ))
            })?;
            tool.execute(
                execution_arguments(tool_call),
                ToolContext {
                    project_root: project_root.clone(),
                },
            )
            .await
        }
        .instrument(tool_span)
        .await
    }

    fn approval_outcome(
        &self,
        tool_call: &ParsedToolCall,
        requirement: ApprovalRequirement,
        tool_metadata: bt_core::ToolMetadata,
    ) -> Result<ApprovalOutcome> {
        if matches!(requirement, ApprovalRequirement::Never) {
            return Ok(ApprovalOutcome::NotNeeded);
        }

        let request = ApprovalRequest {
            session_id: tool_call.session_id,
            call_id: tool_call.call_id.clone(),
            tool_name: tool_call.tool_name.clone(),
            arguments: tool_call.arguments.clone(),
            requirement,
            tool_metadata,
            requested_at: time::OffsetDateTime::now_utc(),
        };

        match self.approvals.evaluate(&request)? {
            Some(decision @ ApprovalDecision::Approved { .. }) => {
                Ok(ApprovalOutcome::Approved(request, decision))
            }
            Some(decision @ ApprovalDecision::Denied { .. }) => {
                Ok(ApprovalOutcome::Denied(request, decision))
            }
            None => Ok(ApprovalOutcome::Pending(request)),
        }
    }
}

struct CollectedResponse {
    assistant_parts: Vec<MessagePart>,
    tool_calls: Vec<CompletedToolCall>,
    chunks: Vec<CompletionChunk>,
    usage: Option<TokenUsage>,
    finish_reason: FinishReason,
}

enum ApprovalOutcome {
    NotNeeded,
    Approved(ApprovalRequest, ApprovalDecision),
    Denied(ApprovalRequest, ApprovalDecision),
    Pending(ApprovalRequest),
}

fn parse_input_request(tool_call: &ParsedToolCall) -> Result<TurnInputRequest> {
    let prompt = tool_call
        .arguments
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            bt_core::BelltowerError::Tool("ask tool call is missing `question`".to_owned())
        })?
        .to_owned();
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
    Ok(TurnInputRequest {
        call_id: tool_call.call_id.clone(),
        tool_name: tool_call.tool_name.clone(),
        prompt,
        choices,
    })
}

fn merge_turn_usage(existing: Option<TokenUsage>, next: Option<TokenUsage>) -> Option<TokenUsage> {
    match (existing, next) {
        (Some(existing), Some(next)) => Some(TokenUsage {
            prompt_tokens: existing.prompt_tokens + next.prompt_tokens,
            completion_tokens: existing.completion_tokens + next.completion_tokens,
            total_tokens: existing.total_tokens + next.total_tokens,
            cache_read_tokens: combine_optional_usage(
                existing.cache_read_tokens,
                next.cache_read_tokens,
            ),
            cache_write_tokens: combine_optional_usage(
                existing.cache_write_tokens,
                next.cache_write_tokens,
            ),
            reasoning_tokens: combine_optional_usage(
                existing.reasoning_tokens,
                next.reasoning_tokens,
            ),
        }),
        (Some(existing), None) => Some(existing),
        (None, Some(next)) => Some(next),
        (None, None) => None,
    }
}

fn combine_optional_usage(existing: Option<u64>, next: Option<u64>) -> Option<u64> {
    match (existing, next) {
        (Some(existing), Some(next)) => Some(existing + next),
        (Some(existing), None) => Some(existing),
        (None, Some(next)) => Some(next),
        (None, None) => None,
    }
}

fn zero_token_usage() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        cache_read_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
    }
}

struct ParsedToolCall {
    session_id: SessionId,
    call_id: ToolCallId,
    tool_name: String,
    arguments: Value,
}

fn parse_tool_call(session_id: SessionId, tool_call: &CompletedToolCall) -> Result<ParsedToolCall> {
    let arguments = normalize_tool_arguments(tool_call.arguments.clone());

    Ok(ParsedToolCall {
        session_id,
        call_id: ToolCallId::new(&tool_call.call_id),
        tool_name: tool_call.tool_name.clone(),
        arguments,
    })
}

fn normalize_tool_arguments(arguments: Value) -> Value {
    match arguments {
        Value::Object(mut object) => {
            object.remove("call_id");
            Value::Object(object)
        }
        other => other,
    }
}

fn tool_call_message(tool_call: &ParsedToolCall) -> Message {
    Message {
        message_id: bt_core::MessageId::new(),
        role: Role::Assistant,
        parts: vec![MessagePart::ToolCall {
            call: ToolCall {
                tool_name: tool_call.tool_name.clone(),
                call_id: tool_call.call_id.to_string(),
                arguments: tool_call.arguments.clone(),
            },
        }],
        created_at: time::OffsetDateTime::now_utc(),
    }
}

fn tool_error_result(
    tool_call: &ParsedToolCall,
    error: bt_core::BelltowerError,
) -> ToolResultEnvelope {
    ToolResultEnvelope {
        call_id: tool_call.call_id.clone(),
        tool_name: tool_call.tool_name.clone(),
        is_error: true,
        output: serde_json::json!({ "error": error.to_string() }),
        duration_ms: None,
    }
}

fn observe_tool_lifecycle_failure<O>(
    observer: &mut O,
    tool_call: &ParsedToolCall,
    error: &bt_core::BelltowerError,
) -> Result<()>
where
    O: ToolLifecycleObserver + ?Sized,
{
    let result = ToolResultEnvelope {
        call_id: tool_call.call_id.clone(),
        tool_name: tool_call.tool_name.clone(),
        is_error: true,
        output: serde_json::json!({
            "error": {
                "class": error.class(),
                "code": error.code(),
                "message": error.to_string(),
                "retryable": error.retryable(),
            }
        }),
        duration_ms: None,
    };
    observer.tool_execution_finished(&result, &tool_result_message(result.clone()))
}

fn tool_result_message(result: ToolResultEnvelope) -> Message {
    Message {
        message_id: bt_core::MessageId::new(),
        role: Role::Tool,
        parts: vec![MessagePart::ToolResult { result }],
        created_at: time::OffsetDateTime::now_utc(),
    }
}

fn execution_arguments(tool_call: &ParsedToolCall) -> Value {
    match tool_call.arguments.clone() {
        Value::Object(mut object) => {
            object.insert("call_id".to_owned(), json!(tool_call.call_id.to_string()));
            Value::Object(object)
        }
        value => json!({
            "call_id": tool_call.call_id.to_string(),
            "input": value,
        }),
    }
}

fn open_tool_call(
    open_tool_calls: &mut Vec<StreamingToolCallAccumulator>,
    call_id: String,
    tool_name: String,
    arguments: Option<Value>,
) {
    if let Some(existing) = open_tool_calls
        .iter_mut()
        .find(|existing| existing.call_id == call_id)
    {
        existing.tool_name = tool_name;
        if arguments.is_some() {
            existing.initial_arguments = arguments;
        }
        return;
    }

    open_tool_calls.push(StreamingToolCallAccumulator {
        call_id,
        tool_name,
        initial_arguments: arguments,
        partial_json: String::new(),
    });
}

fn append_tool_call_arguments(
    open_tool_calls: &mut [StreamingToolCallAccumulator],
    call_id: &str,
    partial_json: &str,
) -> Result<()> {
    let tool_call = open_tool_calls
        .iter_mut()
        .find(|existing| existing.call_id == call_id)
        .ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "tool call `{call_id}` received arguments before open"
            ))
        })?;
    tool_call.partial_json.push_str(partial_json);
    Ok(())
}

fn close_tool_call(
    open_tool_calls: &mut Vec<StreamingToolCallAccumulator>,
    call_id: &str,
) -> Result<Option<CompletedToolCall>> {
    let Some(index) = open_tool_calls
        .iter()
        .position(|existing| existing.call_id == call_id)
    else {
        return Err(bt_core::BelltowerError::InvalidState(format!(
            "tool call `{call_id}` closed before open"
        )));
    };
    let accumulator = open_tool_calls.remove(index);
    Ok(Some(finalize_tool_call(accumulator)?))
}

fn finish_open_tool_calls(
    open_tool_calls: Vec<StreamingToolCallAccumulator>,
) -> Result<Vec<CompletedToolCall>> {
    open_tool_calls
        .into_iter()
        .map(finalize_tool_call)
        .collect()
}

fn finalize_tool_call(accumulator: StreamingToolCallAccumulator) -> Result<CompletedToolCall> {
    if accumulator.call_id.trim().is_empty() {
        return Err(bt_core::BelltowerError::InvalidState(
            "tool call missing `id`".to_owned(),
        ));
    }
    if accumulator.tool_name.trim().is_empty() {
        return Err(bt_core::BelltowerError::InvalidState(
            "tool call missing `name`".to_owned(),
        ));
    }

    let arguments = if !accumulator.partial_json.trim().is_empty() {
        serde_json::from_str(&accumulator.partial_json)
            .unwrap_or(Value::String(accumulator.partial_json))
    } else {
        accumulator
            .initial_arguments
            .unwrap_or_else(|| serde_json::json!({}))
    };

    Ok(CompletedToolCall {
        call_id: accumulator.call_id,
        tool_name: accumulator.tool_name,
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::{ToolLifecycleObserver, TurnLoop, TurnRequest, TurnToolCallRequest};
    use bt_core::{
        ApprovalDecision, ApprovalDecisionSource, ApprovalRequest, ApprovalScope, BelltowerError,
        CompletionChunk, CompletionDelta, CompletionRequest, ConnectionId, ConnectionStatus,
        ModelPricing, Result, Role, SessionId, ToolContext, ToolDisplayGroup, ToolExecutionMode,
        ToolInterruptBehavior, ToolMetadata, ToolResultEnvelope, ToolRiskClass, ToolSpec,
        traits::{ApprovalEvaluator, BoxFuture, BoxStream, Provider, ToolExecutor},
    };
    use bt_tools::BuiltInToolRegistry;
    use camino::Utf8PathBuf;
    use futures_util::stream;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    struct MockProvider {
        responses: Mutex<VecDeque<Vec<CompletionChunk>>>,
        call_count: AtomicUsize,
        lifecycle_events: Option<Arc<Mutex<Vec<String>>>>,
    }

    impl MockProvider {
        fn new(responses: Vec<Vec<CompletionChunk>>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                call_count: AtomicUsize::new(0),
                lifecycle_events: None,
            }
        }

        fn recording(
            responses: Vec<Vec<CompletionChunk>>,
            lifecycle_events: Arc<Mutex<Vec<String>>>,
        ) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                call_count: AtomicUsize::new(0),
                lifecycle_events: Some(lifecycle_events),
            }
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    impl Provider for MockProvider {
        fn provider_id(&self) -> &str {
            "mock"
        }

        fn validate(&self) -> BoxFuture<'_, Result<ConnectionStatus>> {
            Box::pin(async { Ok(ConnectionStatus::Healthy) })
        }

        fn stream_completion(
            &self,
            _request: CompletionRequest,
        ) -> BoxFuture<'_, Result<BoxStream<Result<CompletionChunk>>>> {
            Box::pin(async move {
                let call_number = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(events) = &self.lifecycle_events {
                    events
                        .lock()
                        .expect("lock")
                        .push(format!("provider_call_{call_number}"));
                }
                let response = self
                    .responses
                    .lock()
                    .expect("lock")
                    .pop_front()
                    .unwrap_or_default();
                let stream: BoxStream<Result<CompletionChunk>> =
                    Box::pin(stream::iter(response.into_iter().map(Ok)));
                Ok(stream)
            })
        }

        fn pricing(&self, _model: &str) -> Option<ModelPricing> {
            None
        }
    }

    struct TestToolLifecycleObserver {
        events: Arc<Mutex<Vec<String>>>,
        fail_request: bool,
        fail_approval_requested: bool,
        fail_approval_resolved: bool,
        fail_result: bool,
    }

    impl TestToolLifecycleObserver {
        fn new(fail_request: bool, fail_result: bool) -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                fail_request,
                fail_approval_requested: false,
                fail_approval_resolved: false,
                fail_result,
            }
        }

        fn failing_approval_requested() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                fail_request: false,
                fail_approval_requested: true,
                fail_approval_resolved: false,
                fail_result: false,
            }
        }

        fn failing_approval_resolved() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                fail_request: false,
                fail_approval_requested: false,
                fail_approval_resolved: true,
                fail_result: false,
            }
        }

        fn recording(events: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                events,
                fail_request: false,
                fail_approval_requested: false,
                fail_approval_resolved: false,
                fail_result: false,
            }
        }
    }

    impl ToolLifecycleObserver for TestToolLifecycleObserver {
        fn tool_call_requested(
            &mut self,
            request: &TurnToolCallRequest,
            _message: &bt_core::Message,
        ) -> Result<()> {
            self.events
                .lock()
                .expect("lock")
                .push(format!("tool_requested:{}", request.call_id));
            if self.fail_request {
                return Err(BelltowerError::Storage(
                    "tool request observation failed".to_owned(),
                ));
            }
            Ok(())
        }

        fn tool_approval_requested(&mut self, request: &super::TurnApprovalRequest) -> Result<()> {
            self.events
                .lock()
                .expect("lock")
                .push(format!("approval_requested:{}", request.call_id));
            if self.fail_approval_requested {
                return Err(BelltowerError::Storage(
                    "approval request observation failed".to_owned(),
                ));
            }
            Ok(())
        }

        fn tool_approval_resolved(&mut self, approval: &super::TurnApproval) -> Result<()> {
            self.events
                .lock()
                .expect("lock")
                .push(format!("approval_resolved:{}", approval.call_id));
            if self.fail_approval_resolved {
                return Err(BelltowerError::Storage(
                    "approval resolution observation failed".to_owned(),
                ));
            }
            Ok(())
        }

        fn tool_execution_finished(
            &mut self,
            result: &ToolResultEnvelope,
            _message: &bt_core::Message,
        ) -> Result<()> {
            self.events
                .lock()
                .expect("lock")
                .push(format!("tool_finished:{}", result.call_id));
            if self.fail_result {
                return Err(BelltowerError::Storage(
                    "tool result observation failed".to_owned(),
                ));
            }
            Ok(())
        }
    }

    struct PendingApprovalEvaluator;

    impl ApprovalEvaluator for PendingApprovalEvaluator {
        fn evaluate(&self, _request: &ApprovalRequest) -> Result<Option<ApprovalDecision>> {
            Ok(None)
        }
    }

    struct AlwaysApproveEvaluator;

    impl ApprovalEvaluator for AlwaysApproveEvaluator {
        fn evaluate(&self, _request: &ApprovalRequest) -> Result<Option<ApprovalDecision>> {
            Ok(Some(ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Runtime,
            }))
        }
    }

    struct FailingApprovalEvaluator;

    impl ApprovalEvaluator for FailingApprovalEvaluator {
        fn evaluate(&self, _request: &ApprovalRequest) -> Result<Option<ApprovalDecision>> {
            Err(BelltowerError::Storage(
                "approval evaluation failed".to_owned(),
            ))
        }
    }

    #[derive(Clone)]
    struct AskTool;

    impl ToolExecutor for AskTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "ask".to_owned(),
                description: "Ask the human for input".to_owned(),
                parameters_schema: json!({
                    "type": "object",
                    "required": ["question"],
                    "properties": {
                        "question": {"type": "string"},
                        "choices": {
                            "type": "array",
                            "items": {"type": "string"}
                        }
                    }
                }),
                metadata: ToolMetadata {
                    risk_class: ToolRiskClass::Safe,
                    is_read_only: true,
                    is_concurrency_safe: false,
                    execution_mode: ToolExecutionMode::UserInput,
                    interrupt_behavior: ToolInterruptBehavior::Immediate,
                    should_defer: false,
                    catalogue_tags: vec!["ask".to_owned(), "input".to_owned()],
                    display_group: ToolDisplayGroup::Interaction,
                },
            }
        }

        fn approval_requirement(
            &self,
            _arguments: &serde_json::Value,
        ) -> bt_core::ApprovalRequirement {
            bt_core::ApprovalRequirement::Never
        }

        fn execute(
            &self,
            _arguments: serde_json::Value,
            _context: ToolContext,
        ) -> bt_core::traits::ToolFuture<'_> {
            Box::pin(async move {
                unreachable!("ask tool should pause before execution");
            })
        }
    }

    fn tool_call_chunk(
        call_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> CompletionChunk {
        CompletionChunk {
            llm_call_ordinal: None,
            deltas: vec![
                CompletionDelta::OpenToolCall {
                    call_id: call_id.to_owned(),
                    tool_name: tool_name.to_owned(),
                    arguments: Some(arguments),
                },
                CompletionDelta::CloseToolCall {
                    call_id: call_id.to_owned(),
                },
            ],
            usage: None,
            raw: None,
        }
    }

    fn text_chunk(text: &str) -> CompletionChunk {
        CompletionChunk {
            llm_call_ordinal: None,
            deltas: vec![CompletionDelta::AppendText {
                text: text.to_owned(),
            }],
            usage: None,
            raw: None,
        }
    }

    async fn assert_pre_execution_failure_is_terminalized(
        approvals: Arc<dyn ApprovalEvaluator>,
        mut observer: TestToolLifecycleObserver,
        expected_events: &[&str],
    ) {
        let root = TempDir::new().expect("tempdir");
        let project_root =
            Utf8PathBuf::from_path_buf(root.path().to_path_buf()).expect("utf8 path");
        let provider = Arc::new(MockProvider::new(vec![vec![tool_call_chunk(
            "call-approval-failure",
            "write",
            json!({
                "path": "should-not-exist.txt",
                "content": "not durable"
            }),
        )]]));
        let turn_loop = TurnLoop::new(provider.clone(), BuiltInToolRegistry::new(), approvals);

        let error = turn_loop
            .run_turn_with_tool_observer(
                TurnRequest {
                    session_id: SessionId::new(),
                    project_root: project_root.clone(),
                    request: CompletionRequest {
                        connection_id: ConnectionId::new("local"),
                        model: "test".to_owned(),
                        system_prompt: None,
                        messages: vec![bt_core::Message::text(Role::User, "write a file")],
                        tools: BuiltInToolRegistry::new().specs(),
                        structured_output: None,
                        max_tokens: None,
                        temperature: None,
                        thinking: None,
                    },
                },
                |_| Ok(()),
                |_| Ok(()),
                |_| Ok(()),
                &mut observer,
            )
            .await
            .expect_err("approval boundary should fail the turn");

        assert!(matches!(error, BelltowerError::Storage(_)));
        assert_eq!(provider.call_count(), 1);
        assert!(!project_root.join("should-not-exist.txt").exists());
        assert_eq!(
            observer.events.lock().expect("lock").as_slice(),
            expected_events
        );
    }

    #[tokio::test]
    async fn approval_evaluator_failure_closes_committed_tool_request() {
        assert_pre_execution_failure_is_terminalized(
            Arc::new(FailingApprovalEvaluator),
            TestToolLifecycleObserver::new(false, false),
            &[
                "tool_requested:call-approval-failure",
                "tool_finished:call-approval-failure",
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn approval_request_observer_failure_closes_committed_tool_request() {
        assert_pre_execution_failure_is_terminalized(
            Arc::new(PendingApprovalEvaluator),
            TestToolLifecycleObserver::failing_approval_requested(),
            &[
                "tool_requested:call-approval-failure",
                "approval_requested:call-approval-failure",
                "tool_finished:call-approval-failure",
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn approval_resolution_observer_failure_closes_committed_tool_request() {
        assert_pre_execution_failure_is_terminalized(
            Arc::new(AlwaysApproveEvaluator),
            TestToolLifecycleObserver::failing_approval_resolved(),
            &[
                "tool_requested:call-approval-failure",
                "approval_resolved:call-approval-failure",
                "tool_finished:call-approval-failure",
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn tool_request_observer_failure_prevents_execution() {
        let root = TempDir::new().expect("tempdir");
        let project_root =
            Utf8PathBuf::from_path_buf(root.path().to_path_buf()).expect("utf8 path");
        let provider = Arc::new(MockProvider::new(vec![vec![tool_call_chunk(
            "call-request-failure",
            "write",
            json!({
                "path": "should-not-exist.txt",
                "content": "not durable"
            }),
        )]]));
        let turn_loop = TurnLoop::new(
            provider.clone(),
            BuiltInToolRegistry::new(),
            Arc::new(AlwaysApproveEvaluator),
        );
        let mut observer = TestToolLifecycleObserver::new(true, false);

        let error = turn_loop
            .run_turn_with_tool_observer(
                TurnRequest {
                    session_id: SessionId::new(),
                    project_root: project_root.clone(),
                    request: CompletionRequest {
                        connection_id: ConnectionId::new("local"),
                        model: "test".to_owned(),
                        system_prompt: None,
                        messages: vec![bt_core::Message::text(Role::User, "write a file")],
                        tools: BuiltInToolRegistry::new().specs(),
                        structured_output: None,
                        max_tokens: None,
                        temperature: None,
                        thinking: None,
                    },
                },
                |_| Ok(()),
                |_| Ok(()),
                |_| Ok(()),
                &mut observer,
            )
            .await
            .expect_err("request observation should fail the turn");

        assert!(matches!(error, BelltowerError::Storage(_)));
        assert_eq!(provider.call_count(), 1);
        assert!(!project_root.join("should-not-exist.txt").exists());
    }

    #[tokio::test]
    async fn tool_result_observer_failure_prevents_reentry_and_sibling_execution() {
        let root = TempDir::new().expect("tempdir");
        let project_root =
            Utf8PathBuf::from_path_buf(root.path().to_path_buf()).expect("utf8 path");
        let provider = Arc::new(MockProvider::new(vec![
            vec![
                tool_call_chunk(
                    "call-first",
                    "write",
                    json!({
                        "path": "first.txt",
                        "content": "executed"
                    }),
                ),
                tool_call_chunk(
                    "call-sibling",
                    "write",
                    json!({
                        "path": "sibling.txt",
                        "content": "must not execute"
                    }),
                ),
            ],
            vec![text_chunk("provider re-entry must not happen")],
        ]));
        let turn_loop = TurnLoop::new(
            provider.clone(),
            BuiltInToolRegistry::new(),
            Arc::new(AlwaysApproveEvaluator),
        );
        let mut observer = TestToolLifecycleObserver::new(false, true);

        let error = turn_loop
            .run_turn_with_tool_observer(
                TurnRequest {
                    session_id: SessionId::new(),
                    project_root: project_root.clone(),
                    request: CompletionRequest {
                        connection_id: ConnectionId::new("local"),
                        model: "test".to_owned(),
                        system_prompt: None,
                        messages: vec![bt_core::Message::text(Role::User, "write two files")],
                        tools: BuiltInToolRegistry::new().specs(),
                        structured_output: None,
                        max_tokens: None,
                        temperature: None,
                        thinking: None,
                    },
                },
                |_| Ok(()),
                |_| Ok(()),
                |_| Ok(()),
                &mut observer,
            )
            .await
            .expect_err("result observation should fail the turn");

        assert!(matches!(error, BelltowerError::Storage(_)));
        assert_eq!(provider.call_count(), 1);
        assert_eq!(
            std::fs::read_to_string(project_root.join("first.txt").as_std_path())
                .expect("first tool should execute"),
            "executed"
        );
        assert!(!project_root.join("sibling.txt").exists());
    }

    #[tokio::test]
    async fn terminal_tool_observation_precedes_provider_reentry() {
        let root = TempDir::new().expect("tempdir");
        let project_root =
            Utf8PathBuf::from_path_buf(root.path().to_path_buf()).expect("utf8 path");
        let events = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(MockProvider::recording(
            vec![
                vec![tool_call_chunk(
                    "call-success",
                    "write",
                    json!({
                        "path": "observed.txt",
                        "content": "durable boundary"
                    }),
                )],
                vec![text_chunk("done")],
            ],
            events.clone(),
        ));
        let turn_loop = TurnLoop::new(
            provider.clone(),
            BuiltInToolRegistry::new(),
            Arc::new(AlwaysApproveEvaluator),
        );
        let mut observer = TestToolLifecycleObserver::recording(events.clone());

        let result = turn_loop
            .run_turn_with_tool_observer(
                TurnRequest {
                    session_id: SessionId::new(),
                    project_root,
                    request: CompletionRequest {
                        connection_id: ConnectionId::new("local"),
                        model: "test".to_owned(),
                        system_prompt: None,
                        messages: vec![bt_core::Message::text(Role::User, "write a file")],
                        tools: BuiltInToolRegistry::new().specs(),
                        structured_output: None,
                        max_tokens: None,
                        temperature: None,
                        thinking: None,
                    },
                },
                |_| Ok(()),
                |_| Ok(()),
                |_| Ok(()),
                &mut observer,
            )
            .await
            .expect("turn should complete");

        assert_eq!(provider.call_count(), 2);
        assert_eq!(result.tool_results.len(), 1);
        assert_eq!(
            *events.lock().expect("lock"),
            vec![
                "provider_call_1".to_owned(),
                "tool_requested:call-success".to_owned(),
                "approval_resolved:call-success".to_owned(),
                "tool_finished:call-success".to_owned(),
                "provider_call_2".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn pending_approval_is_reported_in_turn_result() {
        let provider = Arc::new(MockProvider::new(vec![
            vec![tool_call_chunk(
                "call-1",
                "write",
                json!({
                    "path": "note.txt",
                    "content": "hi"
                }),
            )],
            vec![text_chunk("waiting on approval")],
        ]));
        let turn_loop = TurnLoop::new(
            provider,
            BuiltInToolRegistry::new(),
            Arc::new(PendingApprovalEvaluator),
        );

        let result = turn_loop
            .run_turn(TurnRequest {
                session_id: SessionId::new(),
                project_root: Utf8PathBuf::from("."),
                request: CompletionRequest {
                    connection_id: ConnectionId::new("local"),
                    model: "test".to_owned(),
                    system_prompt: None,
                    messages: vec![bt_core::Message::text(Role::User, "write a file")],
                    tools: BuiltInToolRegistry::new().specs(),
                    structured_output: None,
                    max_tokens: None,
                    temperature: None,
                    thinking: None,
                },
            })
            .await
            .expect("turn should complete");

        assert_eq!(result.approval_requests.len(), 1);
        assert!(result.tool_results.is_empty());
        assert!(result.messages.iter().any(|message| {
            message
                .tool_call()
                .is_some_and(|call| call.call_id == "call-1")
        }));
    }

    #[tokio::test]
    async fn pending_approval_suspends_later_tool_calls_in_same_response() {
        let provider = Arc::new(MockProvider::new(vec![vec![
            tool_call_chunk(
                "call-pending",
                "write",
                json!({
                    "path": "note.txt",
                    "content": "hi"
                }),
            ),
            tool_call_chunk(
                "call-later",
                "read",
                json!({
                    "path": "note.txt"
                }),
            ),
        ]]));
        let turn_loop = TurnLoop::new(
            provider,
            BuiltInToolRegistry::new(),
            Arc::new(PendingApprovalEvaluator),
        );

        let result = turn_loop
            .run_turn(TurnRequest {
                session_id: SessionId::new(),
                project_root: Utf8PathBuf::from("."),
                request: CompletionRequest {
                    connection_id: ConnectionId::new("local"),
                    model: "test".to_owned(),
                    system_prompt: None,
                    messages: vec![bt_core::Message::text(Role::User, "write a file")],
                    tools: BuiltInToolRegistry::new().specs(),
                    structured_output: None,
                    max_tokens: None,
                    temperature: None,
                    thinking: None,
                },
            })
            .await
            .expect("turn should pause");

        assert_eq!(result.approval_requests.len(), 1);
        assert!(result.tool_results.is_empty());
        assert!(result.messages.iter().any(|message| {
            message
                .tool_call()
                .is_some_and(|call| call.call_id == "call-pending")
        }));
        assert!(!result.messages.iter().any(|message| {
            message
                .tool_call()
                .is_some_and(|call| call.call_id == "call-later")
        }));
    }

    #[tokio::test]
    async fn tool_execution_injects_call_id_for_built_in_tools() {
        let root = TempDir::new().expect("tempdir");
        let project_root =
            Utf8PathBuf::from_path_buf(root.path().to_path_buf()).expect("utf8 path");
        let provider = Arc::new(MockProvider::new(vec![
            vec![tool_call_chunk(
                "call-2",
                "write",
                json!({
                    "path": "note.txt",
                    "content": "hello"
                }),
            )],
            vec![text_chunk("done")],
        ]));
        let turn_loop = TurnLoop::new(
            provider,
            BuiltInToolRegistry::new(),
            Arc::new(AlwaysApproveEvaluator),
        );

        let result = turn_loop
            .run_turn(TurnRequest {
                session_id: SessionId::new(),
                project_root: project_root.clone(),
                request: CompletionRequest {
                    connection_id: ConnectionId::new("local"),
                    model: "test".to_owned(),
                    system_prompt: None,
                    messages: vec![bt_core::Message::text(Role::User, "write a file")],
                    tools: BuiltInToolRegistry::new().specs(),
                    structured_output: None,
                    max_tokens: None,
                    temperature: None,
                    thinking: None,
                },
            })
            .await
            .expect("turn should complete");

        assert_eq!(result.approvals.len(), 1);
        assert!(!result.tool_results[0].is_error);
        assert_eq!(
            std::fs::read_to_string(project_root.join("note.txt").as_std_path()).expect("file"),
            "hello"
        );
    }

    #[tokio::test]
    async fn model_supplied_call_id_is_ignored_for_execution_and_replay() {
        let root = TempDir::new().expect("tempdir");
        let project_root =
            Utf8PathBuf::from_path_buf(root.path().to_path_buf()).expect("utf8 path");
        let provider = Arc::new(MockProvider::new(vec![
            vec![tool_call_chunk(
                "call-provider",
                "write",
                json!({
                    "path": "note.txt",
                    "content": "hello",
                    "call_id": "call-model"
                }),
            )],
            vec![text_chunk("done")],
        ]));
        let turn_loop = TurnLoop::new(
            provider,
            BuiltInToolRegistry::new(),
            Arc::new(AlwaysApproveEvaluator),
        );

        let result = turn_loop
            .run_turn(TurnRequest {
                session_id: SessionId::new(),
                project_root: project_root.clone(),
                request: CompletionRequest {
                    connection_id: ConnectionId::new("local"),
                    model: "test".to_owned(),
                    system_prompt: None,
                    messages: vec![bt_core::Message::text(Role::User, "write a file")],
                    tools: BuiltInToolRegistry::new().specs(),
                    structured_output: None,
                    max_tokens: None,
                    temperature: None,
                    thinking: None,
                },
            })
            .await
            .expect("turn should complete");

        let tool_call = result
            .messages
            .iter()
            .find_map(|message| message.tool_call())
            .expect("tool call message");
        assert_eq!(tool_call.call_id, "call-provider");
        assert!(tool_call.arguments.get("call_id").is_none());
        assert_eq!(result.tool_results[0].call_id.to_string(), "call-provider");
        assert_eq!(
            std::fs::read_to_string(project_root.join("note.txt").as_std_path()).expect("file"),
            "hello"
        );
    }

    #[tokio::test]
    async fn turn_usage_sums_across_follow_up_model_calls() {
        let root = TempDir::new().expect("tempdir");
        let project_root =
            Utf8PathBuf::from_path_buf(root.path().to_path_buf()).expect("utf8 path");
        let provider = Arc::new(MockProvider::new(vec![
            vec![CompletionChunk {
                llm_call_ordinal: None,
                deltas: vec![
                    CompletionDelta::OpenToolCall {
                        call_id: "call-usage".to_owned(),
                        tool_name: "write".to_owned(),
                        arguments: Some(json!({
                            "path": "note.txt",
                            "content": "hello"
                        })),
                    },
                    CompletionDelta::CloseToolCall {
                        call_id: "call-usage".to_owned(),
                    },
                ],
                usage: Some(bt_core::TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 2,
                    total_tokens: 12,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
                raw: None,
            }],
            vec![CompletionChunk {
                llm_call_ordinal: None,
                deltas: vec![CompletionDelta::AppendText {
                    text: "done".to_owned(),
                }],
                usage: Some(bt_core::TokenUsage {
                    prompt_tokens: 4,
                    completion_tokens: 6,
                    total_tokens: 10,
                    cache_read_tokens: Some(1),
                    cache_write_tokens: None,
                    reasoning_tokens: Some(2),
                }),
                raw: None,
            }],
        ]));
        let turn_loop = TurnLoop::new(
            provider,
            BuiltInToolRegistry::new(),
            Arc::new(AlwaysApproveEvaluator),
        );

        let result = turn_loop
            .run_turn(TurnRequest {
                session_id: SessionId::new(),
                project_root,
                request: CompletionRequest {
                    connection_id: ConnectionId::new("local"),
                    model: "test".to_owned(),
                    system_prompt: None,
                    messages: vec![bt_core::Message::text(Role::User, "write a file")],
                    tools: BuiltInToolRegistry::new().specs(),
                    structured_output: None,
                    max_tokens: None,
                    temperature: None,
                    thinking: None,
                },
            })
            .await
            .expect("turn should complete");

        let usage = result.usage.expect("usage should be aggregated");
        assert_eq!(usage.prompt_tokens, 14);
        assert_eq!(usage.completion_tokens, 8);
        assert_eq!(usage.total_tokens, 22);
        assert_eq!(usage.cache_read_tokens, Some(1));
        assert_eq!(usage.reasoning_tokens, Some(2));
        let ordinals = result
            .completion_chunks
            .iter()
            .map(|chunk| chunk.llm_call_ordinal)
            .collect::<Vec<_>>();
        assert_eq!(ordinals, vec![Some(1), Some(2)]);
    }

    #[tokio::test]
    async fn user_input_tool_is_reported_in_turn_result() {
        let provider = Arc::new(MockProvider::new(vec![vec![tool_call_chunk(
            "call-ask",
            "ask",
            json!({
                "question": "Which file should I inspect?",
                "choices": ["src/lib.rs", "src/main.rs"]
            }),
        )]]));
        let mut tools = BuiltInToolRegistry::new();
        tools.register(AskTool);
        let turn_loop = TurnLoop::new(provider, tools.clone(), Arc::new(AlwaysApproveEvaluator));

        let result = turn_loop
            .run_turn(TurnRequest {
                session_id: SessionId::new(),
                project_root: Utf8PathBuf::from("."),
                request: CompletionRequest {
                    connection_id: ConnectionId::new("local"),
                    model: "test".to_owned(),
                    system_prompt: None,
                    messages: vec![bt_core::Message::text(Role::User, "ask me something")],
                    tools: tools.specs(),
                    structured_output: None,
                    max_tokens: None,
                    temperature: None,
                    thinking: None,
                },
            })
            .await
            .expect("turn should pause for input");

        assert!(result.approval_requests.is_empty());
        assert!(result.tool_results.is_empty());
        assert_eq!(result.input_requests.len(), 1);
        assert_eq!(result.input_requests[0].tool_name, "ask");
        assert_eq!(
            result.input_requests[0].prompt,
            "Which file should I inspect?"
        );
        assert_eq!(
            result.input_requests[0].choices,
            vec!["src/lib.rs".to_owned(), "src/main.rs".to_owned()]
        );
        assert!(result.messages.iter().any(|message| {
            message
                .tool_call()
                .is_some_and(|call| call.call_id == "call-ask" && call.tool_name == "ask")
        }));
    }

    #[tokio::test]
    async fn user_input_tool_suspends_later_tool_calls_in_same_response() {
        let provider = Arc::new(MockProvider::new(vec![vec![
            tool_call_chunk(
                "call-ask",
                "ask",
                json!({
                    "question": "Which file should I inspect?",
                    "choices": ["src/lib.rs", "src/main.rs"]
                }),
            ),
            tool_call_chunk(
                "call-later",
                "read",
                json!({
                    "path": "src/lib.rs"
                }),
            ),
        ]]));
        let mut tools = BuiltInToolRegistry::new();
        tools.register(AskTool);
        let turn_loop = TurnLoop::new(provider, tools.clone(), Arc::new(AlwaysApproveEvaluator));

        let result = turn_loop
            .run_turn(TurnRequest {
                session_id: SessionId::new(),
                project_root: Utf8PathBuf::from("."),
                request: CompletionRequest {
                    connection_id: ConnectionId::new("local"),
                    model: "test".to_owned(),
                    system_prompt: None,
                    messages: vec![bt_core::Message::text(Role::User, "ask me something")],
                    tools: tools.specs(),
                    structured_output: None,
                    max_tokens: None,
                    temperature: None,
                    thinking: None,
                },
            })
            .await
            .expect("turn should pause for input");

        assert_eq!(result.input_requests.len(), 1);
        assert!(result.tool_results.is_empty());
        assert!(result.messages.iter().any(|message| {
            message
                .tool_call()
                .is_some_and(|call| call.call_id == "call-ask")
        }));
        assert!(!result.messages.iter().any(|message| {
            message
                .tool_call()
                .is_some_and(|call| call.call_id == "call-later")
        }));
    }

    #[tokio::test]
    async fn invalid_user_input_is_rejected_before_canonical_request_observation() {
        let provider = Arc::new(MockProvider::new(vec![vec![tool_call_chunk(
            "call-invalid-ask",
            "ask",
            json!({"choices": ["one", "two"]}),
        )]]));
        let mut tools = BuiltInToolRegistry::new();
        tools.register(AskTool);
        let turn_loop = TurnLoop::new(provider, tools.clone(), Arc::new(AlwaysApproveEvaluator));
        let mut observer = TestToolLifecycleObserver::new(false, false);

        let error = turn_loop
            .run_turn_with_tool_observer(
                TurnRequest {
                    session_id: SessionId::new(),
                    project_root: Utf8PathBuf::from("."),
                    request: CompletionRequest {
                        connection_id: ConnectionId::new("local"),
                        model: "test".to_owned(),
                        system_prompt: None,
                        messages: vec![bt_core::Message::text(Role::User, "ask me something")],
                        tools: tools.specs(),
                        structured_output: None,
                        max_tokens: None,
                        temperature: None,
                        thinking: None,
                    },
                },
                |_| Ok(()),
                |_| Ok(()),
                |_| Ok(()),
                &mut observer,
            )
            .await
            .expect_err("invalid ask arguments should fail the turn");

        assert!(matches!(error, BelltowerError::Tool(_)));
        assert!(observer.events.lock().expect("lock").is_empty());
    }
}
