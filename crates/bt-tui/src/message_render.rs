use super::*;

pub(super) fn render_message(message: &Message) -> String {
    render_message_with_options(message, false, TranscriptDensity::Verbose, 80)
}

pub(super) fn render_message_preview(message: &Message) -> String {
    let rendered = message
        .parts
        .iter()
        .map(|part| render_message_part_with_options(part, false, TranscriptDensity::Normal))
        .collect::<Vec<_>>()
        .join(" ");
    if rendered.is_empty() {
        format!("[{:?}] <empty>", message.role)
    } else {
        format!(
            "[{:?}] {}",
            message.role,
            sanitize_inline_preview(&rendered)
        )
    }
}

fn sanitize_inline_preview(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

pub(super) fn user_message_history_entry(message: &Message) -> Option<String> {
    if message.role != Role::User {
        return None;
    }

    let text = message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.trim_end_matches(['\n', '\r'])),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    (!text.trim().is_empty()).then_some(text)
}

pub(super) fn assistant_markdown_source(message: &Message) -> Option<String> {
    if message.role != Role::Assistant || message_is_ask_exchange_only(message) {
        return None;
    }

    let mut blocks = Vec::new();
    for part in &message.parts {
        match part {
            MessagePart::Text { text }
            | MessagePart::Refusal {
                text: Some(text), ..
            } => {
                if !text.trim().is_empty() {
                    blocks.push(text.as_str());
                }
            }
            MessagePart::Refusal { text: None, .. } => {}
            _ => return None,
        }
    }

    let source = blocks.join("\n\n");
    (!source.trim().is_empty()).then_some(source)
}

pub(super) fn render_message_with_options(
    message: &Message,
    show_reasoning: bool,
    transcript_density: TranscriptDensity,
    view_width: u16,
) -> String {
    render_transcript_message_with_options(message, show_reasoning, transcript_density, view_width)
        .unwrap_or_else(|| format!("[{:?}] <empty>", message.role))
}

pub(super) fn render_transcript_message_with_options(
    message: &Message,
    show_reasoning: bool,
    transcript_density: TranscriptDensity,
    view_width: u16,
) -> Option<String> {
    render_transcript_parts_with_options(
        message.role.clone(),
        &message.parts,
        show_reasoning,
        transcript_density,
        view_width,
    )
}

pub(super) fn render_transcript_parts_with_options(
    role: Role,
    parts: &[MessagePart],
    show_reasoning: bool,
    transcript_density: TranscriptDensity,
    view_width: u16,
) -> Option<String> {
    let rendered = match role {
        Role::User => {
            render_user_transcript_parts(parts, show_reasoning, transcript_density, view_width)
        }
        Role::Assistant => {
            render_assistant_transcript_parts(parts, show_reasoning, transcript_density, view_width)
        }
        Role::Tool => {
            render_tool_transcript_parts(parts, show_reasoning, transcript_density, view_width)
        }
        _ => {
            let rendered = parts
                .iter()
                .map(|part| {
                    render_message_part_with_options(part, show_reasoning, transcript_density)
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            (!rendered.trim().is_empty()).then_some(format!("[{:?}] {}", role, rendered))
        }
    };

    rendered.filter(|rendered| !rendered.trim().is_empty())
}

pub(super) fn render_compact_transcript_entry(
    first_prefix: &str,
    continuation_prefix: &str,
    body: &str,
) -> String {
    let mut rendered = Vec::new();
    let mut lines = body.lines();
    let first_line = lines.next().unwrap_or_default();
    rendered.push(format!("{first_prefix}{first_line}"));
    rendered.extend(lines.map(|line| format!("{continuation_prefix}{line}")));
    rendered.join("\n")
}

pub(super) fn render_wrapped_prefixed_entry(
    first_prefix: &str,
    continuation_prefix: &str,
    body: &str,
    view_width: u16,
) -> String {
    let width = usize::from(view_width.max(1));
    let first_width = width.saturating_sub(display_width(first_prefix)).max(1);
    let continuation_width = width
        .saturating_sub(display_width(continuation_prefix))
        .max(1);
    let mut rendered = Vec::new();
    let mut first_visual_line = true;

    if body.is_empty() {
        return first_prefix.trim_end().to_owned();
    }

    for raw_line in body.split('\n') {
        let wrapped = if raw_line.is_empty() {
            vec![String::new()]
        } else if first_visual_line {
            wrap_plain_text(raw_line, first_width)
        } else {
            wrap_plain_text(raw_line, continuation_width)
        };

        for segment in wrapped {
            let prefix = if first_visual_line {
                first_prefix
            } else {
                continuation_prefix
            };
            if segment.is_empty() {
                rendered.push(prefix.trim_end().to_owned());
            } else {
                rendered.push(format!("{prefix}{segment}"));
            }
            first_visual_line = false;
        }
    }

    rendered.join("\n")
}

fn render_tool_header(verb: &str, tool_name: &str, call_id: &str, detail: Option<&str>) -> String {
    match detail {
        Some(detail) if !detail.trim().is_empty() => {
            format!("{verb} {tool_name} {detail} · {call_id}")
        }
        _ => format!("{verb} {tool_name} · {call_id}"),
    }
}

fn render_labeled_transcript_entry(label: &str, body: &str, view_width: u16) -> String {
    let continuation = " ".repeat(display_width(label));
    render_wrapped_prefixed_entry(label, &continuation, body, view_width)
}

fn render_ask_question_entry(detail: Option<String>, view_width: u16) -> String {
    let question = detail.unwrap_or_else(|| "Agent is waiting for input.".to_owned());
    render_labeled_transcript_entry("Question: ", &question, view_width)
}

pub(super) fn ask_question_from_call(call: &bt_core::ToolCall) -> Option<String> {
    call.arguments
        .get("question")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|question| !question.is_empty())
        .map(str::to_owned)
}

pub(super) fn ask_answer_from_result(result: &bt_core::ToolResultEnvelope) -> Option<String> {
    result
        .output
        .get("response")
        .and_then(|response| {
            response
                .as_str()
                .map(str::to_owned)
                .or_else(|| Some(response.to_string()))
        })
        .map(|answer| answer.trim().to_owned())
        .filter(|answer| !answer.is_empty())
}

pub(super) fn render_ask_history_entry(
    question: Option<&str>,
    answer: Option<&str>,
    view_width: u16,
) -> String {
    let mut blocks = vec![if answer.is_some() {
        "• Questions 1/1 answered".to_owned()
    } else {
        "• Questions 0/1 answered".to_owned()
    }];

    if let Some(question) = question.filter(|question| !question.trim().is_empty()) {
        blocks.push(render_wrapped_prefixed_entry(
            "  • ", "    ", question, view_width,
        ));
    }

    if let Some(answer) = answer.filter(|answer| !answer.trim().is_empty()) {
        blocks.push(render_wrapped_prefixed_entry(
            "    answer: ",
            "            ",
            answer,
            view_width,
        ));
    }

    blocks.join("\n")
}

fn render_ask_answer_entry(result: &bt_core::ToolResultEnvelope, view_width: u16) -> String {
    let answer = ask_answer_from_result(result).unwrap_or_else(|| "User input recorded".to_owned());
    render_labeled_transcript_entry("Answer: ", &answer, view_width)
}

fn render_tool_result_detail(result: &bt_core::ToolResultEnvelope) -> String {
    if result.tool_name == "ask" {
        return result
            .output
            .get("response")
            .and_then(|response| {
                response
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| Some(response.to_string()))
            })
            .unwrap_or_else(|| "User input recorded".to_owned());
    }

    if result.is_error {
        return summarize_tool_result_output(&result.tool_name, &result.output)
            .map(|summary| format!("Error: {summary}"))
            .unwrap_or_else(|| format!("Error: {}", truncate_detail(&result.output.to_string())));
    }

    summarize_tool_result_output(&result.tool_name, &result.output)
        .unwrap_or_else(|| "Completed".to_owned())
}

fn render_tool_result_body(result: &bt_core::ToolResultEnvelope) -> String {
    let detail = render_tool_result_detail(result);
    if result.tool_name == "ask" {
        detail
    } else if detail.trim().is_empty() {
        result.tool_name.clone()
    } else {
        format!("{}: {detail}", result.tool_name)
    }
}

fn render_user_transcript_parts(
    parts: &[MessagePart],
    _show_reasoning: bool,
    _transcript_density: TranscriptDensity,
    view_width: u16,
) -> Option<String> {
    let rendered = parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    (!rendered.trim().is_empty()).then(|| {
        render_wrapped_prefixed_entry(
            COMPOSER_PREFIX,
            COMPOSER_CONTINUATION_PREFIX,
            &rendered,
            view_width,
        )
    })
}

fn render_assistant_transcript_parts(
    parts: &[MessagePart],
    show_reasoning: bool,
    transcript_density: TranscriptDensity,
    view_width: u16,
) -> Option<String> {
    let mut blocks: Vec<(TranscriptEntryKind, String)> = Vec::new();

    for part in parts {
        match part {
            MessagePart::ToolCall { call } if call.tool_name == "ask" => {
                blocks.push((
                    TranscriptEntryKind::Assistant,
                    render_ask_question_entry(
                        summarize_tool_detail(&call.tool_name, &call.arguments),
                        view_width,
                    ),
                ));
            }
            MessagePart::ToolCall { call } => {
                blocks.push((
                    TranscriptEntryKind::ToolCall,
                    render_committed_tool_call(
                        &call.tool_name,
                        &call.call_id,
                        summarize_tool_detail(&call.tool_name, &call.arguments).as_deref(),
                        view_width,
                    ),
                ));
            }
            MessagePart::ToolResult { result } if result.tool_name == "ask" => {
                blocks.push((
                    TranscriptEntryKind::Assistant,
                    render_ask_answer_entry(result, view_width),
                ));
            }
            MessagePart::ToolResult { result } => {
                blocks.push((
                    TranscriptEntryKind::ToolResult,
                    render_tool_result_entry(result, view_width),
                ));
            }
            _ => {
                let rendered = match part {
                    MessagePart::Text { text } => render_assistant_markdown_block(text, view_width),
                    _ => render_message_part_with_options(part, show_reasoning, transcript_density),
                };
                if !rendered.trim().is_empty() {
                    blocks.push((TranscriptEntryKind::Assistant, rendered));
                }
            }
        }
    }

    join_transcript_blocks(blocks)
}

fn render_assistant_markdown_block(text: &str, view_width: u16) -> String {
    crate::markdown::render_markdown_to_string(text, Some(usize::from(view_width.max(1))), None)
}

fn render_tool_transcript_parts(
    parts: &[MessagePart],
    show_reasoning: bool,
    transcript_density: TranscriptDensity,
    view_width: u16,
) -> Option<String> {
    let mut blocks: Vec<(TranscriptEntryKind, String)> = Vec::new();

    for part in parts {
        match part {
            MessagePart::ToolResult { result } => {
                blocks.push((
                    TranscriptEntryKind::ToolResult,
                    render_tool_result_entry(result, view_width),
                ));
            }
            _ => {
                let rendered =
                    render_message_part_with_options(part, show_reasoning, transcript_density);
                if !rendered.trim().is_empty() {
                    blocks.push((TranscriptEntryKind::Assistant, rendered));
                }
            }
        }
    }

    join_transcript_blocks(blocks)
}

pub(super) fn join_transcript_blocks(blocks: Vec<(TranscriptEntryKind, String)>) -> Option<String> {
    let mut rendered = String::new();
    let mut previous_kind = None;

    for (kind, block) in blocks {
        if block.trim().is_empty() {
            continue;
        }

        if !rendered.is_empty() {
            match transcript_separator_between(previous_kind, kind) {
                ScrollbackSeparator::Paragraph => rendered.push_str("\n\n"),
                ScrollbackSeparator::Line => rendered.push('\n'),
                ScrollbackSeparator::None => {}
            }
        }

        rendered.push_str(&block);
        previous_kind = Some(kind);
    }

    (!rendered.trim().is_empty()).then_some(rendered)
}

pub(super) fn transcript_entry_kind_for_message(message: &Message) -> TranscriptEntryKind {
    if message_is_ask_exchange_only(message) {
        return TranscriptEntryKind::Assistant;
    }

    match message.role {
        Role::User => TranscriptEntryKind::User,
        Role::Tool => TranscriptEntryKind::ToolResult,
        Role::Assistant => {
            let has_tool_call = message
                .parts
                .iter()
                .any(|part| matches!(part, MessagePart::ToolCall { .. }));
            let has_tool_result = message
                .parts
                .iter()
                .any(|part| matches!(part, MessagePart::ToolResult { .. }));
            if has_tool_call && !has_tool_result {
                TranscriptEntryKind::ToolCall
            } else if has_tool_result {
                TranscriptEntryKind::ToolResult
            } else {
                TranscriptEntryKind::Assistant
            }
        }
        _ => TranscriptEntryKind::Assistant,
    }
}

pub(super) fn message_is_ask_exchange_only(message: &Message) -> bool {
    if message.parts.is_empty() {
        return false;
    }

    match message.role {
        Role::Assistant => message.parts.iter().all(|part| match part {
            MessagePart::ToolCall { call } => call.tool_name == "ask",
            MessagePart::ToolResult { result } => result.tool_name == "ask",
            MessagePart::Text { text } => text.trim().is_empty(),
            _ => false,
        }),
        Role::Tool => message.parts.iter().all(|part| match part {
            MessagePart::ToolResult { result } => result.tool_name == "ask",
            MessagePart::Text { text } => text.trim().is_empty(),
            _ => false,
        }),
        _ => false,
    }
}

pub(super) fn transcript_separator_between(
    previous: Option<TranscriptEntryKind>,
    current: TranscriptEntryKind,
) -> ScrollbackSeparator {
    let Some(previous) = previous else {
        return ScrollbackSeparator::None;
    };

    match current {
        TranscriptEntryKind::Operator | TranscriptEntryKind::LocalError => {
            ScrollbackSeparator::Line
        }
        // User cells already include Codex-style top/bottom padding and
        // composer-background styling. Adding a paragraph separator here creates
        // an extra unstyled blank band between the previous response and the
        // committed prompt.
        TranscriptEntryKind::User => ScrollbackSeparator::Line,
        TranscriptEntryKind::Assistant => match previous {
            TranscriptEntryKind::ToolCall | TranscriptEntryKind::ToolResult => {
                ScrollbackSeparator::Paragraph
            }
            _ => ScrollbackSeparator::Paragraph,
        },
        TranscriptEntryKind::ToolCall => match previous {
            TranscriptEntryKind::User => ScrollbackSeparator::Paragraph,
            TranscriptEntryKind::Assistant
            | TranscriptEntryKind::ToolCall
            | TranscriptEntryKind::ToolResult => ScrollbackSeparator::Line,
            TranscriptEntryKind::Operator | TranscriptEntryKind::LocalError => {
                ScrollbackSeparator::Paragraph
            }
        },
        TranscriptEntryKind::ToolResult => ScrollbackSeparator::Line,
    }
}

pub(super) fn render_compact_operator_entry(header: &str, body: &str) -> String {
    if body.trim().is_empty() {
        return header.to_owned();
    }

    format!(
        "{header}\n{}",
        render_compact_transcript_entry(
            COMPOSER_CONTINUATION_PREFIX,
            COMPOSER_CONTINUATION_PREFIX,
            body,
        )
    )
}

pub(super) fn render_message_part_with_options(
    part: &MessagePart,
    show_reasoning: bool,
    transcript_density: TranscriptDensity,
) -> String {
    match part {
        MessagePart::Text { text } => text.clone(),
        MessagePart::Reasoning {
            text: Some(text),
            redacted: false,
            ..
        } if show_reasoning => format!("[Thinking] {text}"),
        MessagePart::Reasoning {
            text: None,
            redacted: false,
            ..
        } if show_reasoning => "[Thinking]".to_owned(),
        MessagePart::Reasoning { redacted: true, .. } => "[Thinking redacted]".to_owned(),
        MessagePart::Reasoning { .. } => "[Thinking hidden]".to_owned(),
        MessagePart::ToolCall { call } if call.tool_name == "ask" => {
            summarize_tool_detail(&call.tool_name, &call.arguments)
                .map(|detail| format!("Question: {detail}"))
                .unwrap_or_else(|| "Question".to_owned())
        }
        MessagePart::ToolCall { call } => render_tool_call_preview(
            &call.tool_name,
            &call.call_id,
            summarize_tool_detail(&call.tool_name, &call.arguments).as_deref(),
        ),
        MessagePart::ToolResult { result } if result.tool_name == "ask" => result
            .output
            .get("response")
            .and_then(|response| {
                response
                    .as_str()
                    .map(|response| format!("Answer: {response}"))
                    .or_else(|| Some(format!("Answer: {response}")))
            })
            .unwrap_or_else(|| "Answer".to_owned()),
        MessagePart::ToolResult { result } => summarize_tool_result(result),
        MessagePart::Refusal {
            text: Some(text),
            provider_reason,
            ..
        } => format!(
            "{} {text}",
            format_refusal_prefix(provider_reason.as_deref(), true)
        ),
        MessagePart::Refusal {
            text: None,
            provider_reason,
            ..
        } => format_refusal_prefix(provider_reason.as_deref(), false),
        MessagePart::Structured { schema_name, value }
            if matches!(transcript_density, TranscriptDensity::Verbose) =>
        {
            format_structured_output(schema_name.as_deref(), value)
        }
        MessagePart::Structured { schema_name, value } => {
            summarize_structured_output(schema_name.as_deref(), value)
        }
    }
}

pub(super) fn render_tool_call_preview(
    tool_name: &str,
    call_id: &str,
    detail: Option<&str>,
) -> String {
    render_tool_activity_entry(
        "Calling",
        tool_name,
        call_id,
        detail,
        DEFAULT_TEXT_VIEW_WIDTH,
    )
}

pub(super) fn render_committed_tool_call(
    tool_name: &str,
    call_id: &str,
    detail: Option<&str>,
    view_width: u16,
) -> String {
    render_tool_activity_entry("Called", tool_name, call_id, detail, view_width)
}

pub(super) fn render_tool_activity_entry(
    verb: &str,
    tool_name: &str,
    call_id: &str,
    detail: Option<&str>,
    view_width: u16,
) -> String {
    render_wrapped_prefixed_entry(
        "• ",
        "  ",
        &render_tool_header(verb, tool_name, call_id, detail),
        view_width,
    )
}

pub(super) fn render_tool_result_entry(
    result: &bt_core::ToolResultEnvelope,
    view_width: u16,
) -> String {
    render_wrapped_prefixed_entry("  └ ", "    ", &render_tool_result_body(result), view_width)
}

pub(super) fn render_tool_block_entry(
    verb: &str,
    tool_name: &str,
    call_id: &str,
    detail: Option<&str>,
    result: Option<&bt_core::ToolResultEnvelope>,
    view_width: u16,
) -> String {
    let mut rendered = render_tool_activity_entry(verb, tool_name, call_id, detail, view_width);
    if let Some(result) = result {
        rendered.push('\n');
        rendered.push_str(&render_tool_result_entry(result, view_width));
    }
    rendered
}

pub(super) fn summarize_tool_result(result: &bt_core::ToolResultEnvelope) -> String {
    render_tool_result_entry(result, DEFAULT_TEXT_VIEW_WIDTH)
}

fn summarize_tool_result_output(tool_name: &str, output: &serde_json::Value) -> Option<String> {
    match tool_name {
        "list" => output
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .map(|entries| format!("{} entries", entries.len())),
        "read" | "write" | "edit" => output
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(truncate_detail),
        "search" => output
            .get("matches")
            .and_then(serde_json::Value::as_array)
            .map(|matches| format!("{} matches", matches.len())),
        "shell" => output
            .get("status")
            .and_then(serde_json::Value::as_i64)
            .map(|status| format!("status={status}")),
        "inspect" => output
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(truncate_detail),
        "web_search" => output
            .get("result_count")
            .and_then(serde_json::Value::as_u64)
            .map(|count| format!("{count} results")),
        "fetch" | "web_fetch" => output
            .get("status")
            .and_then(serde_json::Value::as_u64)
            .map(|status| format!("status={status}")),
        _ => None,
    }
}

fn summarize_structured_output(schema_name: Option<&str>, value: &serde_json::Value) -> String {
    let prefix = schema_name
        .map(|name| format!("[Structured: {name}]"))
        .unwrap_or_else(|| "[Structured]".to_owned());
    let summary = match value {
        serde_json::Value::Object(map) => format!("{} keys", map.len()),
        serde_json::Value::Array(items) => format!("{} items", items.len()),
        other => truncate_detail(&other.to_string()),
    };
    format!("{prefix} {summary}")
}

pub(super) fn visible_streaming_delta_text(
    delta: &CompletionDelta,
    show_reasoning: bool,
    transcript_density: TranscriptDensity,
) -> Option<String> {
    match delta {
        CompletionDelta::AppendText { text } => Some(text.clone()),
        CompletionDelta::AppendReasoning {
            text: Some(text),
            redacted: false,
            ..
        } if show_reasoning && matches!(transcript_density, TranscriptDensity::Verbose) => {
            Some(format!("[Thinking] {text}"))
        }
        CompletionDelta::AppendReasoning {
            text: Some(text),
            redacted: false,
            ..
        } if show_reasoning => Some(format!("[Thinking] {text}")),
        CompletionDelta::AppendReasoning {
            text: None,
            redacted: false,
            ..
        } if show_reasoning => Some("[Thinking]".to_owned()),
        CompletionDelta::AppendReasoning { redacted: true, .. } => {
            Some("[Thinking redacted]".to_owned())
        }
        CompletionDelta::AppendRefusal {
            text: Some(text),
            provider_reason,
            ..
        } => Some(format!(
            "{} {text}",
            format_refusal_prefix(provider_reason.as_deref(), true)
        )),
        CompletionDelta::SetStructuredOutput { schema_name, value } => {
            Some(format_structured_output(schema_name.as_deref(), value))
        }
        CompletionDelta::AppendReasoning { .. }
        | CompletionDelta::OpenToolCall { .. }
        | CompletionDelta::AppendToolCallArguments { .. }
        | CompletionDelta::CloseToolCall { .. }
        | CompletionDelta::AppendRefusal { text: None, .. } => None,
    }
}

pub(super) fn completion_delta_text_preview(deltas: &[CompletionDelta]) -> Option<String> {
    let preview = deltas
        .iter()
        .filter_map(|delta| match delta {
            CompletionDelta::AppendText { text } => Some(text.clone()),
            CompletionDelta::AppendReasoning {
                text: Some(text),
                redacted: false,
                ..
            } => Some(format!("[Thinking] {text}")),
            CompletionDelta::AppendReasoning {
                text: None,
                redacted: false,
                ..
            } => Some("[Thinking]".to_owned()),
            CompletionDelta::AppendReasoning { redacted: true, .. } => {
                Some("[Thinking redacted]".to_owned())
            }
            CompletionDelta::AppendRefusal {
                text: Some(text),
                provider_reason,
                ..
            } => Some(format!(
                "{} {text}",
                format_refusal_prefix(provider_reason.as_deref(), true)
            )),
            CompletionDelta::SetStructuredOutput { schema_name, value } => {
                Some(format_structured_output(schema_name.as_deref(), value))
            }
            CompletionDelta::OpenToolCall { .. }
            | CompletionDelta::AppendToolCallArguments { .. }
            | CompletionDelta::CloseToolCall { .. }
            | CompletionDelta::AppendRefusal { text: None, .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!preview.is_empty()).then_some(preview)
}

pub(super) fn completion_delta_tool_preview(deltas: &[CompletionDelta]) -> Option<String> {
    deltas.iter().find_map(|delta| match delta {
        CompletionDelta::OpenToolCall {
            call_id,
            tool_name,
            arguments,
        } => Some(format!(
            "{} {} {}",
            tool_name,
            call_id,
            arguments
                .as_ref()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "{}".to_owned())
        )),
        CompletionDelta::AppendToolCallArguments {
            call_id,
            partial_json,
        } => Some(format!("{call_id} {partial_json}")),
        CompletionDelta::CloseToolCall { call_id } => Some(format!("closed {call_id}")),
        CompletionDelta::AppendText { .. }
        | CompletionDelta::AppendReasoning { .. }
        | CompletionDelta::AppendRefusal { .. }
        | CompletionDelta::SetStructuredOutput { .. } => None,
    })
}

fn format_refusal_prefix(provider_reason: Option<&str>, include_brackets: bool) -> String {
    let label = match provider_reason {
        Some(reason) => format!("Refusal: {reason}"),
        None => "Refusal".to_owned(),
    };
    if include_brackets {
        format!("[{label}]")
    } else {
        label
    }
}

fn format_structured_output(schema_name: Option<&str>, value: &serde_json::Value) -> String {
    let prefix = schema_name
        .map(|name| format!("[Structured: {name}]"))
        .unwrap_or_else(|| "[Structured]".to_owned());
    format!("{prefix} {value}")
}
