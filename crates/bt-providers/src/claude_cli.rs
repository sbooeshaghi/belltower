//! Subscription-backed Claude provider that subprocesses the local
//! Claude Code CLI (`claude -p --output-format stream-json`).
//!
//! Auth is delegated entirely to the CLI's own login (the operator's Claude
//! subscription); belltower never sees or stores credentials. The CLI's
//! `stream_event` payloads are Anthropic Messages API events, so translation
//! reuses the anthropic provider's stream state machine verbatim.
//!
//! v1 is completion-only: harness tool specs are NOT bridged into the CLI
//! (the CLI's own tools are disallowed and the loop is capped at one turn),
//! and structured output is unsupported. Use the `anthropic` API connection
//! for tool-calling sessions until the MCP bridge lands.

use crate::anthropic::{AnthropicStreamState, completion_chunks_from_sse_event};
use crate::sse::SseEvent;
use async_stream::try_stream;
use bt_core::{
    BelltowerError, CompletionChunk, CompletionRequest, CompletionSummary, ConnectionStatus,
    Message, MessagePart, ModelPricing, Result, Role,
    traits::{BoxFuture, BoxStream, Provider},
};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const READ_STALL_TIMEOUT: Duration = Duration::from_secs(300);
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(10);

/// The CLI's own tools stay off: belltower owns tool execution and approvals,
/// and a print-mode subprocess must not mutate the workspace out of band.
const DISALLOWED_CLI_TOOLS: &str =
    "Bash,Edit,Write,Read,Glob,Grep,WebFetch,WebSearch,Task,NotebookEdit,TodoWrite";

const KNOWN_MODELS: &[&str] = &[
    "claude-haiku-4-5-20251001",
    "claude-sonnet-5",
    "claude-opus-4-8",
    "claude-fable-5",
];

pub struct ClaudeCliProvider {
    provider_id: String,
    binary: String,
}

impl ClaudeCliProvider {
    #[must_use]
    pub fn new(provider_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            binary: std::env::var("BELLTOWER_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_owned()),
        }
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        // The whole point of this provider is subscription auth through the
        // CLI's own login; never let ambient API-key env vars rebill it.
        command
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("ANTHROPIC_BASE_URL")
            .env_remove("ANTHROPIC_MODEL");
        command
    }
}

impl Provider for ClaudeCliProvider {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn validate(&self) -> BoxFuture<'_, Result<ConnectionStatus>> {
        Box::pin(async move {
            let probe = tokio::time::timeout(
                VALIDATE_TIMEOUT,
                self.base_command()
                    .arg("--version")
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output(),
            )
            .await;
            match probe {
                Ok(Ok(output)) if output.status.success() => Ok(ConnectionStatus::Healthy),
                Ok(Ok(output)) => Ok(ConnectionStatus::Degraded {
                    reason: format!("`{} --version` exited with {}", self.binary, output.status),
                }),
                Ok(Err(error)) => Ok(ConnectionStatus::Degraded {
                    reason: format!(
                        "claude CLI not runnable at `{}` (set BELLTOWER_CLAUDE_BIN): {error}",
                        self.binary
                    ),
                }),
                Err(_) => Ok(ConnectionStatus::Degraded {
                    reason: format!("`{} --version` timed out", self.binary),
                }),
            }
        })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<String>>> {
        Box::pin(async move {
            Ok(KNOWN_MODELS
                .iter()
                .map(|model| (*model).to_owned())
                .collect())
        })
    }

    fn stream_completion(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<BoxStream<Result<CompletionChunk>>>> {
        Box::pin(async move {
            if request.structured_output.is_some() {
                return Err(BelltowerError::Unsupported(
                    "structured output is not supported by the claude-cli provider".to_owned(),
                ));
            }
            if !request.tools.is_empty() {
                // Completion-only v1: declared harness tools cannot execute
                // through the CLI subprocess. The session still works for
                // conversational turns; the model simply has no callable
                // tools on this connection.
                tracing::warn!(
                    provider_id = %self.provider_id,
                    tool_count = request.tools.len(),
                    "claude-cli provider ignores declared harness tools (completion-only v1)"
                );
            }

            let prompt = render_prompt(&request.messages);
            let mut command = self.base_command();
            command
                .arg("-p")
                .arg("--output-format")
                .arg("stream-json")
                .arg("--include-partial-messages")
                .arg("--verbose")
                .arg("--max-turns")
                .arg("1")
                .arg("--disallowedTools")
                .arg(DISALLOWED_CLI_TOOLS)
                .arg("--model")
                .arg(&request.model);
            if let Some(system_prompt) = request
                .system_prompt
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                command.arg("--system-prompt").arg(system_prompt);
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| {
                    BelltowerError::Provider(format!(
                        "failed to launch claude CLI at `{}`: {error}",
                        self.binary
                    ))
                })?;

            let mut stdin = child.stdin.take().ok_or_else(|| {
                BelltowerError::Provider("claude CLI stdin unavailable".to_owned())
            })?;
            stdin.write_all(prompt.as_bytes()).await.map_err(|error| {
                BelltowerError::Provider(format!("failed to write claude CLI prompt: {error}"))
            })?;
            drop(stdin);
            let stdout = child.stdout.take().ok_or_else(|| {
                BelltowerError::Provider("claude CLI stdout unavailable".to_owned())
            })?;
            let stderr = child.stderr.take();

            let stream = try_stream! {
                let mut lines = BufReader::new(stdout).lines();
                let mut state = AnthropicStreamState::default();
                let mut saw_result = false;
                loop {
                    let line = tokio::time::timeout(READ_STALL_TIMEOUT, lines.next_line())
                        .await
                        .map_err(|_| {
                            BelltowerError::Provider(
                                "claude CLI stream stalled beyond the read timeout".to_owned(),
                            )
                        })?
                        .map_err(|error| {
                            BelltowerError::Provider(format!(
                                "failed reading claude CLI stream: {error}"
                            ))
                        })?;
                    let Some(line) = line else {
                        break;
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let envelope: Value = serde_json::from_str(&line).map_err(|error| {
                        BelltowerError::Provider(format!(
                            "claude CLI emitted invalid stream-json: {error}"
                        ))
                    })?;
                    match envelope.get("type").and_then(Value::as_str) {
                        Some("stream_event") => {
                            let Some(event) = envelope.get("event") else {
                                continue;
                            };
                            let sse = SseEvent {
                                event: event
                                    .get("type")
                                    .and_then(Value::as_str)
                                    .map(ToOwned::to_owned),
                                data: event.to_string(),
                            };
                            let mut chunks =
                                completion_chunks_from_sse_event(&sse, &mut state)?;
                            if let Some(first) = chunks.first_mut() {
                                first.raw = Some(envelope.clone());
                            }
                            for chunk in chunks {
                                yield chunk;
                            }
                        }
                        Some("result") => {
                            saw_result = true;
                            let is_error = envelope
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            let subtype = envelope
                                .get("subtype")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if is_error || subtype != "success" {
                                let detail = envelope
                                    .get("result")
                                    .and_then(Value::as_str)
                                    .unwrap_or(subtype);
                                Err(BelltowerError::Provider(format!(
                                    "claude CLI turn failed ({subtype}): {detail}"
                                )))?;
                            }
                            // Durable trailer: the CLI's own final accounting.
                            yield CompletionChunk {
                                llm_call_ordinal: None,
                                deltas: Vec::new(),
                                usage: state.usage_snapshot(),
                                raw: Some(envelope.clone()),
                            };
                        }
                        // init/status/rate-limit envelopes and full-message
                        // snapshots (already covered by deltas) are skipped.
                        _ => {}
                    }
                }

                let status = child.wait().await.map_err(|error| {
                    BelltowerError::Provider(format!("claude CLI wait failed: {error}"))
                })?;
                if !status.success() {
                    let mut detail = String::new();
                    if let Some(stderr) = stderr {
                        let mut reader = BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = reader.next_line().await {
                            if detail.len() > 800 {
                                break;
                            }
                            detail.push_str(&line);
                            detail.push('\n');
                        }
                    }
                    Err(BelltowerError::Provider(format!(
                        "claude CLI exited with {status}: {}",
                        detail.trim()
                    )))?;
                }
                if !saw_result {
                    Err(BelltowerError::Provider(
                        "claude CLI stream ended without a result envelope".to_owned(),
                    ))?;
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
            unpriced_reason: Some("billed through the Claude subscription".to_owned()),
        })
    }

    fn completion_summary(&self, summary: CompletionSummary) -> Result<CompletionSummary> {
        Ok(summary)
    }
}

/// Renders belltower's message history into one prompt for `claude -p`.
/// A single user message passes through verbatim; longer histories become a
/// labeled transcript the model continues.
fn render_prompt(messages: &[Message]) -> String {
    let only_user_text = match messages {
        [message] if message.role == Role::User => message_text(message),
        _ => None,
    };
    if let Some(text) = only_user_text {
        return text;
    }

    let mut prompt = String::from(
        "The following is the conversation so far. Continue it as the assistant; \
         reply with the assistant's next message only.\n",
    );
    for message in messages {
        let label = match message.role {
            Role::System => "System",
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::Tool => "Tool result",
        };
        for part in &message.parts {
            match part {
                MessagePart::Text { text } if !text.trim().is_empty() => {
                    prompt.push_str("\n");
                    prompt.push_str(label);
                    prompt.push_str(": ");
                    prompt.push_str(text);
                    prompt.push('\n');
                }
                MessagePart::ToolCall { call } => {
                    prompt.push_str(&format!(
                        "\nAssistant (tool call {}): {}\n",
                        call.tool_name, call.arguments
                    ));
                }
                MessagePart::ToolResult { result } => {
                    prompt.push_str(&format!(
                        "\nTool result ({}): {}\n",
                        result.tool_name, result.output
                    ));
                }
                _ => {}
            }
        }
    }
    prompt
}

fn message_text(message: &Message) -> Option<String> {
    let text = message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::CompletionDelta;

    /// Captured verbatim from `claude -p "Say exactly: FIXTURE_OK"
    /// --output-format stream-json --include-partial-messages` (v2.1.118).
    const FIXTURE: &str = include_str!("fixtures/claude_stream.ndjson");

    #[test]
    fn fixture_translates_to_text_reasoning_and_usage() {
        let mut state = AnthropicStreamState::default();
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut saw_usage = false;
        let mut saw_result = false;

        for line in FIXTURE.lines().filter(|line| !line.trim().is_empty()) {
            let envelope: Value = serde_json::from_str(line).expect("fixture line parses");
            match envelope.get("type").and_then(Value::as_str) {
                Some("stream_event") => {
                    let event = envelope.get("event").expect("stream event payload");
                    let sse = SseEvent {
                        event: event
                            .get("type")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                        data: event.to_string(),
                    };
                    for chunk in
                        completion_chunks_from_sse_event(&sse, &mut state).expect("translate")
                    {
                        if chunk.usage.is_some() {
                            saw_usage = true;
                        }
                        for delta in chunk.deltas {
                            match delta {
                                CompletionDelta::AppendText { text: t } => text.push_str(&t),
                                CompletionDelta::AppendReasoning { text: Some(t), .. } => {
                                    reasoning.push_str(&t)
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Some("result") => {
                    saw_result = true;
                    assert_eq!(
                        envelope.get("subtype").and_then(Value::as_str),
                        Some("success")
                    );
                    assert_eq!(
                        envelope.get("is_error").and_then(Value::as_bool),
                        Some(false)
                    );
                }
                _ => {}
            }
        }

        assert_eq!(text, "FIXTURE_OK");
        assert!(
            reasoning.contains("FIXTURE_OK"),
            "thinking deltas must translate to reasoning"
        );
        assert!(saw_usage, "message_delta usage must surface");
        assert!(saw_result, "fixture must contain the result envelope");

        let usage = state.usage_snapshot().expect("usage snapshot");
        assert_eq!(usage.completion_tokens, 58);
    }

    #[test]
    fn single_user_message_renders_verbatim() {
        let messages = vec![Message::text(Role::User, "hello world")];
        assert_eq!(render_prompt(&messages), "hello world");
    }

    #[test]
    fn history_renders_as_labeled_transcript() {
        let messages = vec![
            Message::text(Role::User, "first question"),
            Message::text(Role::Assistant, "first answer"),
            Message::text(Role::User, "follow-up"),
        ];
        let prompt = render_prompt(&messages);
        assert!(prompt.contains("User: first question"));
        assert!(prompt.contains("Assistant: first answer"));
        assert!(prompt.contains("User: follow-up"));
        assert!(prompt.starts_with("The following is the conversation so far"));
    }
}
