use super::*;

fn ask_question_for_call_id(messages: &[Message], call_id: &str) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        message.tool_call().and_then(|call| {
            (call.tool_name == "ask" && call.call_id == call_id)
                .then(|| ask_question_from_call(call))
                .flatten()
        })
    })
}

pub(super) fn compare_transcript_seq_ids(
    left: Option<i64>,
    right: Option<i64>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RenderedTranscriptEntry {
    pub(super) key: Option<String>,
    pub(super) tool_call_id: Option<String>,
    pub(super) seq_id: Option<i64>,
    pub(super) kind: TranscriptEntryKind,
    pub(super) rendered: String,
    pub(super) assistant_markdown_source: Option<String>,
    pub(super) line_count: usize,
    pub(super) allow_live_overflow: bool,
    pub(super) printable_in_scrollback: bool,
    pub(super) live_tail_eligible: bool,
    pub(super) is_user_boundary: bool,
}

fn rendered_entry_line_count(rendered: &str, view_width: u16) -> usize {
    wrap_plain_text(rendered, usize::from(view_width.max(1)))
        .len()
        .max(1)
}

pub(super) fn message_print_key(message: &Message) -> String {
    format!("message:{}", message.message_id)
}

pub(super) fn message_tool_call_id(message: &Message) -> Option<String> {
    message
        .tool_call()
        .map(|call| call.call_id.clone())
        .or_else(|| {
            message
                .tool_result()
                .map(|result| result.call_id.to_string())
        })
}

pub(super) fn merged_rendered_transcript_entries(
    messages: &[Message],
    message_seq_ids: &[Option<i64>],
    operator_commands: &[RecordedOperatorCommand],
    show_reasoning: bool,
    transcript_density: TranscriptDensity,
    view_width: u16,
) -> Vec<RenderedTranscriptEntry> {
    let mut entries = Vec::new();
    let mut message_index = 0usize;
    let mut command_index = 0usize;

    while message_index < messages.len() || command_index < operator_commands.len() {
        let message_seq_id = message_seq_ids.get(message_index).copied().flatten();
        let command_seq_id = operator_commands
            .get(command_index)
            .and_then(|command| command.seq_id);
        match (
            messages.get(message_index),
            operator_commands.get(command_index),
        ) {
            (Some(message), Some(command))
                if sequenced_entry_before(message_seq_id, command_seq_id, true) =>
            {
                if let Some(rendered) = rendered_scrollback_message(
                    messages,
                    message_index,
                    message,
                    show_reasoning,
                    transcript_density,
                    view_width,
                ) {
                    entries.push(RenderedTranscriptEntry {
                        key: Some(message_print_key(message)),
                        tool_call_id: message_tool_call_id(message),
                        seq_id: message_seq_id,
                        kind: transcript_entry_kind_for_message(message),
                        line_count: rendered_entry_line_count(&rendered, view_width),
                        assistant_markdown_source: assistant_markdown_source(message),
                        rendered,
                        allow_live_overflow: true,
                        printable_in_scrollback: message_seq_id.is_some(),
                        live_tail_eligible: true,
                        is_user_boundary: message.role == Role::User,
                    });
                }
                message_index += 1;
            }
            (Some(_), Some(command)) => {
                let rendered = render_operator_command(command, view_width);
                entries.push(RenderedTranscriptEntry {
                    key: Some(operator_command_print_key(command)),
                    tool_call_id: None,
                    seq_id: command_seq_id,
                    kind: if command.command_type == "local_error" {
                        TranscriptEntryKind::LocalError
                    } else {
                        TranscriptEntryKind::Operator
                    },
                    assistant_markdown_source: None,
                    line_count: rendered_entry_line_count(&rendered, view_width),
                    rendered,
                    allow_live_overflow: false,
                    printable_in_scrollback: true,
                    live_tail_eligible: true,
                    is_user_boundary: false,
                });
                command_index += 1;
            }
            (Some(message), None) => {
                if let Some(rendered) = rendered_scrollback_message(
                    messages,
                    message_index,
                    message,
                    show_reasoning,
                    transcript_density,
                    view_width,
                ) {
                    entries.push(RenderedTranscriptEntry {
                        key: Some(message_print_key(message)),
                        tool_call_id: message_tool_call_id(message),
                        seq_id: message_seq_id,
                        kind: transcript_entry_kind_for_message(message),
                        line_count: rendered_entry_line_count(&rendered, view_width),
                        assistant_markdown_source: assistant_markdown_source(message),
                        rendered,
                        allow_live_overflow: true,
                        printable_in_scrollback: message_seq_id.is_some(),
                        live_tail_eligible: true,
                        is_user_boundary: message.role == Role::User,
                    });
                }
                message_index += 1;
            }
            (None, Some(command)) => {
                let rendered = render_operator_command(command, view_width);
                entries.push(RenderedTranscriptEntry {
                    key: Some(operator_command_print_key(command)),
                    tool_call_id: None,
                    seq_id: command_seq_id,
                    kind: if command.command_type == "local_error" {
                        TranscriptEntryKind::LocalError
                    } else {
                        TranscriptEntryKind::Operator
                    },
                    assistant_markdown_source: None,
                    line_count: rendered_entry_line_count(&rendered, view_width),
                    rendered,
                    allow_live_overflow: false,
                    printable_in_scrollback: true,
                    live_tail_eligible: true,
                    is_user_boundary: false,
                });
                command_index += 1;
            }
            (None, None) => break,
        }
    }

    entries
}

fn rendered_scrollback_message(
    messages: &[Message],
    message_index: usize,
    message: &Message,
    show_reasoning: bool,
    transcript_density: TranscriptDensity,
    view_width: u16,
) -> Option<String> {
    if message_is_ask_exchange_only(message) {
        if let Some(result) = message
            .tool_result()
            .filter(|result| result.tool_name == "ask")
        {
            let question =
                ask_question_for_call_id(&messages[..message_index], &result.call_id.to_string());
            return Some(render_ask_history_entry(
                question.as_deref(),
                ask_answer_from_result(result).as_deref(),
                view_width,
            ));
        }
        return None;
    }

    render_transcript_message_with_options(message, show_reasoning, transcript_density, view_width)
}

fn sequenced_entry_before(
    message_seq_id: Option<i64>,
    command_seq_id: Option<i64>,
    message_first_on_tie: bool,
) -> bool {
    match (message_seq_id, command_seq_id) {
        (Some(message_seq_id), Some(command_seq_id)) => {
            if message_seq_id == command_seq_id {
                message_first_on_tie
            } else {
                message_seq_id < command_seq_id
            }
        }
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => message_first_on_tie,
    }
}

pub(crate) fn render_operator_command(
    command: &RecordedOperatorCommand,
    view_width: u16,
) -> String {
    let _ = view_width;
    match command.command_type.as_str() {
        "local_error" => render_compact_operator_entry("! error", &command.output),
        _ => {
            let status = if command.success { "ok" } else { "error" };
            render_compact_operator_entry(
                &format!("! {} [{status}]", command.raw_input),
                &command.output,
            )
        }
    }
}
