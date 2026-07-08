//! Telemetry and raw-event command rendering for the TUI.
//!
//! This module owns usage summaries, raw chunk previews, raw-to-event
//! correlation, and turn-event matching. General command rendering remains in
//! the parent module.

use base64::Engine;
use bt_core::{
    EventEnvelope, EventPayload, SessionExecutionInspection, TraceTurnInspection, TurnId,
    TurnInspection,
};
use bt_protocol::RawChunkDto;
use std::collections::{BTreeMap, BTreeSet};

use crate::message_render::{
    completion_delta_text_preview, completion_delta_tool_preview, render_message_preview,
};

use super::*;

#[derive(Default)]
struct UsageAggregate {
    turns: u32,
    llm_calls: u32,
    raw_chunks: u32,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    total_cost_usd: f64,
    latency_sum_ms: u64,
    latency_count: u32,
}

pub(crate) fn render_usage_output(execution: &SessionExecutionInspection, limit: usize) -> String {
    let mut totals = UsageAggregate::default();
    let mut by_model: BTreeMap<(String, String), UsageAggregate> = BTreeMap::new();

    for turn in &execution.turns {
        totals.turns += 1;
        totals.llm_calls += turn.llm_call_count;
        totals.raw_chunks += turn.raw_chunk_count;
        if let Some(usage) = &turn.usage {
            totals.prompt_tokens += usage.prompt_tokens;
            totals.completion_tokens += usage.completion_tokens;
            totals.total_tokens += usage.total_tokens;
        }
        if let Some(cost) = &turn.cost {
            totals.total_cost_usd += cost.total_usd;
        }
        if let Some(latency_ms) = turn.latency_ms {
            totals.latency_sum_ms += latency_ms;
            totals.latency_count += 1;
        }

        let entry = by_model
            .entry((turn.provider.clone(), turn.model.clone()))
            .or_default();
        entry.turns += 1;
        entry.llm_calls += turn.llm_call_count;
        entry.raw_chunks += turn.raw_chunk_count;
        if let Some(usage) = &turn.usage {
            entry.prompt_tokens += usage.prompt_tokens;
            entry.completion_tokens += usage.completion_tokens;
            entry.total_tokens += usage.total_tokens;
        }
        if let Some(cost) = &turn.cost {
            entry.total_cost_usd += cost.total_usd;
        }
        if let Some(latency_ms) = turn.latency_ms {
            entry.latency_sum_ms += latency_ms;
            entry.latency_count += 1;
        }
    }

    let avg_latency = if totals.latency_count > 0 {
        Some(totals.latency_sum_ms / u64::from(totals.latency_count))
    } else {
        None
    };

    let mut lines = vec![
        format!(
            "Usage session={} executed_turns={} llm_calls={} raw_chunks={}",
            short_id_string(&execution.session_id.to_string()),
            totals.turns,
            totals.llm_calls,
            totals.raw_chunks,
        ),
        format!(
            "Totals prompt={} completion={} total={} cost={} avg_latency={}",
            totals.prompt_tokens,
            totals.completion_tokens,
            totals.total_tokens,
            format_cost_or_na(totals.total_cost_usd),
            avg_latency
                .map(|value| format!("{value}ms"))
                .unwrap_or_else(|| "na".to_owned()),
        ),
        "By model:".to_owned(),
    ];

    if by_model.is_empty() {
        lines.push("- none".to_owned());
    } else {
        lines.extend(by_model.into_iter().map(|((provider, model), aggregate)| {
            let avg_latency = if aggregate.latency_count > 0 {
                Some(aggregate.latency_sum_ms / u64::from(aggregate.latency_count))
            } else {
                None
            };
            format!(
                "- {} {} turns={} llm_calls={} tokens={} cost={} avg_latency={}",
                provider,
                model,
                aggregate.turns,
                aggregate.llm_calls,
                aggregate.total_tokens,
                format_cost_or_na(aggregate.total_cost_usd),
                avg_latency
                    .map(|value| format!("{value}ms"))
                    .unwrap_or_else(|| "na".to_owned()),
            )
        }));
    }

    lines.push("Recent executed turns:".to_owned());
    let mut recent_turns = execution
        .turns
        .iter()
        .map(render_usage_turn_entry)
        .collect::<Vec<_>>();
    if recent_turns.len() > limit {
        recent_turns = recent_turns.split_off(recent_turns.len() - limit);
    }
    if recent_turns.is_empty() {
        lines.push("- none".to_owned());
    } else {
        lines.extend(recent_turns);
    }

    lines.join("\n")
}

fn render_usage_turn_entry(turn: &TraceTurnInspection) -> String {
    let total_tokens = turn.usage.as_ref().map(|usage| usage.total_tokens);
    let total_cost = turn.cost.as_ref().map(|cost| cost.total_usd);
    format!(
        "- {} {} {} llm_calls={} tokens={} cost={} latency={} finish={}",
        short_id_string(&turn.turn_id.to_string()),
        turn.provider,
        turn.model,
        turn.llm_call_count,
        total_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "na".to_owned()),
        total_cost
            .map(|value| format!("${value:.6}"))
            .unwrap_or_else(|| "na".to_owned()),
        turn.latency_ms
            .map(|value| format!("{value}ms"))
            .unwrap_or_else(|| "na".to_owned()),
        turn.finish_reason.as_deref().unwrap_or("none"),
    )
}

pub(crate) fn render_raw_output(
    turn: &TurnInspection,
    chunks: &[RawChunkDto],
    llm_call_ordinal: Option<u32>,
    has_more_before: bool,
) -> String {
    let mut lines = vec![format!(
        "Raw turn={} branch={} {} {} llm_call={} chunks={} has_more_before={}",
        short_id_string(&turn.turn_id.to_string()),
        short_id_string(&turn.branch_id.to_string()),
        turn.provider,
        turn.model,
        llm_call_ordinal
            .map(|value| value.to_string())
            .unwrap_or_else(|| "all".to_owned()),
        chunks.len(),
        if has_more_before { "yes" } else { "no" },
    )];

    if chunks.is_empty() {
        lines.push("No raw chunks recorded for this turn.".to_owned());
        return lines.join("\n");
    }

    lines.extend(chunks.iter().map(render_raw_chunk_entry));
    lines.join("\n")
}

pub(crate) fn render_raw_diff_output(
    turn: &TurnInspection,
    chunks: &[RawChunkDto],
    llm_call_ordinal: Option<u32>,
    has_more_before: bool,
    events: &[EventEnvelope],
) -> String {
    let completion_chunks = events
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::CompletionChunk { .. }))
        .count();
    let raw_markers = events
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::RawChunkPersisted { .. }))
        .count();
    let message_appends = events
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::MessageAppended { .. }))
        .count();
    let tool_events = events
        .iter()
        .filter(|event| {
            matches!(
                event.payload,
                EventPayload::ToolCallRequested { .. }
                    | EventPayload::ToolApprovalRequested { .. }
                    | EventPayload::ToolApprovalResolved { .. }
                    | EventPayload::ToolExecutionFinished { .. }
            )
        })
        .count();

    let mut lines = vec![
        format!(
            "Raw diff turn={} branch={} {} {}",
            short_id_string(&turn.turn_id.to_string()),
            short_id_string(&turn.branch_id.to_string()),
            turn.provider,
            turn.model,
        ),
        format!(
            "Summary raw_chunks={} llm_call={} has_more_before={} structured_events={} completion_chunks={} raw_markers={} message_appends={} tool_events={}",
            chunks.len(),
            llm_call_ordinal
                .map(|value| value.to_string())
                .unwrap_or_else(|| "all".to_owned()),
            if has_more_before { "yes" } else { "no" },
            events.len(),
            completion_chunks,
            raw_markers,
            message_appends,
            tool_events,
        ),
        "Structured projection:".to_owned(),
    ];

    if events.is_empty() {
        lines.push("- none".to_owned());
    } else {
        lines.extend(events.iter().filter_map(render_raw_projection_event));
    }

    lines.push("Raw chunks:".to_owned());
    if chunks.is_empty() {
        lines.push("- none".to_owned());
    } else {
        lines.extend(chunks.iter().map(render_raw_chunk_entry));
    }

    lines.join("\n")
}

pub(crate) fn correlate_raw_diff_events(
    chunks: &[RawChunkDto],
    llm_call_ordinal: Option<u32>,
    events: &[EventEnvelope],
) -> Vec<EventEnvelope> {
    let chunk_ids = chunks
        .iter()
        .map(|chunk| chunk.chunk_id)
        .collect::<BTreeSet<_>>();
    let event_ids = chunks
        .iter()
        .filter_map(|chunk| chunk.event_id.clone())
        .collect::<BTreeSet<_>>();

    events
        .iter()
        .filter(|event| match &event.payload {
            EventPayload::RawChunkPersisted { .. } => {
                event_ids.contains(&event.event_id.to_string())
            }
            EventPayload::CompletionChunk {
                llm_call_ordinal: event_llm_call_ordinal,
                raw_chunk_index,
                ..
            } => {
                raw_chunk_index.is_some_and(|value| chunk_ids.contains(&value))
                    && llm_call_ordinal
                        .map(|expected| event_llm_call_ordinal == &Some(expected))
                        .unwrap_or(true)
            }
            _ => false,
        })
        .cloned()
        .collect()
}

fn render_raw_chunk_entry(chunk: &RawChunkDto) -> String {
    let llm_call = chunk
        .llm_call_ordinal
        .map(|ordinal| ordinal.to_string())
        .unwrap_or_else(|| "na".to_owned());
    let event_id = chunk
        .event_id
        .as_deref()
        .map(short_id_string)
        .unwrap_or_else(|| "na".to_owned());

    format!(
        "- #{} stream={} provider={} llm_call={} event={} at={} {}",
        chunk.chunk_id,
        chunk.stream_name,
        chunk.provider,
        llm_call,
        event_id,
        chunk.received_at,
        preview_raw_chunk_content(&chunk.content_base64),
    )
}

fn render_raw_projection_event(event: &EventEnvelope) -> Option<String> {
    let seq = event.seq_id.unwrap_or_default();
    match &event.payload {
        EventPayload::TurnStarted {
            provider,
            model,
            message_count,
            ..
        } => Some(format!(
            "- #{seq} turn.started {} {} messages={}",
            provider, model, message_count
        )),
        EventPayload::TurnInstructionProvenanceRecorded { provenance } => Some(format!(
            "- #{seq} turn.instructions.recorded {} {} instructions={}",
            provenance.provider,
            provenance.model,
            provenance.instructions.len()
        )),
        EventPayload::CompletionRequested {
            provider,
            model,
            message_count,
            ..
        } => Some(format!(
            "- #{seq} completion.requested {} {} messages={}",
            provider, model, message_count
        )),
        EventPayload::CompletionChunk {
            llm_call_ordinal,
            deltas,
            raw_chunk_index,
        } => Some(format!(
            "- #{seq} completion.chunk llm_call={} raw_chunk_index={} text={} tool_call={}",
            llm_call_ordinal
                .map(|value| value.to_string())
                .unwrap_or_else(|| "na".to_owned()),
            raw_chunk_index
                .map(|value| value.to_string())
                .unwrap_or_else(|| "na".to_owned()),
            completion_delta_text_preview(deltas)
                .map(|value| truncate_preview(&sanitize_inline_preview(&value), 96))
                .unwrap_or_else(|| "na".to_owned()),
            completion_delta_tool_preview(deltas)
                .map(|value| truncate_preview(&sanitize_inline_preview(&value), 96))
                .unwrap_or_else(|| "na".to_owned()),
        )),
        EventPayload::MessageAppended { message } => Some(format!(
            "- #{seq} message.appended {}",
            truncate_preview(&render_message_preview(message), 120)
        )),
        EventPayload::ToolCallRequested {
            call_id,
            tool_name,
            arguments,
        } => Some(format!(
            "- #{seq} tool.call.requested {} {} {}",
            tool_name,
            short_id_string(&call_id.to_string()),
            truncate_preview(&sanitize_inline_preview(&arguments.to_string()), 96)
        )),
        EventPayload::ToolApprovalRequested {
            call_id, tool_name, ..
        } => Some(format!(
            "- #{seq} tool.approval.requested {} {}",
            tool_name,
            short_id_string(&call_id.to_string())
        )),
        EventPayload::ToolApprovalResolved {
            call_id,
            tool_name,
            decision,
            ..
        } => Some(format!(
            "- #{seq} tool.approval.resolved {} {} {:?}",
            tool_name,
            short_id_string(&call_id.to_string()),
            decision
        )),
        EventPayload::ToolExecutionFinished {
            call_id,
            tool_name,
            result,
        } => Some(format!(
            "- #{seq} tool.execution.finished {} {} {}",
            tool_name,
            short_id_string(&call_id.to_string()),
            truncate_preview(&sanitize_inline_preview(&result.output.to_string()), 96)
        )),
        EventPayload::CompletionFinished {
            provider,
            model,
            finish_reason,
            usage,
            ..
        } => Some(format!(
            "- #{seq} completion.finished {} {} finish={} tokens={}",
            provider, model, finish_reason, usage.total_tokens
        )),
        EventPayload::SessionError {
            class,
            code,
            message,
            retryable,
        } => Some(format!(
            "- #{seq} session.error {} {} retryable={} {}",
            class,
            code,
            retryable,
            truncate_preview(&sanitize_inline_preview(message), 96)
        )),
        EventPayload::TurnFinished {
            provider,
            model,
            status,
            finish_reason,
            ..
        } => Some(format!(
            "- #{seq} turn.finished {} {} status={} finish={}",
            provider,
            model,
            status,
            finish_reason.as_deref().unwrap_or("none")
        )),
        EventPayload::RawChunkPersisted {
            provider,
            chunk_index,
            stream,
            llm_call_ordinal,
        } => Some(format!(
            "- #{seq} raw_chunk.persisted {} stream={} llm_call={} chunk_index={}",
            provider,
            stream,
            llm_call_ordinal
                .map(|value| value.to_string())
                .unwrap_or_else(|| "na".to_owned()),
            chunk_index
        )),
        EventPayload::ContextCompacted {
            messages_before,
            messages_after,
            ..
        } => Some(format!(
            "- #{seq} context.compacted {} -> {}",
            messages_before, messages_after
        )),
        EventPayload::SessionSteered { message, .. } => Some(format!(
            "- #{seq} session.steered {}",
            truncate_preview(&sanitize_inline_preview(message), 96)
        )),
        EventPayload::SessionSteersResolved {
            outcome,
            combined_message,
            reason,
            ..
        } => Some(format!(
            "- #{seq} session.steers.resolved {:?} {}",
            outcome,
            truncate_preview(
                &sanitize_inline_preview(
                    combined_message
                        .as_deref()
                        .or(reason.as_deref())
                        .unwrap_or_default()
                ),
                96
            )
        )),
        EventPayload::SessionCancelled { reason } => Some(format!(
            "- #{seq} session.cancelled {}",
            sanitize_inline_preview(reason)
        )),
        EventPayload::SessionCancelCleared { reason } => Some(format!(
            "- #{seq} session.cancel.cleared {}",
            sanitize_inline_preview(reason)
        )),
        EventPayload::SessionQueuedMessageEnqueued { message, .. } => Some(format!(
            "- #{seq} session.queued_message.enqueued {}",
            truncate_preview(
                &sanitize_inline_preview(&bt_core::render_queue_message_input(message)),
                96
            )
        )),
        EventPayload::SessionQueuedMessageResolved {
            outcome, reason, ..
        } => Some(format!(
            "- #{seq} session.queued_message.resolved {:?} {}",
            outcome,
            sanitize_inline_preview(reason.as_deref().unwrap_or_default())
        )),
        EventPayload::BudgetCheckpoint { tokens_used, .. } => {
            Some(format!("- #{seq} budget.checkpoint tokens={tokens_used}"))
        }
        _ => None,
    }
}

fn preview_raw_chunk_content(content_base64: &str) -> String {
    match base64::engine::general_purpose::STANDARD.decode(content_base64) {
        Ok(bytes) => {
            let byte_len = bytes.len();
            match String::from_utf8(bytes) {
                Ok(text) => format!(
                    "bytes={} text={}",
                    byte_len,
                    truncate_preview(
                        &sanitize_inline_preview(&text),
                        crate::RAW_CHUNK_PREVIEW_LIMIT
                    )
                ),
                Err(_) => format!(
                    "bytes={} base64={}",
                    byte_len,
                    truncate_preview(content_base64, crate::RAW_CHUNK_PREVIEW_LIMIT)
                ),
            }
        }
        Err(error) => format!("decode_error={error}"),
    }
}

pub(super) fn sanitize_inline_preview(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

pub(super) fn truncate_preview(raw: &str, limit: usize) -> String {
    if raw.chars().count() <= limit {
        return raw.to_owned();
    }

    let truncated = raw
        .chars()
        .take(limit.saturating_sub(3))
        .collect::<String>();
    format!("{truncated}...")
}

pub(crate) fn turn_event_matches(event: &EventEnvelope, turn: &TurnInspection) -> bool {
    if event.branch_id != turn.branch_id {
        return false;
    }

    if event
        .turn_id
        .as_ref()
        .is_some_and(|turn_id| *turn_id == turn.turn_id)
    {
        return true;
    }

    payload_turn_id(&event.payload).is_some_and(|turn_id| turn_id == turn.turn_id)
}

fn payload_turn_id(payload: &EventPayload) -> Option<TurnId> {
    match payload {
        EventPayload::TurnStarted { turn_id, .. } | EventPayload::TurnFinished { turn_id, .. } => {
            Some(*turn_id)
        }
        _ => None,
    }
}
