//! Focused crate tests live here so the public crate root stays small. These
//! tests pin export and mirrored-event behavior; implementation details remain
//! in the sibling modules they exercise.

use crate::export_session;
use crate::mirror::mirrored_event_fields;
use bt_core::{
    ApprovalDecision, ApprovalDecisionSource, ApprovalRequestSnapshot, ApprovalRequirement,
    ApprovalResolution, ApprovalScope, BranchId, CompletionDelta, ConnectionId, EventEnvelope,
    EventId, EventPayload, Message, MessageId, MessagePart, RelatedSessionDeliveryMode,
    RelatedSessionMessage, RelatedSessionMessageDirection, RelatedSessionMessageId,
    RelatedSessionMessageKind, RelatedSessionMessageStatus, Role, SessionId, SessionRecord,
    SessionStatus, SessionToolMode, SpanKind, ToolCall, ToolCallId, ToolDisplayGroup,
    ToolExecutionMode, ToolInterruptBehavior, ToolMetadata, ToolOperationInitiator,
    ToolResultEnvelope, ToolRiskClass, TurnId, TurnStartSource, default_settings_revision_id,
};
use bt_protocol::ExportFormat;
use camino::Utf8PathBuf;
use serde_json::Value;

fn session() -> SessionRecord {
    SessionRecord {
        session_id: SessionId::new(),
        project_root: Utf8PathBuf::from("/tmp/project"),
        connection_id: ConnectionId::new("local"),
        model_id: None,
        tool_mode: SessionToolMode::Extended,
        settings_revision_id: default_settings_revision_id(),
        created_at: time::OffsetDateTime::now_utc(),
        updated_at: time::OffsetDateTime::now_utc(),
        status: SessionStatus::Active,
        display_name: Some("Example".to_owned()),
        objective: Some("Ship a fix".to_owned()),
        parent_session_id: None,
        parent_branch_id: None,
        parent_turn_id: None,
    }
}

fn approval_snapshot(
    session_id: SessionId,
    call_id: ToolCallId,
    tool_name: &str,
) -> ApprovalRequestSnapshot {
    ApprovalRequestSnapshot {
        session_id,
        call_id,
        tool_name: tool_name.to_owned(),
        request_fingerprint: format!("{tool_name}:fnv1a64:approval"),
        arguments_hash: "fnv1a64:arguments".to_owned(),
        redacted_arguments_preview: Some(serde_json::json!({"path": "Cargo.toml"})),
        requirement: ApprovalRequirement::Always,
        tool_metadata: ToolMetadata {
            risk_class: ToolRiskClass::High,
            is_read_only: false,
            is_concurrency_safe: false,
            interrupt_behavior: ToolInterruptBehavior::WaitForCompletion,
            execution_mode: ToolExecutionMode::Immediate,
            should_defer: false,
            catalogue_tags: vec!["test".to_owned()],
            display_group: ToolDisplayGroup::Execution,
        },
        initiator: ToolOperationInitiator::Agent,
        surface: "agent_turn".to_owned(),
        registry_schema_hash: Some("schema:abc".to_owned()),
        policy_rule: Some("always".to_owned()),
        requested_at: time::OffsetDateTime::now_utc(),
    }
}

fn span_by_name<'a>(document: &'a Value, name: &str) -> &'a Value {
    document["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .expect("spans array")
        .iter()
        .find(|span| span["name"] == name)
        .unwrap_or_else(|| panic!("missing span named {name}"))
}

fn spans_by_name<'a>(document: &'a Value, name: &str) -> Vec<&'a Value> {
    document["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .expect("spans array")
        .iter()
        .filter(|span| span["name"] == name)
        .collect()
}

fn span_attr<'a>(span: &'a Value, key: &str) -> Option<&'a Value> {
    span["attributes"]
        .as_array()
        .expect("attributes array")
        .iter()
        .find(|attribute| attribute["key"] == key)
        .map(|attribute| &attribute["value"])
}

fn event_attr<'a>(span: &'a Value, event_name: &str, key: &str) -> Option<&'a Value> {
    span["events"]
        .as_array()
        .expect("events array")
        .iter()
        .find(|event| event["name"] == event_name)
        .and_then(|event| {
            event["attributes"]
                .as_array()
                .expect("event attributes array")
                .iter()
                .find(|attribute| attribute["key"] == key)
        })
        .map(|attribute| &attribute["value"])
}

#[test]
fn exports_jsonl_html_sharegpt_and_otlp() {
    let session = session();
    let branch_id = BranchId::new();
    let turn_id = TurnId::new();
    let events = vec![
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "hello"),
            },
        ),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnStarted {
                turn_id,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                message_count: 1,
                settings_revision_id: default_settings_revision_id(),
                source: TurnStartSource::UserMessage,
                resumed_from_call_id: None,
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnFinished {
                turn_id,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                status: "completed".to_owned(),
                finish_reason: Some("stop".to_owned()),
                latency_ms: 1,
            },
        )
        .with_turn_id(turn_id),
    ];
    let messages = vec![
        Message::text(Role::User, "hello"),
        Message::text(Role::Assistant, "world"),
    ];

    let jsonl =
        export_session(&session, &messages, &events, ExportFormat::Jsonl).expect("jsonl export");
    assert!(jsonl.content.contains("\"event_id\""));

    let html =
        export_session(&session, &messages, &events, ExportFormat::Html).expect("html export");
    assert!(html.content.contains("<!doctype html>"));
    assert!(html.content.contains("hello"));

    let sharegpt = export_session(&session, &messages, &events, ExportFormat::ShareGpt)
        .expect("sharegpt export");
    assert!(sharegpt.content.contains("\"conversations\""));
    assert!(sharegpt.content.contains("\"human\""));
    assert!(sharegpt.content.contains("\"gpt\""));

    let otlp =
        export_session(&session, &messages, &events, ExportFormat::Otlp).expect("otlp export");
    assert!(otlp.content.contains("\"resourceSpans\""));
    assert!(otlp.content.contains("\"service.name\""));
    assert!(otlp.content.contains("\"session.id\""));
}

#[test]
fn mirrored_fields_capture_completion_and_turn_metadata() {
    let session = session();
    let branch_id = BranchId::new();
    let turn_id = bt_core::TurnId::new();
    let event = EventEnvelope::new(
        session.session_id,
        branch_id,
        SpanKind::Llm,
        EventPayload::CompletionFinished {
            llm_call_ordinal: 1,
            provider: "openai-compatible".to_owned(),
            model: "o4-mini".to_owned(),
            usage: bt_core::TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 4,
                total_tokens: 14,
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
            cost: Some(bt_core::CostBreakdown {
                prompt_usd: 0.1,
                completion_usd: 0.2,
                total_usd: 0.3,
                cache_read_usd: None,
                cache_write_usd: None,
                reasoning_usd: None,
            }),
            finish_reason: "stop".to_owned(),
            latency_ms: 320,
        },
    )
    .with_turn_id(turn_id);
    let fields = mirrored_event_fields(&event);
    let turn_id_text = turn_id.to_string();

    assert_eq!(fields.event_kind, "completion.finished");
    assert_eq!(fields.provider.as_deref(), Some("openai-compatible"));
    assert_eq!(fields.model.as_deref(), Some("o4-mini"));
    assert_eq!(fields.turn_id.as_deref(), Some(turn_id_text.as_str()));
    assert_eq!(fields.llm_call_ordinal, Some(1));
    assert_eq!(fields.total_tokens, Some(14));
    assert_eq!(fields.cost_total_usd, Some(0.3));
    assert_eq!(fields.finish_reason.as_deref(), Some("stop"));
}

#[test]
fn mirrored_fields_capture_tool_and_operator_previews() {
    let session = session();
    let branch_id = BranchId::new();
    let tool_event = EventEnvelope::new(
        session.session_id,
        branch_id,
        SpanKind::Tool,
        EventPayload::ToolExecutionFinished {
            call_id: ToolCallId::new("call_1"),
            tool_name: "shell".to_owned(),
            result: ToolResultEnvelope {
                call_id: ToolCallId::new("call_1"),
                tool_name: "shell".to_owned(),
                is_error: false,
                output: serde_json::json!({"stdout": "ok"}),
                duration_ms: Some(55),
            },
        },
    );
    let tool_fields = mirrored_event_fields(&tool_event);
    assert_eq!(tool_fields.tool_name.as_deref(), Some("shell"));
    assert_eq!(tool_fields.call_id.as_deref(), Some("call_1"));
    assert_eq!(tool_fields.is_error, Some(false));
    assert!(
        tool_fields
            .output_preview
            .unwrap_or_default()
            .contains("stdout")
    );

    let command_event = EventEnvelope::new(
        session.session_id,
        branch_id,
        SpanKind::Agent,
        EventPayload::OperatorCommandRecorded {
            command_type: "slash".to_owned(),
            raw_input: "/history 5".to_owned(),
            output: "Rendered 5 events".to_owned(),
            success: true,
        },
    );
    let command_fields = mirrored_event_fields(&command_event);
    assert_eq!(command_fields.command_type.as_deref(), Some("slash"));
    assert_eq!(command_fields.success, Some(true));
    assert_eq!(command_fields.text_preview.as_deref(), Some("/history 5"));
    assert_eq!(
        command_fields.output_preview.as_deref(),
        Some("Rendered 5 events")
    );
}

#[test]
fn mirrored_fields_capture_approval_scope_and_decision() {
    let session = session();
    let branch_id = BranchId::new();
    let call_id = ToolCallId::new("call_9");
    let request_fingerprint = "shell:fnv1a64:approval".to_owned();
    let decision = ApprovalDecision::Approved {
        decided_at: time::OffsetDateTime::now_utc(),
        decided_by: "operator".to_owned(),
        scope: ApprovalScope::Session,
        source: ApprovalDecisionSource::Human,
    };
    let event = EventEnvelope::new(
        session.session_id,
        branch_id,
        SpanKind::Tool,
        EventPayload::ToolApprovalResolved {
            call_id,
            tool_name: "shell".to_owned(),
            request_fingerprint: Some(request_fingerprint.clone()),
            resolution: Some(ApprovalResolution {
                request_fingerprint: request_fingerprint.clone(),
                decision: decision.clone(),
            }),
            decision,
        },
    );
    let fields = mirrored_event_fields(&event);

    assert_eq!(fields.approval_decision.as_deref(), Some("approved"));
    assert_eq!(fields.approval_scope.as_deref(), Some("Session"));
    assert_eq!(
        fields.approval_request_fingerprint.as_deref(),
        Some(request_fingerprint.as_str())
    );
}

#[test]
fn mirrored_fields_capture_approval_request_snapshot() {
    let session = session();
    let branch_id = BranchId::new();
    let call_id = ToolCallId::new("call_snapshot");
    let snapshot = approval_snapshot(session.session_id, call_id.clone(), "shell");
    let event = EventEnvelope::new(
        session.session_id,
        branch_id,
        SpanKind::Tool,
        EventPayload::ToolApprovalRequested {
            call_id,
            tool_name: "shell".to_owned(),
            snapshot: Some(snapshot.clone()),
        },
    );
    let fields = mirrored_event_fields(&event);

    assert_eq!(fields.status.as_deref(), Some("pending"));
    assert_eq!(
        fields.approval_request_fingerprint.as_deref(),
        Some(snapshot.request_fingerprint.as_str())
    );
    assert_eq!(
        fields.approval_arguments_hash.as_deref(),
        Some(snapshot.arguments_hash.as_str())
    );
    assert_eq!(fields.approval_surface.as_deref(), Some("agent_turn"));
    assert_eq!(fields.approval_risk_class.as_deref(), Some("High"));
}

#[test]
fn mirrored_fields_capture_turn_start_provenance() {
    let session = session();
    let branch_id = BranchId::new();
    let turn_id = TurnId::new();
    let call_id = ToolCallId::new("call_resume");
    let event = EventEnvelope::new(
        session.session_id,
        branch_id,
        SpanKind::Agent,
        EventPayload::TurnStarted {
            turn_id,
            provider: "openai".to_owned(),
            model: "o4-mini".to_owned(),
            message_count: 2,
            settings_revision_id: 7,
            source: TurnStartSource::ApprovalResume,
            resumed_from_call_id: Some(call_id.clone()),
        },
    )
    .with_turn_id(turn_id);

    let fields = mirrored_event_fields(&event);

    assert_eq!(fields.settings_revision_id, Some(7));
    assert_eq!(fields.turn_start_source.as_deref(), Some("approval_resume"));
    assert_eq!(
        fields.resumed_from_call_id.as_deref(),
        Some(call_id.to_string().as_str())
    );
}

#[test]
fn mirrored_fields_capture_related_session_delivery_and_resolution() {
    let parent = session();
    let parent_branch_id = BranchId::new();
    let child_session_id = SessionId::new();
    let child_branch_id = BranchId::new();
    let message_id = RelatedSessionMessageId::new();
    let counterpart_event_id = EventId::new();
    let message = RelatedSessionMessage {
        message_id,
        context_message_id: MessageId::new(),
        source_session_id: parent.session_id,
        source_branch_id: parent_branch_id,
        caused_by_turn_id: None,
        destination_session_id: child_session_id,
        destination_branch_id: child_branch_id,
        kind: RelatedSessionMessageKind::Instruction,
        delivery_mode: RelatedSessionDeliveryMode::Wake,
        in_reply_to: None,
        text: "check the counterexample".to_owned(),
        artifact_refs: Vec::new(),
        created_at: time::OffsetDateTime::now_utc(),
    };
    let recorded = EventEnvelope::new(
        parent.session_id,
        parent_branch_id,
        SpanKind::Chain,
        EventPayload::RelatedSessionMessageRecorded {
            direction: RelatedSessionMessageDirection::Sent,
            counterpart_event_id,
            message,
        },
    );

    let recorded_fields = mirrored_event_fields(&recorded);
    assert_eq!(
        recorded_fields.event_kind,
        "session.related_message.recorded"
    );
    assert_eq!(recorded_fields.status.as_deref(), Some("Sent"));
    assert_eq!(
        recorded_fields.related_message_id.as_deref(),
        Some(message_id.to_string().as_str())
    );
    assert_eq!(
        recorded_fields.related_message_direction.as_deref(),
        Some("Sent")
    );
    assert_eq!(
        recorded_fields.related_message_peer_session_id.as_deref(),
        Some(child_session_id.to_string().as_str())
    );
    assert_eq!(
        recorded_fields.related_message_delivery_mode.as_deref(),
        Some("Wake")
    );
    assert_eq!(
        recorded_fields.related_message_kind.as_deref(),
        Some("Instruction")
    );
    assert_eq!(
        recorded_fields.related_message_status.as_deref(),
        Some("Delivered")
    );
    assert_eq!(
        recorded_fields.text_preview.as_deref(),
        Some("check the counterexample")
    );
    let recorded_summary = recorded_fields.finish_reason.expect("delivery summary");
    assert!(recorded_summary.contains("delivery=Wake"));
    assert!(recorded_summary.contains(&child_session_id.to_string()));

    let resulting_turn_id = TurnId::new();
    let resolved = EventEnvelope::new(
        child_session_id,
        child_branch_id,
        SpanKind::Chain,
        EventPayload::RelatedSessionMessageResolved {
            message_id,
            status: RelatedSessionMessageStatus::Claimed,
            resulting_turn_id: Some(resulting_turn_id),
            reason: None,
        },
    );
    let resolved_fields = mirrored_event_fields(&resolved);
    assert_eq!(
        resolved_fields.event_kind,
        "session.related_message.resolved"
    );
    assert_eq!(resolved_fields.status.as_deref(), Some("Claimed"));
    assert_eq!(
        resolved_fields.related_message_id.as_deref(),
        Some(message_id.to_string().as_str())
    );
    assert_eq!(
        resolved_fields.related_message_status.as_deref(),
        Some("Claimed")
    );
    assert_eq!(
        resolved_fields.related_message_resulting_turn_id.as_deref(),
        Some(resulting_turn_id.to_string().as_str())
    );
    let resolution_summary = resolved_fields.finish_reason.expect("resolution summary");
    assert!(resolution_summary.contains(&message_id.to_string()));
    assert!(resolution_summary.contains(&resulting_turn_id.to_string()));
}

#[test]
fn otlp_export_sets_openinference_span_kinds_and_llm_io() {
    let session = session();
    let branch_id = BranchId::new();
    let turn_id = TurnId::new();
    let user_message = Message::text(Role::User, "Summarize the repo.");
    let assistant_message = Message::new(
        Role::Assistant,
        vec![
            MessagePart::Reasoning {
                text: Some("Inspecting the workspace.".to_owned()),
                redacted: false,
                opaque_replay: None,
            },
            MessagePart::Text {
                text: "This repo contains Belltower.".to_owned(),
            },
        ],
    );
    let events = vec![
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: user_message,
            },
        ),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnStarted {
                turn_id,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                message_count: 1,
                settings_revision_id: default_settings_revision_id(),
                source: TurnStartSource::UserMessage,
                resumed_from_call_id: None,
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::CompletionRequested {
                llm_call_ordinal: 1,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                message_count: 1,
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: assistant_message,
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::CompletionChunk {
                llm_call_ordinal: Some(1),
                deltas: vec![CompletionDelta::AppendText {
                    text: "This repo contains Belltower.".to_owned(),
                }],
                raw_chunk_index: Some(0),
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::CompletionFinished {
                llm_call_ordinal: 1,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                usage: bt_core::TokenUsage {
                    prompt_tokens: 8,
                    completion_tokens: 5,
                    total_tokens: 13,
                    cache_read_tokens: Some(3),
                    cache_write_tokens: Some(1),
                    reasoning_tokens: Some(2),
                },
                cost: Some(bt_core::CostBreakdown {
                    prompt_usd: 0.1,
                    completion_usd: 0.2,
                    total_usd: 0.3,
                    cache_read_usd: None,
                    cache_write_usd: None,
                    reasoning_usd: Some(0.05),
                }),
                finish_reason: "stop".to_owned(),
                latency_ms: 42,
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnFinished {
                turn_id,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                status: "completed".to_owned(),
                finish_reason: Some("stop".to_owned()),
                latency_ms: 42,
            },
        )
        .with_turn_id(turn_id),
    ];

    let otlp = export_session(&session, &[], &events, ExportFormat::Otlp).expect("otlp export");
    let document: Value = serde_json::from_str(&otlp.content).expect("valid otlp json");

    let turn_span = span_by_name(&document, "turn");
    let llm_span = span_by_name(&document, "llm_call");

    assert_eq!(
        span_attr(turn_span, "openinference.span.kind")
            .and_then(|value| value["stringValue"].as_str()),
        Some("AGENT")
    );
    assert_eq!(
        span_attr(turn_span, "session.id").and_then(|value| value["stringValue"].as_str()),
        Some(session.session_id.to_string().as_str())
    );
    assert_eq!(
        span_attr(llm_span, "openinference.span.kind")
            .and_then(|value| value["stringValue"].as_str()),
        Some("LLM")
    );
    assert_eq!(
        span_attr(llm_span, "llm.provider").and_then(|value| value["stringValue"].as_str()),
        Some("openai")
    );
    assert_eq!(
        span_attr(llm_span, "llm.model_name").and_then(|value| value["stringValue"].as_str()),
        Some("o4-mini")
    );
    assert_eq!(
        span_attr(llm_span, "input.mime_type").and_then(|value| value["stringValue"].as_str()),
        Some("application/json")
    );
    assert_eq!(
        span_attr(llm_span, "llm.token_count.prompt").and_then(|value| value["intValue"].as_str()),
        Some("8")
    );
    assert_eq!(
        span_attr(llm_span, "llm.token_count.completion")
            .and_then(|value| value["intValue"].as_str()),
        Some("5")
    );
    assert_eq!(
        span_attr(llm_span, "llm.token_count.total").and_then(|value| value["intValue"].as_str()),
        Some("13")
    );
    assert_eq!(
        span_attr(llm_span, "llm.token_count.prompt_details.cache_read")
            .and_then(|value| value["intValue"].as_str()),
        Some("3")
    );
    assert_eq!(
        span_attr(llm_span, "llm.token_count.prompt_details.cache_write")
            .and_then(|value| value["intValue"].as_str()),
        Some("1")
    );
    assert_eq!(
        span_attr(llm_span, "llm.token_count.completion_details.reasoning")
            .and_then(|value| value["intValue"].as_str()),
        Some("2")
    );
    assert_eq!(
        span_attr(llm_span, "llm.cost.total").and_then(|value| value["doubleValue"].as_f64()),
        Some(0.3)
    );
    assert_eq!(
        span_attr(llm_span, "llm.input_messages.0.message.role")
            .and_then(|value| value["stringValue"].as_str()),
        Some("user")
    );
    assert_eq!(
        span_attr(llm_span, "llm.output_messages.0.message.role")
            .and_then(|value| value["stringValue"].as_str()),
        Some("assistant")
    );
    assert!(
        span_attr(llm_span, "input.value")
            .and_then(|value| value["stringValue"].as_str())
            .unwrap_or_default()
            .contains("Summarize the repo.")
    );
    assert!(
        span_attr(llm_span, "output.value")
            .and_then(|value| value["stringValue"].as_str())
            .unwrap_or_default()
            .contains("This repo contains Belltower.")
    );
    assert_eq!(
        event_attr(llm_span, "completion.chunk", "belltower.delta_text_preview")
            .and_then(|value| value["stringValue"].as_str()),
        Some("This repo contains Belltower.")
    );
}

#[test]
fn otlp_turn_root_input_stays_turn_local_when_history_exists() {
    let session = session();
    let branch_id = BranchId::new();
    let turn_one = TurnId::new();
    let turn_two = TurnId::new();
    let events = vec![
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "Summarize the repo."),
            },
        )
        .with_turn_id(turn_one),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::Assistant, "This repo contains Belltower."),
            },
        )
        .with_turn_id(turn_one),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::User, "List the workspace files."),
            },
        )
        .with_turn_id(turn_two),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnStarted {
                turn_id: turn_two,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                message_count: 3,
                settings_revision_id: default_settings_revision_id(),
                source: TurnStartSource::UserMessage,
                resumed_from_call_id: None,
            },
        )
        .with_turn_id(turn_two),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::CompletionRequested {
                llm_call_ordinal: 1,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                message_count: 3,
            },
        )
        .with_turn_id(turn_two),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(Role::Assistant, "The workspace root contains Cargo.toml."),
            },
        )
        .with_turn_id(turn_two),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::CompletionFinished {
                llm_call_ordinal: 1,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                usage: bt_core::TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 4,
                    total_tokens: 14,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: Some(bt_core::CostBreakdown {
                    prompt_usd: 0.01,
                    completion_usd: 0.02,
                    total_usd: 0.03,
                    cache_read_usd: None,
                    cache_write_usd: None,
                    reasoning_usd: None,
                }),
                finish_reason: "stop".to_owned(),
                latency_ms: 9,
            },
        )
        .with_turn_id(turn_two),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnFinished {
                turn_id: turn_two,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                status: "completed".to_owned(),
                finish_reason: Some("stop".to_owned()),
                latency_ms: 9,
            },
        )
        .with_turn_id(turn_two),
    ];

    let otlp = export_session(&session, &[], &events, ExportFormat::Otlp).expect("otlp export");
    let document: Value = serde_json::from_str(&otlp.content).expect("valid otlp json");
    let turn_spans = spans_by_name(&document, "turn");
    let turn_span = turn_spans
        .iter()
        .find(|span| {
            span_attr(span, "turn.id").and_then(|value| value["stringValue"].as_str())
                == Some(turn_two.to_string().as_str())
        })
        .expect("second turn span");

    let turn_input = span_attr(turn_span, "input.value")
        .and_then(|value| value["stringValue"].as_str())
        .unwrap_or_default();
    assert!(turn_input.contains("List the workspace files."));
    assert!(!turn_input.contains("This repo contains Belltower."));
}

#[test]
fn otlp_export_sets_tool_io_and_event_previews() {
    let session = session();
    let branch_id = BranchId::new();
    let turn_id = TurnId::new();
    let call_id = ToolCallId::new("call_1");
    let tool_call_message = Message::from_part(
        Role::Assistant,
        MessagePart::ToolCall {
            call: ToolCall {
                tool_name: "read".to_owned(),
                call_id: call_id.to_string(),
                arguments: serde_json::json!({"path": "Cargo.toml"}),
            },
        },
    );
    let tool_result_message = Message::from_part(
        Role::Tool,
        MessagePart::ToolResult {
            result: ToolResultEnvelope {
                call_id: call_id.clone(),
                tool_name: "read".to_owned(),
                is_error: false,
                output: serde_json::json!({"content": "[package]"}),
                duration_ms: Some(12),
            },
        },
    );
    let events = vec![
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnStarted {
                turn_id,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                message_count: 1,
                settings_revision_id: default_settings_revision_id(),
                source: TurnStartSource::UserMessage,
                resumed_from_call_id: None,
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolCallRequested {
                call_id: call_id.clone(),
                tool_name: "read".to_owned(),
                arguments: serde_json::json!({"path": "Cargo.toml"}),
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: tool_call_message,
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: tool_result_message,
            },
        )
        .with_turn_id(turn_id),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolExecutionFinished {
                call_id: call_id.clone(),
                tool_name: "read".to_owned(),
                result: ToolResultEnvelope {
                    call_id,
                    tool_name: "read".to_owned(),
                    is_error: false,
                    output: serde_json::json!({"content": "[package]"}),
                    duration_ms: Some(12),
                },
            },
        )
        .with_turn_id(turn_id),
    ];

    let otlp = export_session(&session, &[], &events, ExportFormat::Otlp).expect("otlp export");
    let document: Value = serde_json::from_str(&otlp.content).expect("valid otlp json");
    let tool_span = span_by_name(&document, "tool_execution");

    assert_eq!(
        span_attr(tool_span, "openinference.span.kind")
            .and_then(|value| value["stringValue"].as_str()),
        Some("TOOL")
    );
    assert_eq!(
        span_attr(tool_span, "tool.name").and_then(|value| value["stringValue"].as_str()),
        Some("read")
    );
    assert_eq!(
        span_attr(tool_span, "tool.id").and_then(|value| value["stringValue"].as_str()),
        Some("call_1")
    );
    assert!(
        span_attr(tool_span, "input.value")
            .and_then(|value| value["stringValue"].as_str())
            .unwrap_or_default()
            .contains("Cargo.toml")
    );
    assert!(
        span_attr(tool_span, "output.value")
            .and_then(|value| value["stringValue"].as_str())
            .unwrap_or_default()
            .contains("[package]")
    );
    assert_eq!(
        event_attr(
            tool_span,
            "tool.execution.finished",
            "belltower.output_preview"
        )
        .and_then(|value| value["stringValue"].as_str()),
        Some("{\"content\":\"[package]\"}")
    );
}

#[test]
fn otlp_export_preserves_resumed_turn_provenance_and_tool_input() {
    let session = session();
    let branch_id = BranchId::new();
    let original_turn = TurnId::new();
    let resumed_turn = TurnId::new();
    let call_id = ToolCallId::new("call_resume");
    let events = vec![
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnStarted {
                turn_id: original_turn,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                message_count: 1,
                settings_revision_id: default_settings_revision_id(),
                source: TurnStartSource::UserMessage,
                resumed_from_call_id: None,
            },
        )
        .with_turn_id(original_turn),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolCallRequested {
                call_id: call_id.clone(),
                tool_name: "read".to_owned(),
                arguments: serde_json::json!({"path": "Cargo.toml"}),
            },
        )
        .with_turn_id(original_turn),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolApprovalRequested {
                call_id: call_id.clone(),
                tool_name: "read".to_owned(),
                snapshot: Some(approval_snapshot(
                    session.session_id,
                    call_id.clone(),
                    "read",
                )),
            },
        )
        .with_turn_id(original_turn),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnStarted {
                turn_id: resumed_turn,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                message_count: 2,
                settings_revision_id: 7,
                source: TurnStartSource::ApprovalResume,
                resumed_from_call_id: Some(call_id.clone()),
            },
        )
        .with_turn_id(resumed_turn),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolApprovalResolved {
                call_id: call_id.clone(),
                tool_name: "read".to_owned(),
                request_fingerprint: Some("read:fnv1a64:approval".to_owned()),
                resolution: None,
                decision: ApprovalDecision::Approved {
                    decided_at: time::OffsetDateTime::now_utc(),
                    decided_by: "operator".to_owned(),
                    scope: ApprovalScope::Session,
                    source: ApprovalDecisionSource::Human,
                },
            },
        )
        .with_turn_id(resumed_turn),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolExecutionFinished {
                call_id: call_id.clone(),
                tool_name: "read".to_owned(),
                result: ToolResultEnvelope {
                    call_id: call_id.clone(),
                    tool_name: "read".to_owned(),
                    is_error: false,
                    output: serde_json::json!({"content": "[package]"}),
                    duration_ms: Some(12),
                },
            },
        )
        .with_turn_id(resumed_turn),
        EventEnvelope::new(
            session.session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnFinished {
                turn_id: resumed_turn,
                provider: "openai".to_owned(),
                model: "o4-mini".to_owned(),
                status: "completed".to_owned(),
                finish_reason: Some("tool_result".to_owned()),
                latency_ms: 12,
            },
        )
        .with_turn_id(resumed_turn),
    ];

    let otlp = export_session(&session, &[], &events, ExportFormat::Otlp).expect("otlp export");
    let document: Value = serde_json::from_str(&otlp.content).expect("valid otlp json");

    let resumed_turn_span = spans_by_name(&document, "turn")
        .into_iter()
        .find(|span| {
            span_attr(span, "turn.id").and_then(|value| value["stringValue"].as_str())
                == Some(resumed_turn.to_string().as_str())
        })
        .expect("resumed turn span");
    assert_eq!(
        span_attr(resumed_turn_span, "belltower.turn.source")
            .and_then(|value| value["stringValue"].as_str()),
        Some("approval_resume")
    );
    assert_eq!(
        span_attr(resumed_turn_span, "belltower.settings_revision_id")
            .and_then(|value| value["intValue"].as_str()),
        Some("7")
    );
    assert_eq!(
        span_attr(resumed_turn_span, "belltower.resumed_from_call_id")
            .and_then(|value| value["stringValue"].as_str()),
        Some(call_id.to_string().as_str())
    );

    let resumed_tool_span = spans_by_name(&document, "tool_execution")
        .into_iter()
        .find(|span| {
            span_attr(span, "turn.id").and_then(|value| value["stringValue"].as_str())
                == Some(resumed_turn.to_string().as_str())
        })
        .expect("resumed tool span");
    assert!(
        span_attr(resumed_tool_span, "input.value")
            .and_then(|value| value["stringValue"].as_str())
            .unwrap_or_default()
            .contains("Cargo.toml")
    );
    assert!(
        span_attr(resumed_tool_span, "output.value")
            .and_then(|value| value["stringValue"].as_str())
            .unwrap_or_default()
            .contains("[package]")
    );
    assert_eq!(
        span_attr(resumed_tool_span, "belltower.tool.approval_scope")
            .and_then(|value| value["stringValue"].as_str()),
        Some("Session")
    );
    assert_eq!(
        span_attr(resumed_tool_span, "belltower.tool.approval_source")
            .and_then(|value| value["stringValue"].as_str()),
        Some("Human")
    );
    assert_eq!(
        span_attr(
            resumed_tool_span,
            "belltower.tool.approval_request_fingerprint"
        )
        .and_then(|value| value["stringValue"].as_str()),
        Some("read:fnv1a64:approval")
    );
    assert_eq!(
        span_attr(resumed_tool_span, "belltower.tool.approval_arguments_hash")
            .and_then(|value| value["stringValue"].as_str()),
        Some("fnv1a64:arguments")
    );
    assert_eq!(
        span_attr(resumed_tool_span, "belltower.tool.approval_risk_class")
            .and_then(|value| value["stringValue"].as_str()),
        Some("High")
    );
}
