use super::*;

pub(crate) fn merge_messages(
    existing_messages: &mut Vec<Message>,
    existing_seq_ids: &mut Vec<Option<i64>>,
    incoming_messages: Vec<Message>,
    incoming_seq_ids: Vec<Option<i64>>,
    optimistic_user_text: Option<&str>,
) -> bool {
    let drained_messages = std::mem::take(existing_messages);
    let mut drained_seq_ids = std::mem::take(existing_seq_ids);
    drained_seq_ids.resize(drained_messages.len(), None);
    let mut entries = drained_messages
        .into_iter()
        .zip(drained_seq_ids)
        .map(|(message, seq_id)| (seq_id, message))
        .collect::<Vec<_>>();
    let mut reconciled_optimistic_user = false;
    for (message, seq_id) in incoming_messages.into_iter().zip(incoming_seq_ids) {
        if !reconciled_optimistic_user
            && let Some(expected_text) = optimistic_user_text
            && seq_id.is_some()
            && message_has_text(&message, Role::User, expected_text)
        {
            reconciled_optimistic_user = true;
        }
        entries.push((seq_id, message));
    }
    entries.sort_by(|left, right| compare_transcript_seq_ids(left.0, right.0));
    entries.dedup_by(|left, right| {
        (left.0.is_some() && left.0 == right.0) || left.1.message_id == right.1.message_id
    });
    for (seq_id, message) in entries {
        existing_seq_ids.push(seq_id);
        existing_messages.push(message);
    }
    reconciled_optimistic_user
}

pub(crate) fn merge_operator_commands(
    existing: &mut Vec<RecordedOperatorCommand>,
    incoming: Vec<RecordedOperatorCommand>,
) {
    existing.extend(incoming);
    existing.sort_by(|left, right| compare_transcript_seq_ids(left.seq_id, right.seq_id));
    existing.dedup_by(|left, right| left.seq_id.is_some() && left.seq_id == right.seq_id);
}

pub(crate) fn operator_command_print_key(command: &RecordedOperatorCommand) -> String {
    format!(
        "command:{:?}:{}:{}:{}:{}",
        command.seq_id,
        command.occurred_at.unix_timestamp_nanos(),
        command.command_type,
        command.raw_input,
        command.output
    )
}

pub(crate) fn message_has_text(message: &Message, role: Role, text: &str) -> bool {
    let expected = text.trim_end_matches(['\n', '\r']);
    message_text_content(message, role)
        .is_some_and(|actual| !actual.is_empty() && actual == expected)
}

pub(crate) fn message_text_content(message: &Message, role: Role) -> Option<String> {
    if message.role != role {
        return None;
    }

    let actual = message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.trim_end_matches(['\n', '\r'])),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    (!actual.is_empty()).then_some(actual)
}

pub(crate) fn normalized_error_signature(message: &str) -> String {
    let mut current = message.trim();
    loop {
        let lower = current.to_ascii_lowercase();
        if let Some(rest) = lower
            .strip_prefix("send failed: ")
            .map(|_| &current["send failed: ".len()..])
            .or_else(|| {
                lower
                    .strip_prefix("send task failed: ")
                    .map(|_| &current["send task failed: ".len()..])
            })
            .or_else(|| {
                lower
                    .strip_prefix("background update failed: ")
                    .map(|_| &current["background update failed: ".len()..])
            })
        {
            current = rest.trim();
            continue;
        }

        let Some(wrapper_end) = lower.find(": ") else {
            break;
        };
        let prefix = &lower[..wrapper_end];
        if prefix.contains("error") || prefix.contains("failed") {
            current = current[wrapper_end + 2..].trim();
            continue;
        }
        break;
    }

    current
        .trim_end_matches(" [retryable]")
        .trim()
        .to_ascii_lowercase()
}

pub(crate) fn errors_look_equivalent(existing: &str, incoming: &str) -> bool {
    let existing = normalized_error_signature(existing);
    let incoming = normalized_error_signature(incoming);
    existing == incoming || existing.contains(&incoming) || incoming.contains(&existing)
}
