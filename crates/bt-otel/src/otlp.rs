//! OTLP span assembly and attribute shaping live here. This module owns
//! turn/LLM/tool span derivation plus OpenInference-compatible attributes,
//! while export dispatch and mirrored tracing stay in sibling modules.

use bt_core::{
    CostBreakdown, EventEnvelope, EventPayload, Message, MessagePart, Result, SessionRecord,
    TokenUsage, TurnId, oi_attrs,
};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message as _;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::mirror::{mirrored_event_fields, turn_start_source_label};
use crate::render::{preview_json, render_message_text, role_name, serialize_json};

const OI_SPAN_KIND: &str = "openinference.span.kind";
const INPUT_MIME_TYPE: &str = "input.mime_type";
const OUTPUT_MIME_TYPE: &str = "output.mime_type";
const LLM_INPUT_MESSAGES: &str = "llm.input_messages";
const LLM_OUTPUT_MESSAGES: &str = "llm.output_messages";
const TOOL_ID: &str = "tool.id";
const TOOL_CALL_ID: &str = "tool_call.id";
const TOOL_CALL_FUNCTION_NAME: &str = "tool_call.function.name";
const TOOL_CALL_FUNCTION_ARGUMENTS: &str = "tool_call.function.arguments";

#[derive(Clone, Debug)]
struct OtlpTurnSpan {
    turn_id: TurnId,
    otlp_span_id: String,
    start: time::OffsetDateTime,
    end: time::OffsetDateTime,
    provider: Option<String>,
    model: Option<String>,
    message_count: Option<u32>,
    settings_revision_id: Option<u64>,
    source: Option<String>,
    resumed_from_call_id: Option<String>,
    status: Option<String>,
    finish_reason: Option<String>,
    latency_ms: Option<u64>,
    input_messages: Vec<Message>,
    output_messages: Vec<Message>,
    events: Vec<Value>,
    llm_spans: Vec<OtlpLlmSpan>,
    tool_spans: Vec<OtlpToolSpan>,
}

#[derive(Clone, Debug)]
struct OtlpLlmSpan {
    ordinal: u32,
    otlp_span_id: String,
    start: time::OffsetDateTime,
    end: time::OffsetDateTime,
    provider: Option<String>,
    model: Option<String>,
    message_count: Option<u32>,
    finish_reason: Option<String>,
    usage: Option<TokenUsage>,
    cost: Option<CostBreakdown>,
    chunk_count: u32,
    input_messages: Vec<Message>,
    output_messages: Vec<Message>,
    events: Vec<Value>,
}

#[derive(Clone, Debug)]
struct OtlpToolSpan {
    call_id: String,
    tool_name: String,
    otlp_span_id: String,
    start: time::OffsetDateTime,
    end: time::OffsetDateTime,
    arguments: Option<Value>,
    arguments_preview: Option<String>,
    output: Option<Value>,
    approval_decision: Option<String>,
    approval_scope: Option<String>,
    approval_source: Option<String>,
    approval_request_fingerprint: Option<String>,
    approval_arguments_hash: Option<String>,
    approval_surface: Option<String>,
    approval_risk_class: Option<String>,
    is_error: Option<bool>,
    duration_ms: Option<u64>,
    events: Vec<Value>,
}

#[derive(Clone, Debug)]
struct CachedToolRequest {
    tool_name: String,
    arguments: Option<Value>,
    arguments_preview: Option<String>,
}

#[derive(Clone, Debug)]
struct CachedApprovalRequestEvidence {
    request_fingerprint: String,
    arguments_hash: String,
    surface: String,
    risk_class: String,
}

#[derive(Default)]
struct OtlpTurnBuilder {
    start: Option<time::OffsetDateTime>,
    end: Option<time::OffsetDateTime>,
    provider: Option<String>,
    model: Option<String>,
    message_count: Option<u32>,
    settings_revision_id: Option<u64>,
    source: Option<String>,
    resumed_from_call_id: Option<String>,
    status: Option<String>,
    finish_reason: Option<String>,
    latency_ms: Option<u64>,
    input_messages: Vec<Message>,
    output_messages: Vec<Message>,
    events: Vec<Value>,
    llm_spans: Vec<OtlpLlmSpan>,
    tool_spans: Vec<OtlpToolSpan>,
}

pub(crate) fn export_otlp_json(
    session: &SessionRecord,
    events: &[EventEnvelope],
) -> Result<String> {
    Ok(serde_json::to_string_pretty(&build_otlp_export(
        session, events, None,
    ))?)
}

pub fn export_otlp_protobuf(
    session: &SessionRecord,
    events: &[EventEnvelope],
    project_name: Option<&str>,
) -> Result<Vec<u8>> {
    let request: ExportTraceServiceRequest =
        serde_json::from_value(build_otlp_export(session, events, project_name))?;
    Ok(request.encode_to_vec())
}

fn build_otlp_export(
    session: &SessionRecord,
    events: &[EventEnvelope],
    project_name: Option<&str>,
) -> Value {
    let mut turns = derive_otlp_turns(events);

    let mut spans = Vec::new();
    for turn in turns.values_mut() {
        let trace_id = otlp_trace_id(turn.turn_id.0.as_bytes());
        spans.push(json!({
            "traceId": trace_id,
            "spanId": turn.otlp_span_id,
            "name": "turn",
            "kind": 1,
            "startTimeUnixNano": unix_nanos(turn.start),
            "endTimeUnixNano": unix_nanos(turn.end),
            "attributes": turn_span_attributes(turn, session),
            "events": turn.events,
            "status": otlp_status(turn.status.as_deref() != Some("failed")),
        }));

        for llm in &turn.llm_spans {
            spans.push(json!({
                "traceId": trace_id,
                "spanId": llm.otlp_span_id,
                "parentSpanId": turn.otlp_span_id,
                "name": "llm_call",
                "kind": 1,
                "startTimeUnixNano": unix_nanos(llm.start),
                "endTimeUnixNano": unix_nanos(llm.end),
                "attributes": llm_span_attributes(llm, session, turn.turn_id),
                "events": llm.events,
                "status": otlp_status(llm.finish_reason.as_deref() != Some("error")),
            }));
        }

        for tool in &turn.tool_spans {
            spans.push(json!({
                "traceId": trace_id,
                "spanId": tool.otlp_span_id,
                "parentSpanId": turn.otlp_span_id,
                "name": "tool_execution",
                "kind": 1,
                "startTimeUnixNano": unix_nanos(tool.start),
                "endTimeUnixNano": unix_nanos(tool.end),
                "attributes": tool_span_attributes(tool, session, turn.turn_id),
                "events": tool.events,
                "status": otlp_status(tool.is_error != Some(true)),
            }));
        }
    }

    let mut resource_attributes = vec![
        string_attr("service.name", "belltower"),
        string_attr("service.version", env!("CARGO_PKG_VERSION")),
        string_attr("belltower.session.id", session.session_id.to_string()),
        string_attr("belltower.project_root", session.project_root.as_str()),
        string_attr("belltower.connection_id", session.connection_id.to_string()),
    ];
    if let Some(project_name) = project_name.filter(|value| !value.trim().is_empty()) {
        resource_attributes.push(string_attr("openinference.project.name", project_name));
    }

    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": resource_attributes
            },
            "scopeSpans": [{
                "scope": {
                    "name": "belltower.bt-otel",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "spans": spans,
            }]
        }]
    })
}

fn derive_otlp_turns(events: &[EventEnvelope]) -> BTreeMap<TurnId, OtlpTurnSpan> {
    let mut turns = BTreeMap::<TurnId, OtlpTurnBuilder>::new();
    let mut active_llm_by_turn = BTreeMap::<TurnId, BTreeMap<u32, usize>>::new();
    let mut active_tool_by_turn = BTreeMap::<TurnId, BTreeMap<String, usize>>::new();
    let mut cached_tool_requests = BTreeMap::<String, CachedToolRequest>::new();
    let mut cached_approval_requests = BTreeMap::<String, CachedApprovalRequestEvidence>::new();
    let mut message_history = Vec::<Message>::new();

    for event in events {
        if let EventPayload::MessageAppended { message } = &event.payload {
            message_history.push(message.clone());
            if let Some(turn_id) = event.turn_id
                && let Some(index) = latest_active_llm_index(&active_llm_by_turn, turn_id)
                && let Some(turn) = turns.get_mut(&turn_id)
            {
                if let Some(span) = turn.llm_spans.get_mut(index) {
                    span.output_messages.push(message.clone());
                }
            } else if let Some(turn_id) = event.turn_id {
                let turn = turns.entry(turn_id).or_default();
                match message.role {
                    bt_core::Role::User => turn.input_messages.push(message.clone()),
                    bt_core::Role::Assistant => turn.output_messages.push(message.clone()),
                    bt_core::Role::Tool | bt_core::Role::System => {}
                }
            }
        }

        let Some(turn_id) = event.turn_id else {
            continue;
        };
        let span_event = otlp_event_from_envelope(event);
        let turn = turns.entry(turn_id).or_default();
        if turn.start.is_none() {
            turn.start = Some(event.occurred_at);
        }
        turn.end = Some(event.occurred_at);
        turn.events.push(span_event.clone());

        match &event.payload {
            EventPayload::TurnStarted {
                provider,
                model,
                message_count,
                settings_revision_id,
                source,
                resumed_from_call_id,
                ..
            } => {
                turn.start = Some(event.occurred_at);
                turn.provider = Some(provider.clone());
                turn.model = Some(model.clone());
                turn.message_count = Some(*message_count);
                turn.settings_revision_id = Some(*settings_revision_id);
                turn.source = Some(turn_start_source_label(source).to_owned());
                turn.resumed_from_call_id = resumed_from_call_id.as_ref().map(ToString::to_string);
            }
            EventPayload::TurnFinished {
                provider,
                model,
                status,
                finish_reason,
                latency_ms,
                ..
            } => {
                turn.end = Some(event.occurred_at);
                turn.provider = Some(provider.clone());
                turn.model = Some(model.clone());
                turn.status = Some(status.clone());
                turn.finish_reason = finish_reason.clone();
                turn.latency_ms = Some(*latency_ms);
            }
            EventPayload::CompletionRequested {
                llm_call_ordinal,
                provider,
                model,
                message_count,
            } => {
                let input_messages = trailing_messages(&message_history, *message_count as usize);
                if turn.input_messages.is_empty() {
                    turn.input_messages = input_messages.clone();
                }
                turn.llm_spans.push(OtlpLlmSpan {
                    ordinal: *llm_call_ordinal,
                    otlp_span_id: otlp_span_id_from_bytes(&event.span_id.0.as_bytes()[..8]),
                    start: event.occurred_at,
                    end: event.occurred_at,
                    provider: Some(provider.clone()),
                    model: Some(model.clone()),
                    message_count: Some(*message_count),
                    finish_reason: None,
                    usage: None,
                    cost: None,
                    chunk_count: 0,
                    input_messages,
                    output_messages: Vec::new(),
                    events: vec![span_event],
                });
                active_llm_by_turn
                    .entry(turn_id)
                    .or_default()
                    .insert(*llm_call_ordinal, turn.llm_spans.len() - 1);
            }
            EventPayload::CompletionChunk {
                llm_call_ordinal, ..
            } => {
                if let Some(index) =
                    resolve_active_llm_index(&active_llm_by_turn, turn_id, *llm_call_ordinal)
                    && let Some(span) = turn.llm_spans.get_mut(index)
                {
                    span.end = event.occurred_at;
                    span.chunk_count += 1;
                    span.events.push(span_event);
                }
            }
            EventPayload::CompletionFinished {
                llm_call_ordinal,
                provider,
                model,
                usage,
                cost,
                finish_reason,
                ..
            } => {
                if let Some(index) = active_llm_by_turn
                    .get_mut(&turn_id)
                    .and_then(|active| active.remove(llm_call_ordinal))
                    && let Some(span) = turn.llm_spans.get_mut(index)
                {
                    span.end = event.occurred_at;
                    span.provider = Some(provider.clone());
                    span.model = Some(model.clone());
                    span.usage = Some(usage.clone());
                    span.cost = cost.clone();
                    span.finish_reason = Some(finish_reason.clone());
                    span.events.push(span_event);
                }
            }
            EventPayload::SessionError { code, .. } => {
                if let Some(index) =
                    remove_latest_active_llm_index(&mut active_llm_by_turn, turn_id)
                    && let Some(span) = turn.llm_spans.get_mut(index)
                {
                    span.end = event.occurred_at;
                    span.finish_reason = Some(code.clone());
                    span.events.push(span_event);
                }
            }
            EventPayload::ToolCallRequested {
                call_id,
                tool_name,
                arguments,
            } => {
                cached_tool_requests.insert(
                    call_id.to_string(),
                    CachedToolRequest {
                        tool_name: tool_name.clone(),
                        arguments: Some(arguments.clone()),
                        arguments_preview: Some(preview_json(arguments)),
                    },
                );
                let span = ensure_otlp_tool_span(
                    turn,
                    &mut active_tool_by_turn,
                    &cached_tool_requests,
                    &cached_approval_requests,
                    turn_id,
                    &call_id.to_string(),
                    tool_name,
                    event.occurred_at,
                    otlp_span_id_from_bytes(&event.span_id.0.as_bytes()[..8]),
                );
                span.end = event.occurred_at;
                span.arguments = Some(arguments.clone());
                span.arguments_preview = Some(preview_json(arguments));
                span.events.push(span_event);
            }
            EventPayload::ToolApprovalRequested { call_id, .. }
            | EventPayload::ToolApprovalResolved { call_id, .. }
            | EventPayload::ToolExecutionFinished { call_id, .. } => {
                let tool_name = match &event.payload {
                    EventPayload::ToolApprovalRequested { tool_name, .. }
                    | EventPayload::ToolApprovalResolved { tool_name, .. }
                    | EventPayload::ToolExecutionFinished { tool_name, .. } => tool_name.as_str(),
                    _ => unreachable!("tool event matched above"),
                };
                if let EventPayload::ToolApprovalRequested {
                    call_id,
                    snapshot: Some(snapshot),
                    ..
                } = &event.payload
                {
                    cached_approval_requests.insert(
                        call_id.to_string(),
                        CachedApprovalRequestEvidence {
                            request_fingerprint: snapshot.request_fingerprint.clone(),
                            arguments_hash: snapshot.arguments_hash.clone(),
                            surface: snapshot.surface.clone(),
                            risk_class: format!("{:?}", snapshot.tool_metadata.risk_class),
                        },
                    );
                }
                let span = ensure_otlp_tool_span(
                    turn,
                    &mut active_tool_by_turn,
                    &cached_tool_requests,
                    &cached_approval_requests,
                    turn_id,
                    &call_id.to_string(),
                    tool_name,
                    event.occurred_at,
                    otlp_span_id_from_bytes(&event.span_id.0.as_bytes()[..8]),
                );
                span.end = event.occurred_at;
                span.events.push(span_event.clone());
                if let EventPayload::ToolApprovalRequested { snapshot, .. } = &event.payload {
                    if let Some(snapshot) = snapshot {
                        span.approval_request_fingerprint =
                            Some(snapshot.request_fingerprint.clone());
                        span.approval_arguments_hash = Some(snapshot.arguments_hash.clone());
                        span.approval_surface = Some(snapshot.surface.clone());
                        span.approval_risk_class =
                            Some(format!("{:?}", snapshot.tool_metadata.risk_class));
                    }
                }
                if let EventPayload::ToolApprovalResolved {
                    decision,
                    request_fingerprint,
                    resolution,
                    ..
                } = &event.payload
                {
                    span.approval_request_fingerprint = resolution
                        .as_ref()
                        .map(|resolution| resolution.request_fingerprint.clone())
                        .or_else(|| request_fingerprint.clone())
                        .or_else(|| span.approval_request_fingerprint.clone());
                    match decision {
                        bt_core::ApprovalDecision::Approved { scope, source, .. } => {
                            span.approval_decision = Some(format!("approved:{scope:?}"));
                            span.approval_scope = Some(format!("{scope:?}"));
                            span.approval_source = Some(format!("{source:?}"));
                        }
                        bt_core::ApprovalDecision::Denied { scope, source, .. } => {
                            span.approval_decision = Some(format!("denied:{scope:?}"));
                            span.approval_scope = Some(format!("{scope:?}"));
                            span.approval_source = Some(format!("{source:?}"));
                        }
                    }
                }
                if let EventPayload::ToolExecutionFinished { result, .. } = &event.payload {
                    span.output = Some(result.output.clone());
                    span.is_error = Some(result.is_error);
                    span.duration_ms = result.duration_ms;
                }
            }
            _ => {}
        }
    }

    turns
        .into_iter()
        .filter_map(|(turn_id, builder)| {
            let start = builder.start?;
            let end = builder.end.unwrap_or(start);
            let output_messages = builder
                .llm_spans
                .iter()
                .rev()
                .find_map(|span| {
                    (!span.output_messages.is_empty()).then(|| span.output_messages.clone())
                })
                .unwrap_or(builder.output_messages);
            Some((
                turn_id,
                OtlpTurnSpan {
                    turn_id,
                    otlp_span_id: otlp_span_id_from_bytes(&turn_id.0.as_bytes()[..8]),
                    start,
                    end,
                    provider: builder.provider,
                    model: builder.model,
                    message_count: builder.message_count,
                    settings_revision_id: builder.settings_revision_id,
                    source: builder.source,
                    resumed_from_call_id: builder.resumed_from_call_id,
                    status: builder.status,
                    finish_reason: builder.finish_reason,
                    latency_ms: builder.latency_ms,
                    input_messages: builder.input_messages,
                    output_messages,
                    events: builder.events,
                    llm_spans: builder.llm_spans,
                    tool_spans: builder.tool_spans,
                },
            ))
        })
        .collect()
}

fn trailing_messages(history: &[Message], count: usize) -> Vec<Message> {
    if count == 0 {
        return Vec::new();
    }
    let start = history.len().saturating_sub(count);
    history[start..].to_vec()
}

fn resolve_active_llm_index(
    active_llm_by_turn: &BTreeMap<TurnId, BTreeMap<u32, usize>>,
    turn_id: TurnId,
    ordinal: Option<u32>,
) -> Option<usize> {
    ordinal
        .and_then(|ordinal| active_llm_by_turn.get(&turn_id)?.get(&ordinal).copied())
        .or_else(|| latest_active_llm_index(active_llm_by_turn, turn_id))
}

fn latest_active_llm_index(
    active_llm_by_turn: &BTreeMap<TurnId, BTreeMap<u32, usize>>,
    turn_id: TurnId,
) -> Option<usize> {
    active_llm_by_turn
        .get(&turn_id)
        .and_then(|active| active.iter().next_back().map(|(_, index)| *index))
}

fn remove_latest_active_llm_index(
    active_llm_by_turn: &mut BTreeMap<TurnId, BTreeMap<u32, usize>>,
    turn_id: TurnId,
) -> Option<usize> {
    let active = active_llm_by_turn.get_mut(&turn_id)?;
    let ordinal = *active.keys().next_back()?;
    let index = active.remove(&ordinal);
    if active.is_empty() {
        active_llm_by_turn.remove(&turn_id);
    }
    index
}

fn ensure_otlp_tool_span<'a>(
    turn: &'a mut OtlpTurnBuilder,
    active_tool_by_turn: &mut BTreeMap<TurnId, BTreeMap<String, usize>>,
    cached_tool_requests: &BTreeMap<String, CachedToolRequest>,
    cached_approval_requests: &BTreeMap<String, CachedApprovalRequestEvidence>,
    turn_id: TurnId,
    call_id: &str,
    tool_name: &str,
    occurred_at: time::OffsetDateTime,
    otlp_span_id: String,
) -> &'a mut OtlpToolSpan {
    let index = if let Some(index) = active_tool_by_turn
        .get(&turn_id)
        .and_then(|tool_map| tool_map.get(call_id))
        .copied()
    {
        index
    } else {
        let cached = cached_tool_requests.get(call_id);
        let cached_approval = cached_approval_requests.get(call_id);
        turn.tool_spans.push(OtlpToolSpan {
            call_id: call_id.to_owned(),
            tool_name: cached
                .map(|request| request.tool_name.clone())
                .unwrap_or_else(|| tool_name.to_owned()),
            otlp_span_id,
            start: occurred_at,
            end: occurred_at,
            arguments: cached.and_then(|request| request.arguments.clone()),
            arguments_preview: cached.and_then(|request| request.arguments_preview.clone()),
            output: None,
            approval_decision: None,
            approval_scope: None,
            approval_source: None,
            approval_request_fingerprint: cached_approval
                .map(|evidence| evidence.request_fingerprint.clone()),
            approval_arguments_hash: cached_approval
                .map(|evidence| evidence.arguments_hash.clone()),
            approval_surface: cached_approval.map(|evidence| evidence.surface.clone()),
            approval_risk_class: cached_approval.map(|evidence| evidence.risk_class.clone()),
            is_error: None,
            duration_ms: None,
            events: Vec::new(),
        });
        let index = turn.tool_spans.len() - 1;
        active_tool_by_turn
            .entry(turn_id)
            .or_default()
            .insert(call_id.to_owned(), index);
        index
    };
    turn.tool_spans
        .get_mut(index)
        .expect("tool span exists immediately after insertion or lookup")
}

fn otlp_event_from_envelope(event: &EventEnvelope) -> Value {
    let mirrored = mirrored_event_fields(event);
    let mut attributes = vec![
        string_attr("belltower.event.kind", event.kind()),
        string_attr("belltower.event.id", event.event_id.to_string()),
        string_attr("belltower.branch_id", event.branch_id.to_string()),
        string_attr("belltower.span_kind", format!("{:?}", event.span_kind)),
    ];
    if let Some(seq_id) = event.seq_id {
        attributes.push(int_attr("belltower.seq_id", seq_id as u64));
    }
    if let Some(turn_id) = event.turn_id {
        attributes.push(string_attr("belltower.turn_id", turn_id.to_string()));
    }
    for (key, value) in &event.attributes {
        push_json_attribute(&mut attributes, key, value);
    }
    push_optional_string_attr(
        &mut attributes,
        "belltower.provider",
        mirrored.provider.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.model",
        mirrored.model.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.tool_name",
        mirrored.tool_name.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.call_id",
        mirrored.call_id.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.status",
        mirrored.status.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.finish_reason",
        mirrored.finish_reason.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.approval_decision",
        mirrored.approval_decision.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.approval_scope",
        mirrored.approval_scope.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.approval_source",
        mirrored.approval_source.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.approval_request_fingerprint",
        mirrored.approval_request_fingerprint.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.approval_arguments_hash",
        mirrored.approval_arguments_hash.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.approval_surface",
        mirrored.approval_surface.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.approval_risk_class",
        mirrored.approval_risk_class.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.turn_start_source",
        mirrored.turn_start_source.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.resumed_from_call_id",
        mirrored.resumed_from_call_id.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.message_role",
        mirrored.message_role.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.command_type",
        mirrored.command_type.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.text_preview",
        mirrored.text_preview.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.arguments_preview",
        mirrored.arguments_preview.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.output_preview",
        mirrored.output_preview.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.delta_text_preview",
        mirrored.delta_text_preview.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.delta_tool_call_preview",
        mirrored.delta_tool_call_preview.as_deref(),
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.message_count",
        mirrored.message_count.map(u64::from),
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.settings_revision_id",
        mirrored.settings_revision_id,
    );
    push_optional_u64_attr(&mut attributes, "belltower.latency_ms", mirrored.latency_ms);
    push_optional_i64_attr(
        &mut attributes,
        "belltower.chunk_index",
        mirrored.chunk_index,
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.llm_call_ordinal",
        mirrored.llm_call_ordinal.map(u64::from),
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.messages_before",
        mirrored.messages_before.map(u64::from),
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.messages_after",
        mirrored.messages_after.map(u64::from),
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.tokens_before",
        mirrored.tokens_before,
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.tokens_after",
        mirrored.tokens_after,
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.files_read_count",
        mirrored.files_read_count.map(|value| value as u64),
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.files_modified_count",
        mirrored.files_modified_count.map(|value| value as u64),
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.prompt_tokens",
        mirrored.prompt_tokens,
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.completion_tokens",
        mirrored.completion_tokens,
    );
    push_optional_u64_attr(
        &mut attributes,
        "belltower.total_tokens",
        mirrored.total_tokens,
    );
    push_optional_f64_attr(
        &mut attributes,
        "belltower.cost_total_usd",
        mirrored.cost_total_usd,
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.error_class",
        mirrored.error_class.as_deref(),
    );
    push_optional_string_attr(
        &mut attributes,
        "belltower.error_code",
        mirrored.error_code.as_deref(),
    );
    push_optional_bool_attr(
        &mut attributes,
        "belltower.error_retryable",
        mirrored.error_retryable,
    );
    push_optional_bool_attr(&mut attributes, "belltower.success", mirrored.success);
    push_optional_bool_attr(&mut attributes, "belltower.is_error", mirrored.is_error);

    json!({
        "timeUnixNano": unix_nanos(event.occurred_at),
        "name": event.kind(),
        "attributes": attributes,
    })
}

fn turn_span_attributes(turn: &OtlpTurnSpan, session: &SessionRecord) -> Vec<Value> {
    let mut attributes = vec![
        string_attr(OI_SPAN_KIND, "AGENT"),
        string_attr("session.id", session.session_id.to_string()),
        string_attr("turn.id", turn.turn_id.to_string()),
    ];
    if let Some(provider) = &turn.provider {
        attributes.push(string_attr("gen_ai.system", provider));
    }
    if let Some(model) = &turn.model {
        attributes.push(string_attr("gen_ai.request.model", model));
    }
    if let Some(message_count) = turn.message_count {
        attributes.push(int_attr(
            "belltower.turn.message_count",
            message_count as u64,
        ));
    }
    if let Some(settings_revision_id) = turn.settings_revision_id {
        attributes.push(int_attr(
            "belltower.settings_revision_id",
            settings_revision_id,
        ));
    }
    if let Some(source) = &turn.source {
        attributes.push(string_attr("belltower.turn.source", source));
    }
    if let Some(resumed_from_call_id) = &turn.resumed_from_call_id {
        attributes.push(string_attr(
            "belltower.resumed_from_call_id",
            resumed_from_call_id,
        ));
    }
    if let Some(status) = &turn.status {
        attributes.push(string_attr("belltower.turn.status", status));
    }
    if let Some(finish_reason) = &turn.finish_reason {
        attributes.push(string_attr("belltower.turn.finish_reason", finish_reason));
    }
    if let Some(latency_ms) = turn.latency_ms {
        attributes.push(int_attr("belltower.turn.latency_ms", latency_ms));
    }
    push_message_bundle_attributes(
        &mut attributes,
        &turn.input_messages,
        oi_attrs::INPUT_VALUE,
        INPUT_MIME_TYPE,
        None,
    );
    push_message_bundle_attributes(
        &mut attributes,
        &turn.output_messages,
        oi_attrs::OUTPUT_VALUE,
        OUTPUT_MIME_TYPE,
        None,
    );
    attributes
}

fn llm_span_attributes(llm: &OtlpLlmSpan, session: &SessionRecord, turn_id: TurnId) -> Vec<Value> {
    let mut attributes = vec![
        string_attr(OI_SPAN_KIND, "LLM"),
        string_attr("session.id", session.session_id.to_string()),
        string_attr("turn.id", turn_id.to_string()),
        int_attr("belltower.llm_call.ordinal", llm.ordinal as u64),
        int_attr("belltower.llm_call.chunk_count", llm.chunk_count as u64),
    ];
    if let Some(provider) = &llm.provider {
        attributes.push(string_attr(oi_attrs::LLM_PROVIDER, provider));
        attributes.push(string_attr(oi_attrs::LLM_SYSTEM, provider));
        attributes.push(string_attr("gen_ai.system", provider));
    }
    if let Some(model) = &llm.model {
        attributes.push(string_attr(oi_attrs::LLM_MODEL_NAME, model));
        attributes.push(string_attr("gen_ai.request.model", model));
    }
    if let Some(message_count) = llm.message_count {
        attributes.push(int_attr(
            "belltower.llm_call.message_count",
            message_count as u64,
        ));
    }
    if let Some(finish_reason) = &llm.finish_reason {
        attributes.push(string_attr("gen_ai.response.finish_reason", finish_reason));
    }
    if let Some(usage) = &llm.usage {
        attributes.push(int_attr(oi_attrs::LLM_TOKEN_PROMPT, usage.prompt_tokens));
        attributes.push(int_attr(
            oi_attrs::LLM_TOKEN_COMPLETION,
            usage.completion_tokens,
        ));
        attributes.push(int_attr(oi_attrs::LLM_TOKEN_TOTAL, usage.total_tokens));
        if let Some(cache_read_tokens) = usage.cache_read_tokens {
            attributes.push(int_attr(
                oi_attrs::LLM_TOKEN_PROMPT_DETAILS_CACHE_READ,
                cache_read_tokens,
            ));
        }
        if let Some(cache_write_tokens) = usage.cache_write_tokens {
            attributes.push(int_attr(
                oi_attrs::LLM_TOKEN_PROMPT_DETAILS_CACHE_WRITE,
                cache_write_tokens,
            ));
        }
        if let Some(reasoning_tokens) = usage.reasoning_tokens {
            attributes.push(int_attr(
                oi_attrs::LLM_TOKEN_COMPLETION_DETAILS_REASONING,
                reasoning_tokens,
            ));
        }
        attributes.push(int_attr("gen_ai.usage.input_tokens", usage.prompt_tokens));
        attributes.push(int_attr(
            "gen_ai.usage.output_tokens",
            usage.completion_tokens,
        ));
        attributes.push(int_attr("gen_ai.usage.total_tokens", usage.total_tokens));
    }
    if let Some(cost) = &llm.cost {
        attributes.push(double_attr(oi_attrs::LLM_COST_PROMPT, cost.prompt_usd));
        attributes.push(double_attr(
            oi_attrs::LLM_COST_COMPLETION,
            cost.completion_usd,
        ));
        attributes.push(double_attr(oi_attrs::LLM_COST_TOTAL, cost.total_usd));
        attributes.push(double_attr("belltower.cost.prompt_usd", cost.prompt_usd));
        attributes.push(double_attr(
            "belltower.cost.completion_usd",
            cost.completion_usd,
        ));
        attributes.push(double_attr("belltower.cost.total_usd", cost.total_usd));
    }
    push_message_bundle_attributes(
        &mut attributes,
        &llm.input_messages,
        oi_attrs::INPUT_VALUE,
        INPUT_MIME_TYPE,
        Some(LLM_INPUT_MESSAGES),
    );
    push_message_bundle_attributes(
        &mut attributes,
        &llm.output_messages,
        oi_attrs::OUTPUT_VALUE,
        OUTPUT_MIME_TYPE,
        Some(LLM_OUTPUT_MESSAGES),
    );
    attributes
}

fn tool_span_attributes(
    tool: &OtlpToolSpan,
    session: &SessionRecord,
    turn_id: TurnId,
) -> Vec<Value> {
    let mut attributes = vec![
        string_attr(OI_SPAN_KIND, "TOOL"),
        string_attr("session.id", session.session_id.to_string()),
        string_attr("turn.id", turn_id.to_string()),
        string_attr("tool.name", &tool.tool_name),
        string_attr(TOOL_ID, &tool.call_id),
        string_attr(TOOL_CALL_ID, &tool.call_id),
        string_attr("belltower.tool.call_id", &tool.call_id),
    ];
    if let Some(arguments) = &tool.arguments {
        let serialized = serialize_json(arguments);
        attributes.push(string_attr(oi_attrs::TOOL_PARAMETERS, serialized.clone()));
        attributes.push(string_attr(oi_attrs::INPUT_VALUE, serialized));
        attributes.push(string_attr(INPUT_MIME_TYPE, "application/json"));
        attributes.push(string_attr(TOOL_CALL_FUNCTION_NAME, &tool.tool_name));
        attributes.push(string_attr(
            TOOL_CALL_FUNCTION_ARGUMENTS,
            serialize_json(arguments),
        ));
    } else if let Some(arguments_preview) = &tool.arguments_preview {
        attributes.push(string_attr(oi_attrs::TOOL_PARAMETERS, arguments_preview));
    }
    if let Some(output) = &tool.output {
        attributes.push(string_attr(oi_attrs::OUTPUT_VALUE, serialize_json(output)));
        attributes.push(string_attr(OUTPUT_MIME_TYPE, "application/json"));
    }
    if let Some(approval_decision) = &tool.approval_decision {
        attributes.push(string_attr("belltower.tool.approval", approval_decision));
    }
    if let Some(approval_scope) = &tool.approval_scope {
        attributes.push(string_attr("belltower.tool.approval_scope", approval_scope));
    }
    if let Some(approval_source) = &tool.approval_source {
        attributes.push(string_attr(
            "belltower.tool.approval_source",
            approval_source,
        ));
    }
    if let Some(request_fingerprint) = &tool.approval_request_fingerprint {
        attributes.push(string_attr(
            "belltower.tool.approval_request_fingerprint",
            request_fingerprint,
        ));
    }
    if let Some(arguments_hash) = &tool.approval_arguments_hash {
        attributes.push(string_attr(
            "belltower.tool.approval_arguments_hash",
            arguments_hash,
        ));
    }
    if let Some(surface) = &tool.approval_surface {
        attributes.push(string_attr("belltower.tool.approval_surface", surface));
    }
    if let Some(risk_class) = &tool.approval_risk_class {
        attributes.push(string_attr(
            "belltower.tool.approval_risk_class",
            risk_class,
        ));
    }
    if let Some(is_error) = tool.is_error {
        attributes.push(bool_attr("belltower.tool.is_error", is_error));
    }
    if let Some(duration_ms) = tool.duration_ms {
        attributes.push(int_attr("belltower.tool.duration_ms", duration_ms));
    }
    attributes
}

fn push_message_bundle_attributes(
    attributes: &mut Vec<Value>,
    messages: &[Message],
    value_key: &str,
    mime_key: &str,
    flattened_prefix: Option<&str>,
) {
    if messages.is_empty() {
        return;
    }
    attributes.push(string_attr(
        value_key,
        serialize_json(&messages_to_oi_json(messages)),
    ));
    attributes.push(string_attr(mime_key, "application/json"));
    if let Some(prefix) = flattened_prefix {
        flatten_messages(attributes, prefix, messages);
    }
}

fn flatten_messages(attributes: &mut Vec<Value>, prefix: &str, messages: &[Message]) {
    for (message_index, message) in messages.iter().enumerate() {
        let message_prefix = format!("{prefix}.{message_index}.message");
        attributes.push(string_attr(
            &format!("{message_prefix}.role"),
            role_name(&message.role),
        ));
        let content = render_message_text(message);
        if !content.is_empty() {
            attributes.push(string_attr(&format!("{message_prefix}.content"), content));
        }
        if let Some(result) = message.tool_result() {
            attributes.push(string_attr(
                &format!("{message_prefix}.name"),
                result.tool_name.clone(),
            ));
            attributes.push(string_attr(
                &format!("{message_prefix}.tool_call_id"),
                result.call_id.to_string(),
            ));
        }
        for (tool_index, call) in message
            .parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::ToolCall { call } => Some(call),
                _ => None,
            })
            .enumerate()
        {
            let tool_prefix = format!("{message_prefix}.tool_calls.{tool_index}.tool_call");
            attributes.push(string_attr(
                &format!("{tool_prefix}.id"),
                call.call_id.clone(),
            ));
            attributes.push(string_attr(
                &format!("{tool_prefix}.function.name"),
                call.tool_name.clone(),
            ));
            attributes.push(string_attr(
                &format!("{tool_prefix}.function.arguments"),
                serialize_json(&call.arguments),
            ));
        }
    }
}

fn messages_to_oi_json(messages: &[Message]) -> Vec<Value> {
    messages.iter().map(message_to_oi_json).collect()
}

fn message_to_oi_json(message: &Message) -> Value {
    let mut object = serde_json::Map::new();
    object.insert(
        "role".to_owned(),
        Value::String(role_name(&message.role).to_owned()),
    );
    let content = render_message_text(message);
    if !content.is_empty() {
        object.insert("content".to_owned(), Value::String(content));
    }
    if let Some(result) = message.tool_result() {
        object.insert("name".to_owned(), Value::String(result.tool_name.clone()));
        object.insert(
            "tool_call_id".to_owned(),
            Value::String(result.call_id.to_string()),
        );
    }
    let tool_calls = message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::ToolCall { call } => Some(json!({
                "id": call.call_id,
                "function": {
                    "name": call.tool_name,
                    "arguments": call.arguments,
                }
            })),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !tool_calls.is_empty() {
        object.insert("tool_calls".to_owned(), Value::Array(tool_calls));
    }
    Value::Object(object)
}

fn push_json_attribute(attributes: &mut Vec<Value>, key: &str, value: &Value) {
    match value {
        Value::Null => {}
        Value::Bool(value) => attributes.push(bool_attr(key, *value)),
        Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                attributes.push(int_attr(key, value));
            } else if let Some(value) = number.as_i64() {
                if value >= 0 {
                    attributes.push(int_attr(key, value as u64));
                } else {
                    attributes.push(string_attr(key, value.to_string()));
                }
            } else if let Some(value) = number.as_f64() {
                attributes.push(double_attr(key, value));
            }
        }
        Value::String(value) => attributes.push(string_attr(key, value)),
        Value::Array(_) | Value::Object(_) => {
            attributes.push(string_attr(key, serialize_json(value)))
        }
    }
}

fn push_optional_string_attr(attributes: &mut Vec<Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        attributes.push(string_attr(key, value));
    }
}

fn push_optional_u64_attr(attributes: &mut Vec<Value>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        attributes.push(int_attr(key, value));
    }
}

fn push_optional_i64_attr(attributes: &mut Vec<Value>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        if value >= 0 {
            attributes.push(int_attr(key, value as u64));
        } else {
            attributes.push(string_attr(key, value.to_string()));
        }
    }
}

fn push_optional_f64_attr(attributes: &mut Vec<Value>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        attributes.push(double_attr(key, value));
    }
}

fn push_optional_bool_attr(attributes: &mut Vec<Value>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        attributes.push(bool_attr(key, value));
    }
}

fn string_attr(key: &str, value: impl Into<String>) -> Value {
    json!({
        "key": key,
        "value": {
            "stringValue": value.into(),
        }
    })
}

fn int_attr(key: &str, value: u64) -> Value {
    json!({
        "key": key,
        "value": {
            "intValue": value.to_string(),
        }
    })
}

fn double_attr(key: &str, value: f64) -> Value {
    json!({
        "key": key,
        "value": {
            "doubleValue": value,
        }
    })
}

fn bool_attr(key: &str, value: bool) -> Value {
    json!({
        "key": key,
        "value": {
            "boolValue": value,
        }
    })
}

fn otlp_status(ok: bool) -> Value {
    if ok {
        json!({ "code": 1 })
    } else {
        json!({ "code": 2 })
    }
}

fn unix_nanos(timestamp: time::OffsetDateTime) -> String {
    timestamp.unix_timestamp_nanos().to_string()
}

fn otlp_trace_id(bytes: &[u8]) -> String {
    uuid_to_hex(bytes)
}

fn otlp_span_id_from_bytes(bytes: &[u8]) -> String {
    uuid_to_hex(bytes)
}

fn uuid_to_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}
