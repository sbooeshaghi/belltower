//! Mirrored-event shaping for tracing lives here. This module projects canonical
//! session events into preview-friendly fields and tracing records; export
//! format dispatch and OTLP span assembly live elsewhere.

use bt_core::{EventEnvelope, EventPayload, Message, MessagePart};
use tracing::Level;

use crate::render::{
    completion_delta_text_preview, completion_delta_tool_preview, format_timestamp,
    message_part_preview_text, preview_json, truncate_string,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MirroredEventFields {
    pub(crate) event_kind: String,
    pub(crate) seq_id: Option<i64>,
    pub(crate) event_id: String,
    pub(crate) session_id: String,
    pub(crate) branch_id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) span_id: String,
    pub(crate) parent_span_id: Option<String>,
    pub(crate) span_kind: String,
    pub(crate) occurred_at: String,
    pub(crate) attributes_json: String,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) call_id: Option<String>,
    pub(crate) related_message_id: Option<String>,
    pub(crate) related_message_direction: Option<String>,
    pub(crate) related_message_peer_session_id: Option<String>,
    pub(crate) related_message_delivery_mode: Option<String>,
    pub(crate) related_message_kind: Option<String>,
    pub(crate) related_message_status: Option<String>,
    pub(crate) related_message_resulting_turn_id: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) finish_reason: Option<String>,
    pub(crate) approval_decision: Option<String>,
    pub(crate) approval_scope: Option<String>,
    pub(crate) approval_source: Option<String>,
    pub(crate) approval_request_fingerprint: Option<String>,
    pub(crate) approval_arguments_hash: Option<String>,
    pub(crate) approval_surface: Option<String>,
    pub(crate) approval_risk_class: Option<String>,
    pub(crate) turn_start_source: Option<String>,
    pub(crate) resumed_from_call_id: Option<String>,
    pub(crate) message_role: Option<String>,
    pub(crate) command_type: Option<String>,
    pub(crate) message_count: Option<u32>,
    pub(crate) settings_revision_id: Option<u64>,
    pub(crate) latency_ms: Option<u64>,
    pub(crate) chunk_index: Option<i64>,
    pub(crate) llm_call_ordinal: Option<u32>,
    pub(crate) messages_before: Option<u32>,
    pub(crate) messages_after: Option<u32>,
    pub(crate) tokens_before: Option<u64>,
    pub(crate) tokens_after: Option<u64>,
    pub(crate) files_read_count: Option<usize>,
    pub(crate) files_modified_count: Option<usize>,
    pub(crate) prompt_tokens: Option<u64>,
    pub(crate) completion_tokens: Option<u64>,
    pub(crate) total_tokens: Option<u64>,
    pub(crate) cost_total_usd: Option<f64>,
    pub(crate) error_class: Option<String>,
    pub(crate) error_code: Option<String>,
    pub(crate) error_retryable: Option<bool>,
    pub(crate) success: Option<bool>,
    pub(crate) is_error: Option<bool>,
    pub(crate) text_preview: Option<String>,
    pub(crate) arguments_preview: Option<String>,
    pub(crate) output_preview: Option<String>,
    pub(crate) delta_text_preview: Option<String>,
    pub(crate) delta_tool_call_preview: Option<String>,
}

pub fn mirror_event(event: &EventEnvelope) {
    let fields = mirrored_event_fields(event);
    tracing::event!(
        target: "belltower.events",
        Level::INFO,
        event_kind = %fields.event_kind,
        seq_id = ?fields.seq_id,
        event_id = %fields.event_id,
        session_id = %fields.session_id,
        branch_id = %fields.branch_id,
        turn_id = ?fields.turn_id,
        span_id = %fields.span_id,
        parent_span_id = ?fields.parent_span_id,
        span_kind = %fields.span_kind,
        occurred_at = %fields.occurred_at,
        attributes_json = %fields.attributes_json,
        provider = ?fields.provider,
        model = ?fields.model,
        tool_name = ?fields.tool_name,
        call_id = ?fields.call_id,
        related_message_id = ?fields.related_message_id,
        related_message_direction = ?fields.related_message_direction,
        related_message_peer_session_id = ?fields.related_message_peer_session_id,
        related_message_delivery_mode = ?fields.related_message_delivery_mode,
        related_message_kind = ?fields.related_message_kind,
        related_message_status = ?fields.related_message_status,
        related_message_resulting_turn_id = ?fields.related_message_resulting_turn_id,
        status = ?fields.status,
        finish_reason = ?fields.finish_reason,
        approval_decision = ?fields.approval_decision,
        approval_scope = ?fields.approval_scope,
        approval_source = ?fields.approval_source,
        approval_request_fingerprint = ?fields.approval_request_fingerprint,
        approval_arguments_hash = ?fields.approval_arguments_hash,
        approval_surface = ?fields.approval_surface,
        approval_risk_class = ?fields.approval_risk_class,
        turn_start_source = ?fields.turn_start_source,
        resumed_from_call_id = ?fields.resumed_from_call_id,
        message_role = ?fields.message_role,
        command_type = ?fields.command_type,
        message_count = ?fields.message_count,
        settings_revision_id = ?fields.settings_revision_id,
        latency_ms = ?fields.latency_ms,
        chunk_index = ?fields.chunk_index,
        llm_call_ordinal = ?fields.llm_call_ordinal,
        messages_before = ?fields.messages_before,
        messages_after = ?fields.messages_after,
        tokens_before = ?fields.tokens_before,
        tokens_after = ?fields.tokens_after,
        files_read_count = ?fields.files_read_count,
        files_modified_count = ?fields.files_modified_count,
        prompt_tokens = ?fields.prompt_tokens,
        completion_tokens = ?fields.completion_tokens,
        total_tokens = ?fields.total_tokens,
        cost_total_usd = ?fields.cost_total_usd,
        error_class = ?fields.error_class,
        error_code = ?fields.error_code,
        error_retryable = ?fields.error_retryable,
        success = ?fields.success,
        is_error = ?fields.is_error,
        text_preview = ?fields.text_preview,
        arguments_preview = ?fields.arguments_preview,
        output_preview = ?fields.output_preview,
        delta_text_preview = ?fields.delta_text_preview,
        delta_tool_call_preview = ?fields.delta_tool_call_preview,
        "committed session event mirrored into tracing"
    );
}

pub(crate) fn mirrored_event_fields(event: &EventEnvelope) -> MirroredEventFields {
    let mut fields = MirroredEventFields {
        event_kind: event.kind().to_owned(),
        seq_id: event.seq_id,
        event_id: event.event_id.to_string(),
        session_id: event.session_id.to_string(),
        branch_id: event.branch_id.to_string(),
        turn_id: event.turn_id.map(|turn_id| turn_id.to_string()),
        span_id: event.span_id.to_string(),
        parent_span_id: event.parent_span_id.map(|span_id| span_id.to_string()),
        span_kind: format!("{:?}", event.span_kind),
        occurred_at: format_timestamp(event.occurred_at),
        attributes_json: preview_json(&event.attributes),
        provider: None,
        model: None,
        tool_name: None,
        call_id: None,
        related_message_id: None,
        related_message_direction: None,
        related_message_peer_session_id: None,
        related_message_delivery_mode: None,
        related_message_kind: None,
        related_message_status: None,
        related_message_resulting_turn_id: None,
        status: None,
        finish_reason: None,
        approval_decision: None,
        approval_scope: None,
        approval_source: None,
        approval_request_fingerprint: None,
        approval_arguments_hash: None,
        approval_surface: None,
        approval_risk_class: None,
        turn_start_source: None,
        resumed_from_call_id: None,
        message_role: None,
        command_type: None,
        message_count: None,
        settings_revision_id: None,
        latency_ms: None,
        chunk_index: None,
        llm_call_ordinal: None,
        messages_before: None,
        messages_after: None,
        tokens_before: None,
        tokens_after: None,
        files_read_count: None,
        files_modified_count: None,
        prompt_tokens: None,
        completion_tokens: None,
        total_tokens: None,
        cost_total_usd: None,
        error_class: None,
        error_code: None,
        error_retryable: None,
        success: None,
        is_error: None,
        text_preview: None,
        arguments_preview: None,
        output_preview: None,
        delta_text_preview: None,
        delta_tool_call_preview: None,
    };

    match &event.payload {
        EventPayload::SessionStarted {
            project_root,
            connection_id,
        } => {
            fields.provider = Some(connection_id.clone());
            fields.text_preview = Some(truncate_string(project_root));
        }
        EventPayload::SessionSpawnRequested {
            child_session_id,
            objective,
            connection_id,
            model_id,
        } => {
            fields.provider = Some(connection_id.clone());
            fields.model = model_id.clone();
            fields.status = Some(child_session_id.to_string());
            fields.text_preview = Some(truncate_string(objective));
        }
        EventPayload::SessionSpawned {
            child_session_id,
            child_branch_id,
            objective,
        } => {
            fields.status = Some(child_session_id.to_string());
            fields.finish_reason = Some(child_branch_id.to_string());
            fields.text_preview = Some(truncate_string(objective));
        }
        EventPayload::SessionHandoffRecorded {
            parent_session_id,
            parent_branch_id,
            parent_turn_id,
            objective,
            summary,
        } => {
            fields.status = Some(parent_session_id.to_string());
            fields.finish_reason = Some(format!(
                "parent_branch={} parent_turn={parent_turn_id:?}",
                parent_branch_id
            ));
            fields.text_preview = Some(truncate_string(objective));
            fields.output_preview = Some(truncate_string(summary));
        }
        EventPayload::SessionResultImported {
            child_session_id,
            status,
            summary,
        } => {
            fields.status = Some(status.clone());
            fields.finish_reason = Some(child_session_id.to_string());
            fields.output_preview = Some(truncate_string(summary));
        }
        EventPayload::SessionResultRejected {
            child_session_id,
            reason,
        } => {
            fields.status = Some(child_session_id.to_string());
            fields.output_preview = Some(truncate_string(reason));
        }
        EventPayload::RelatedSessionMessageRecorded {
            direction,
            counterpart_event_id,
            message,
        } => {
            fields.status = Some(format!("{direction:?}"));
            fields.related_message_id = Some(message.message_id.to_string());
            fields.related_message_direction = Some(format!("{direction:?}"));
            fields.related_message_peer_session_id = Some(
                if matches!(direction, bt_core::RelatedSessionMessageDirection::Sent) {
                    message.destination_session_id
                } else {
                    message.source_session_id
                }
                .to_string(),
            );
            fields.related_message_delivery_mode = Some(format!("{:?}", message.delivery_mode));
            fields.related_message_kind = Some(format!("{:?}", message.kind));
            fields.related_message_status = Some(
                if matches!(direction, bt_core::RelatedSessionMessageDirection::Received)
                    && matches!(
                        message.delivery_mode,
                        bt_core::RelatedSessionDeliveryMode::Wake
                    )
                {
                    "Pending"
                } else {
                    "Delivered"
                }
                .to_owned(),
            );
            fields.finish_reason = Some(format!(
                "delivery={:?} peer={} counterpart_event={counterpart_event_id}",
                message.delivery_mode,
                if matches!(direction, bt_core::RelatedSessionMessageDirection::Sent) {
                    message.destination_session_id
                } else {
                    message.source_session_id
                }
            ));
            fields.text_preview = Some(truncate_string(&message.text));
        }
        EventPayload::RelatedSessionMessageResolved {
            message_id,
            status,
            resulting_turn_id,
            reason,
        } => {
            fields.status = Some(format!("{status:?}"));
            fields.related_message_id = Some(message_id.to_string());
            fields.related_message_status = Some(format!("{status:?}"));
            fields.related_message_resulting_turn_id =
                resulting_turn_id.map(|turn_id| turn_id.to_string());
            fields.finish_reason = Some(format!(
                "message={message_id} resulting_turn={resulting_turn_id:?}"
            ));
            fields.output_preview = reason.as_ref().map(|reason| truncate_string(reason));
        }
        EventPayload::RelatedSessionMessageSettled {
            message_id,
            reply_message_id,
            reply_kind,
            settling_turn_id,
        } => {
            fields.status = Some("Settled".to_owned());
            fields.related_message_id = Some(message_id.to_string());
            fields.related_message_kind = Some(format!("{reply_kind:?}"));
            fields.related_message_status = Some("Settled".to_owned());
            fields.related_message_resulting_turn_id = Some(settling_turn_id.to_string());
            fields.finish_reason = Some(format!(
                "message={message_id} reply={reply_message_id} outcome={reply_kind:?} settling_turn={settling_turn_id}"
            ));
        }
        EventPayload::SessionSettingsUpdated {
            settings_revision_id,
            connection_id,
            model_id,
            ..
        } => {
            fields.settings_revision_id = Some(*settings_revision_id);
            fields.provider = Some(connection_id.clone());
            fields.model = model_id.clone();
        }
        EventPayload::SessionEnded { reason }
        | EventPayload::SessionCancelled { reason }
        | EventPayload::SessionCancelCleared { reason } => {
            fields.status = Some(reason.clone());
        }
        EventPayload::SessionQueuedMessageEnqueued {
            message,
            settings_revision_id,
            ..
        } => {
            fields.settings_revision_id = Some(*settings_revision_id);
            populate_message_fields(&mut fields, message);
            fields.status = Some("pending".to_owned());
        }
        EventPayload::SessionQueuedMessageResolved {
            outcome, reason, ..
        } => {
            fields.status = Some(format!("{outcome:?}"));
            fields.text_preview = reason.as_ref().map(|reason| truncate_string(reason));
        }
        EventPayload::BranchCreated {
            parent_branch_id,
            parent_event_id,
        } => {
            fields.text_preview = Some(truncate_string(&format!(
                "parent_branch={parent_branch_id:?} parent_event={parent_event_id:?}"
            )));
        }
        EventPayload::BranchActivated { branch_id } => {
            fields.text_preview = Some(truncate_string(&branch_id.to_string()));
        }
        EventPayload::BranchSummarized {
            summary,
            files_read,
            files_modified,
            ..
        } => {
            fields.text_preview = Some(truncate_string(summary));
            fields.files_read_count = Some(files_read.len());
            fields.files_modified_count = Some(files_modified.len());
        }
        EventPayload::PlanUpdated { items } => {
            fields.message_count = Some(items.len() as u32);
            fields.text_preview = items.first().map(|item| truncate_string(&item.content));
        }
        EventPayload::MessageAppended { message } => populate_message_fields(&mut fields, message),
        EventPayload::TurnStarted {
            provider,
            model,
            message_count,
            settings_revision_id,
            source,
            resumed_from_call_id,
            ..
        } => {
            fields.provider = Some(provider.clone());
            fields.model = Some(model.clone());
            fields.message_count = Some(*message_count);
            fields.settings_revision_id = Some(*settings_revision_id);
            fields.turn_start_source = Some(turn_start_source_label(source).to_owned());
            fields.resumed_from_call_id = resumed_from_call_id.as_ref().map(ToString::to_string);
        }
        EventPayload::TurnInstructionProvenanceRecorded { provenance } => {
            fields.provider = Some(provenance.provider.clone());
            fields.model = Some(provenance.model.clone());
            fields.settings_revision_id = Some(provenance.settings_revision_id);
            fields.message_count = Some(provenance.instructions.len() as u32);
            fields.text_preview = Some(truncate_string(&provenance.core_prompt.title));
            fields.output_preview = Some(truncate_string(&provenance.rendered_system_prompt));
        }
        EventPayload::TurnContextManifestRecorded { manifest } => {
            fields.llm_call_ordinal = Some(manifest.llm_call_ordinal);
            fields.provider = Some(manifest.provider.clone());
            fields.model = Some(manifest.model.clone());
            fields.settings_revision_id = Some(manifest.settings_revision_id);
            fields.message_count = Some(manifest.messages.len() as u32);
            fields.text_preview = Some(truncate_string(&format!(
                "messages={} tools={} system_prompt_chars={} compacted={}",
                manifest.messages.len(),
                manifest.tools.len(),
                manifest.system_prompt.char_count,
                manifest.compacted
            )));
        }
        EventPayload::CompletionRequested {
            llm_call_ordinal,
            provider,
            model,
            message_count,
        } => {
            fields.llm_call_ordinal = Some(*llm_call_ordinal);
            fields.provider = Some(provider.clone());
            fields.model = Some(model.clone());
            fields.message_count = Some(*message_count);
        }
        EventPayload::CompletionChunk {
            llm_call_ordinal,
            deltas,
            raw_chunk_index,
        } => {
            fields.llm_call_ordinal = *llm_call_ordinal;
            fields.chunk_index = *raw_chunk_index;
            fields.delta_text_preview = completion_delta_text_preview(deltas);
            fields.delta_tool_call_preview = completion_delta_tool_preview(deltas);
        }
        EventPayload::CompletionFinished {
            llm_call_ordinal,
            provider,
            model,
            usage,
            cost,
            finish_reason,
            latency_ms,
        } => {
            fields.llm_call_ordinal = Some(*llm_call_ordinal);
            fields.provider = Some(provider.clone());
            fields.model = Some(model.clone());
            fields.finish_reason = Some(finish_reason.clone());
            fields.latency_ms = Some(*latency_ms);
            fields.prompt_tokens = Some(usage.prompt_tokens);
            fields.completion_tokens = Some(usage.completion_tokens);
            fields.total_tokens = Some(usage.total_tokens);
            fields.cost_total_usd = cost.as_ref().map(|cost| cost.total_usd);
        }
        EventPayload::SessionError {
            class,
            code,
            message,
            retryable,
        } => {
            fields.error_class = Some(class.to_string());
            fields.error_code = Some(code.clone());
            fields.error_retryable = Some(*retryable);
            fields.status = Some(code.clone());
            fields.text_preview = Some(truncate_string(message));
        }
        EventPayload::TurnFinished {
            provider,
            model,
            status,
            finish_reason,
            latency_ms,
            ..
        } => {
            fields.provider = Some(provider.clone());
            fields.model = Some(model.clone());
            fields.status = Some(status.clone());
            fields.finish_reason = finish_reason.clone();
            fields.latency_ms = Some(*latency_ms);
        }
        EventPayload::ToolCallRequested {
            call_id,
            tool_name,
            arguments,
        } => {
            fields.call_id = Some(call_id.to_string());
            fields.tool_name = Some(tool_name.clone());
            fields.arguments_preview = Some(preview_json(arguments));
        }
        EventPayload::ToolOperationRecorded {
            call_id,
            tool_name,
            operation,
        } => {
            fields.call_id = Some(call_id.to_string());
            fields.tool_name = Some(tool_name.clone());
            fields.status = Some(format!("{:?}", operation.initiator));
            fields.message_count = Some(operation.artifact_refs.len() as u32);
            fields.text_preview = Some(truncate_string(&format!(
                "risk={:?} read_only={:?} execution={:?}",
                operation.risk_class, operation.is_read_only, operation.execution_mode
            )));
        }
        EventPayload::ToolApprovalRequested {
            call_id,
            tool_name,
            snapshot,
        } => {
            fields.call_id = Some(call_id.to_string());
            fields.tool_name = Some(tool_name.clone());
            fields.status = Some("pending".to_owned());
            if let Some(snapshot) = snapshot {
                fields.approval_request_fingerprint = Some(snapshot.request_fingerprint.clone());
                fields.approval_arguments_hash = Some(snapshot.arguments_hash.clone());
                fields.approval_surface = Some(snapshot.surface.clone());
                fields.approval_risk_class =
                    Some(format!("{:?}", snapshot.tool_metadata.risk_class));
            }
        }
        EventPayload::ToolApprovalResolved {
            call_id,
            tool_name,
            request_fingerprint,
            resolution,
            decision,
        } => {
            fields.call_id = Some(call_id.to_string());
            fields.tool_name = Some(tool_name.clone());
            fields.approval_request_fingerprint = resolution
                .as_ref()
                .map(|resolution| resolution.request_fingerprint.clone())
                .or_else(|| request_fingerprint.clone());
            match decision {
                bt_core::ApprovalDecision::Approved { scope, source, .. } => {
                    fields.approval_decision = Some("approved".to_owned());
                    fields.approval_scope = Some(format!("{scope:?}"));
                    fields.approval_source = Some(format!("{source:?}"));
                }
                bt_core::ApprovalDecision::Denied {
                    reason,
                    scope,
                    source,
                    ..
                } => {
                    fields.approval_decision = Some("denied".to_owned());
                    fields.approval_scope = Some(format!("{scope:?}"));
                    fields.approval_source = Some(format!("{source:?}"));
                    fields.status = reason.clone();
                }
            }
        }
        EventPayload::SessionSteered {
            message,
            settings_revision_id,
        } => {
            fields.settings_revision_id = Some(*settings_revision_id);
            fields.text_preview = Some(truncate_string(message));
        }
        EventPayload::ToolExecutionFinished {
            call_id,
            tool_name,
            result,
        } => {
            fields.call_id = Some(call_id.to_string());
            fields.tool_name = Some(tool_name.clone());
            fields.is_error = Some(result.is_error);
            fields.latency_ms = result.duration_ms;
            fields.output_preview = Some(preview_json(&result.output));
        }
        EventPayload::ContextCompacted {
            messages_before,
            messages_after,
            tokens_before,
            tokens_after,
            files_read,
            files_modified,
            ..
        } => {
            fields.messages_before = Some(*messages_before);
            fields.messages_after = Some(*messages_after);
            fields.tokens_before = Some(*tokens_before);
            fields.tokens_after = Some(*tokens_after);
            fields.files_read_count = Some(files_read.len());
            fields.files_modified_count = Some(files_modified.len());
        }
        EventPayload::BudgetConfigured { budget } => {
            fields.status = budget
                .max_tokens
                .map(|max_tokens| format!("max_tokens={max_tokens}"));
            fields.cost_total_usd = budget.max_cost_usd;
            fields.text_preview = Some(truncate_string(&format!(
                "max_turns={:?} max_wall_clock_seconds={:?}",
                budget.max_turns, budget.max_wall_clock_seconds
            )));
        }
        EventPayload::BudgetCheckpoint {
            tokens_used,
            turns_used,
            elapsed_seconds,
            cost_used_usd,
        } => {
            fields.tokens_after = Some(*tokens_used);
            fields.text_preview = Some(truncate_string(&format!(
                "turns_used={turns_used} elapsed_seconds={elapsed_seconds}"
            )));
            fields.cost_total_usd = *cost_used_usd;
        }
        EventPayload::SessionSteersResolved {
            steer_event_ids,
            outcome,
            combined_message,
            reason,
        } => {
            fields.status = Some(format!("{outcome:?}"));
            fields.message_count = Some(steer_event_ids.len() as u32);
            fields.text_preview = combined_message
                .as_ref()
                .map(|message| truncate_string(message))
                .or_else(|| reason.as_ref().map(|reason| truncate_string(reason)));
        }
        EventPayload::OperatorCommandRecorded {
            command_type,
            raw_input,
            output,
            success,
        } => {
            fields.command_type = Some(command_type.clone());
            fields.success = Some(*success);
            fields.text_preview = Some(truncate_string(raw_input));
            fields.output_preview = Some(truncate_string(output));
        }
        EventPayload::RawChunkPersisted {
            provider,
            chunk_index,
            stream,
            llm_call_ordinal,
        } => {
            fields.provider = Some(provider.clone());
            fields.llm_call_ordinal = *llm_call_ordinal;
            fields.chunk_index = Some(*chunk_index);
            fields.status = Some(stream.clone());
        }
    }

    fields
}

fn populate_message_fields(fields: &mut MirroredEventFields, message: &Message) {
    fields.message_role = Some(format!("{:?}", message.role));
    let text = message
        .parts
        .iter()
        .filter_map(message_part_preview_text)
        .collect::<Vec<_>>()
        .join("\n");
    if !text.is_empty() {
        fields.text_preview = Some(truncate_string(&text));
    }
    for part in &message.parts {
        match part {
            MessagePart::ToolCall { call } => {
                fields.tool_name = Some(call.tool_name.clone());
                fields.call_id = Some(call.call_id.clone());
                fields.arguments_preview = Some(preview_json(&call.arguments));
                break;
            }
            MessagePart::ToolResult { result } => {
                fields.tool_name = Some(result.tool_name.clone());
                fields.call_id = Some(result.call_id.to_string());
                fields.is_error = Some(result.is_error);
                fields.latency_ms = result.duration_ms;
                fields.output_preview = Some(preview_json(&result.output));
                break;
            }
            _ => {}
        }
    }
}

pub(crate) fn turn_start_source_label(source: &bt_core::TurnStartSource) -> &'static str {
    match source {
        bt_core::TurnStartSource::UserMessage => "user_message",
        bt_core::TurnStartSource::ApprovalResume => "approval_resume",
        bt_core::TurnStartSource::InputResume => "input_resume",
        bt_core::TurnStartSource::SteerFollowUp => "steer_follow_up",
        bt_core::TurnStartSource::QueuedFollowUp => "queued_follow_up",
        bt_core::TurnStartSource::RelatedSessionMessage => "related_session_message",
    }
}
