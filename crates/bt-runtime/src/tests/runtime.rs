use super::support::completion_cost_breakdown;
use super::{
    BelltowerRuntime, BudgetEnforcementOutcome, PostTurnControlAction, ResumableToolCallKind,
    UserMessageAdmission,
};
use crate::{TurnAdapterFuture, TurnExecutionAdapters, TurnRunRequest};
use bt_core::{
    ApprovalDecision, ApprovalDecisionSource, ApprovalRequest, ApprovalRequirement, ApprovalScope,
    BelltowerConfig, BelltowerError, BudgetConfig, CompletionRequest, CompletionSummary,
    ConnectionDescriptor, ConnectionId, ContextCompactionPhase, ContextCompactionStatus,
    ContextCompactionTrigger, ContextManifest, ContextMessageSourceRef, EventEnvelope,
    EventPayload, FinishReason, InstructionDocument, Message, MessagePart, PlanItem, PlanStatus,
    RelatedSessionDeliveryMode, RelatedSessionMessageDirection, RelatedSessionMessageKind,
    RelatedSessionMessageStatus, Role, SessionRuntimeState, SessionToolMode, SpanKind, TokenUsage,
    ToolCall, ToolCallId, ToolDisplayGroup, ToolExecutionMode, ToolInterruptBehavior, ToolMetadata,
    ToolOperationContext, ToolOperationInitiator, ToolResultEnvelope, ToolRiskClass, TurnId,
    TurnInstructionProvenance, TurnStartSource,
};
use bt_session::SqliteSessionStore;
use bt_tools::BuiltInToolRegistry;
use serde_json::json;
use std::{sync::Arc, thread, time::Duration};
use tempfile::NamedTempFile;
use time::OffsetDateTime;

fn approval_request(
    session_id: bt_core::SessionId,
    call_id: &str,
    tool_name: &str,
    arguments: serde_json::Value,
) -> ApprovalRequest {
    ApprovalRequest {
        session_id,
        call_id: ToolCallId::new(call_id),
        tool_name: tool_name.to_owned(),
        arguments,
        requirement: ApprovalRequirement::Always,
        tool_metadata: ToolMetadata {
            risk_class: ToolRiskClass::High,
            is_read_only: false,
            is_concurrency_safe: false,
            interrupt_behavior: ToolInterruptBehavior::TerminateProcess,
            execution_mode: ToolExecutionMode::Immediate,
            should_defer: false,
            catalogue_tags: Vec::new(),
            display_group: ToolDisplayGroup::Execution,
        },
        requested_at: OffsetDateTime::now_utc(),
    }
}

fn assert_cost_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn concurrent_runtime_commits_publish_in_canonical_sequence_order() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = Arc::new(BelltowerRuntime::open(config, file.path()).expect("runtime"));
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("event-order".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .append_message(&session, &branch, Role::Assistant, "seed")
        .expect("seed canonical stream");
    let mut receiver = runtime.subscribe();

    let writer_count = 32;
    let barrier = Arc::new(std::sync::Barrier::new(writer_count));
    let handles = (0..writer_count)
        .map(|index| {
            let runtime = Arc::clone(&runtime);
            let barrier = Arc::clone(&barrier);
            let session = session.clone();
            let branch = branch.clone();
            thread::spawn(move || {
                barrier.wait();
                runtime
                    .append_message(
                        &session,
                        &branch,
                        Role::Assistant,
                        format!("concurrent-{index}"),
                    )
                    .expect("append concurrent event")
            })
        })
        .collect::<Vec<_>>();

    let mut committed = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer thread"))
        .collect::<Vec<_>>();
    committed.sort_unstable();
    let published = (0..writer_count)
        .map(|_| {
            receiver
                .blocking_recv()
                .expect("receive committed event")
                .seq_id
                .expect("published sequence id")
        })
        .collect::<Vec<_>>();
    assert_eq!(published, committed);
}

struct FailingProviderPreflightAdapters;

impl TurnExecutionAdapters for FailingProviderPreflightAdapters {
    fn build_tool_registry<'a>(
        &'a self,
        _session: &'a bt_core::SessionRecord,
        _branch_id: bt_core::BranchId,
    ) -> TurnAdapterFuture<'a, BuiltInToolRegistry> {
        Box::pin(async { Ok(BuiltInToolRegistry::default()) })
    }

    fn provider_for_connection<'a>(
        &'a self,
        _connection: &'a ConnectionDescriptor,
    ) -> TurnAdapterFuture<'a, Arc<dyn bt_core::traits::Provider>> {
        Box::pin(async { Err(BelltowerError::Auth("provider preflight failed".to_owned())) })
    }
}

#[test]
fn completion_cost_breakdown_includes_token_class_rates() {
    let cost = completion_cost_breakdown(
        "openai-compatible",
        "gpt-5.4-mini",
        &TokenUsage {
            prompt_tokens: 1_000_000,
            completion_tokens: 1_000_000,
            total_tokens: 2_000_000,
            cache_read_tokens: Some(1_000_000),
            cache_write_tokens: None,
            reasoning_tokens: Some(1_000_000),
        },
    )
    .expect("cost calculation")
    .expect("priced cost");

    assert_cost_close(cost.prompt_usd, 0.75);
    assert_cost_close(cost.completion_usd, 4.50);
    assert_cost_close(cost.cache_read_usd.expect("cache read cost"), 0.075);
    assert_eq!(cost.cache_write_usd, None);
    assert_cost_close(cost.reasoning_usd.expect("reasoning cost"), 4.50);
    assert_cost_close(cost.total_usd, 9.825);
}

#[test]
fn completion_cost_breakdown_keeps_unpriced_token_classes_explicit() {
    let cost = completion_cost_breakdown(
        "openai-compatible",
        "o4-mini",
        &TokenUsage {
            prompt_tokens: 1_000_000,
            completion_tokens: 1_000_000,
            total_tokens: 2_000_000,
            cache_read_tokens: Some(1),
            cache_write_tokens: None,
            reasoning_tokens: None,
        },
    )
    .expect("cost calculation");

    assert_eq!(cost, None);
}

#[test]
fn runtime_creates_session_and_records_controls() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    assert!(
        runtime
            .list_sessions()
            .expect("listed sessions before activity")
            .is_empty()
    );

    runtime
        .append_message(&session, &branch, Role::User, "hello")
        .expect("message append");
    runtime
        .steer_session(
            session.session_id,
            branch.branch_id,
            "keep going".to_owned(),
        )
        .expect("steer");
    runtime
        .cancel_session(session.session_id, branch.branch_id, "user request")
        .expect("cancel");

    assert!(
        runtime
            .is_cancelled(session.session_id)
            .expect("cancel state")
    );
    assert!(
        runtime
            .clear_cancel_request(
                session.session_id,
                branch.branch_id,
                "cleared for follow-up work",
            )
            .expect("clear cancel state")
    );
    assert!(
        !runtime
            .is_cancelled(session.session_id)
            .expect("cancel state cleared")
    );
    let dispatch = runtime
        .apply_pending_steers(&session, &branch)
        .expect("apply steer")
        .expect("steer dispatch");
    assert_eq!(dispatch.branch_id, branch.branch_id);
    assert_eq!(dispatch.settings_revision_id, session.settings_revision_id);
    assert_eq!(
        runtime
            .messages(session.session_id, Some(branch.branch_id))
            .expect("messages")
            .len(),
        2
    );
    let listed_sessions = runtime
        .list_sessions()
        .expect("listed sessions after activity");
    assert_eq!(listed_sessions.len(), 1);
    assert_eq!(listed_sessions[0].session_id, session.session_id);
}

#[test]
fn runtime_rejects_unconfigured_connection_before_session_creation() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let error = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("missing"),
            None,
            SessionToolMode::Extended,
            Some("bad connection".to_owned()),
            None,
        )
        .expect_err("unconfigured connection should fail");

    assert!(matches!(
        error,
        BelltowerError::Config(message)
            if message.starts_with("connection `missing` is not configured")
    ));
    assert!(
        runtime
            .store
            .lock()
            .expect("store lock")
            .list_sessions()
            .expect("stored sessions")
            .is_empty()
    );
}

#[test]
fn reopened_runtime_reuses_session_scoped_approval() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("approval-session".to_owned()),
            None,
        )
        .expect("session creation");

    let first = approval_request(
        session.session_id,
        "call-1",
        "shell",
        json!({"command":"pwd","call_id":"call-1"}),
    );
    runtime
        .record_approval_for_request(
            session.session_id,
            branch.branch_id,
            &first,
            ApprovalDecision::Approved {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "user".to_owned(),
                scope: ApprovalScope::Session,
                source: ApprovalDecisionSource::Human,
            },
            None,
        )
        .expect("record approval");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopen");
    let evaluator = reopened.approval_evaluator();
    let same_session = approval_request(
        session.session_id,
        "call-2",
        "shell",
        json!({"command":"pwd","call_id":"call-2"}),
    );
    let other_session = approval_request(
        bt_core::SessionId::new(),
        "call-3",
        "shell",
        json!({"command":"pwd","call_id":"call-3"}),
    );

    assert!(matches!(
        evaluator
            .evaluate(&same_session)
            .expect("same-session evaluate"),
        Some(ApprovalDecision::Approved {
            scope: ApprovalScope::Session,
            ..
        })
    ));
    assert!(
        evaluator
            .evaluate(&other_session)
            .expect("other-session evaluate")
            .is_none()
    );
}

#[test]
fn reopened_runtime_replays_latest_reusable_approval_from_canonical_events() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("approval-event-replay".to_owned()),
            None,
        )
        .expect("session creation");

    let approved = approval_request(
        session.session_id,
        "call-reused",
        "shell",
        json!({"command":"pwd","call_id":"call-reused"}),
    );
    runtime
        .record_approval_for_request(
            session.session_id,
            branch.branch_id,
            &approved,
            ApprovalDecision::Approved {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "user".to_owned(),
                scope: ApprovalScope::Session,
                source: ApprovalDecisionSource::Human,
            },
            None,
        )
        .expect("record approved scope");
    runtime
        .record_approval_for_request(
            session.session_id,
            branch.branch_id,
            &approved,
            ApprovalDecision::Denied {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "user".to_owned(),
                reason: Some("later denial".to_owned()),
                scope: ApprovalScope::Session,
                source: ApprovalDecisionSource::Human,
            },
            None,
        )
        .expect("record later denied scope");

    // Reusing a call ID resets its latest-request projection. Rehydration must still replay the
    // canonical approval history rather than resurrecting an older projected decision.
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            approved.call_id.clone(),
            approved.tool_name.clone(),
            approved.arguments.clone(),
            None,
        )
        .expect("reuse call id");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopen");
    let next = approval_request(
        session.session_id,
        "call-next",
        "shell",
        json!({"command":"pwd","call_id":"call-next"}),
    );
    assert!(matches!(
        reopened
            .approval_evaluator()
            .evaluate(&next)
            .expect("evaluate replayed scope"),
        Some(ApprovalDecision::Denied {
            scope: ApprovalScope::Session,
            reason: Some(ref reason),
            ..
        }) if reason == "later denial"
    ));
}

#[test]
fn reopened_runtime_reuses_global_approval_across_sessions() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("approval-global".to_owned()),
            None,
        )
        .expect("session creation");

    let first = approval_request(
        session.session_id,
        "call-1",
        "shell",
        json!({"command":"pwd","call_id":"call-1"}),
    );
    runtime
        .record_approval_for_request(
            session.session_id,
            branch.branch_id,
            &first,
            ApprovalDecision::Approved {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "user".to_owned(),
                scope: ApprovalScope::Always,
                source: ApprovalDecisionSource::Human,
            },
            None,
        )
        .expect("record approval");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopen");
    let evaluator = reopened.approval_evaluator();
    let other_session = approval_request(
        bt_core::SessionId::new(),
        "call-2",
        "shell",
        json!({"command":"pwd","call_id":"call-2"}),
    );

    assert!(matches!(
        evaluator.evaluate(&other_session).expect("evaluate"),
        Some(ApprovalDecision::Approved {
            scope: ApprovalScope::Always,
            ..
        })
    ));
}

#[test]
fn reopened_runtime_does_not_reuse_once_scoped_approval() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("approval-once".to_owned()),
            None,
        )
        .expect("session creation");

    let first = approval_request(
        session.session_id,
        "call-1",
        "shell",
        json!({"command":"pwd","call_id":"call-1"}),
    );
    runtime
        .record_approval_for_request(
            session.session_id,
            branch.branch_id,
            &first,
            ApprovalDecision::Approved {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "user".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
            None,
        )
        .expect("record approval");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopen");
    let evaluator = reopened.approval_evaluator();
    let same_session = approval_request(
        session.session_id,
        "call-2",
        "shell",
        json!({"command":"pwd","call_id":"call-2"}),
    );

    assert!(
        evaluator
            .evaluate(&same_session)
            .expect("evaluate")
            .is_none()
    );
}

#[test]
fn first_recorded_activity_emits_session_started_and_makes_session_visible() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    runtime
        .record_operator_command(
            session.session_id,
            branch.branch_id,
            "help".to_owned(),
            "/help".to_owned(),
            "ok".to_owned(),
            true,
        )
        .expect("record operator command");

    let events = runtime
        .all_events(session.session_id)
        .expect("events after first activity");
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0].payload,
        EventPayload::SessionStarted { .. }
    ));
    assert!(matches!(
        events[1].payload,
        EventPayload::OperatorCommandRecorded { .. }
    ));
    assert_eq!(
        runtime
            .list_sessions()
            .expect("visible sessions")
            .into_iter()
            .map(|session| session.session_id)
            .collect::<Vec<_>>(),
        vec![session.session_id]
    );
}

#[test]
fn operator_tool_terminal_recovery_closes_requested_operation() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("operator-terminal-recovery".to_owned()),
            None,
        )
        .expect("session creation");
    let call_id = ToolCallId::new("operator-shell-recovery");
    runtime
        .record_tool_operation_request_transition(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            json!({ "command": "printf ok" }),
            ToolOperationContext {
                initiator: ToolOperationInitiator::Human,
                ..ToolOperationContext::default()
            },
            None,
        )
        .expect("operator request");
    let result = ToolResultEnvelope {
        call_id: call_id.clone(),
        tool_name: "shell".to_owned(),
        is_error: false,
        output: json!({ "stdout": "ok" }),
        duration_ms: Some(1),
    };
    runtime.inject_next_store_append_error_for_test("operator terminal write failed");
    let persistence_error = runtime
        .record_operator_tool_terminal_transition(
            session.session_id,
            branch.branch_id,
            result.clone(),
            "shell_command".to_owned(),
            "!printf ok".to_owned(),
            "ok".to_owned(),
            true,
        )
        .expect_err("terminal persistence should fail once");
    runtime
        .recover_operator_tool_terminal_transition(
            session.session_id,
            branch.branch_id,
            result,
            "shell_command".to_owned(),
            "!printf ok".to_owned(),
            "ok".to_owned(),
            true,
            &persistence_error,
        )
        .expect("recovered terminal batch");

    let events = runtime.all_events(session.session_id).expect("events");
    let terminal_index = events
        .iter()
        .position(|event| matches!(event.payload, EventPayload::ToolExecutionFinished { .. }))
        .expect("terminal event");
    assert!(matches!(
        events[terminal_index + 1].payload,
        EventPayload::OperatorCommandRecorded { success: true, .. }
    ));
    assert!(matches!(
        events[terminal_index + 2].payload,
        EventPayload::SessionError { ref code, .. } if code == "storage_error"
    ));
    let inspection = runtime
        .inspect_tool_call(session.session_id, call_id)
        .expect("inspection")
        .expect("tool call");
    assert_eq!(inspection.execution_status.as_deref(), Some("completed"));
}

#[test]
fn reopened_runtime_terminalizes_unbound_human_tool_without_retrying_it() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("operator-restart-recovery".to_owned()),
            None,
        )
        .expect("session creation");
    let call_id = ToolCallId::new("operator-shell-interrupted");
    runtime
        .record_tool_operation_request_transition(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            json!({ "command": "touch may-have-run" }),
            ToolOperationContext {
                initiator: ToolOperationInitiator::Human,
                ..ToolOperationContext::default()
            },
            None,
        )
        .expect("operator request");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config.clone(), file.path()).expect("reopen");
    let events = reopened.all_events(session.session_id).expect("events");
    let recovered = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolExecutionFinished {
                call_id: event_call_id,
                result,
                ..
            } if *event_call_id == call_id => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].is_error);
    assert_eq!(
        recovered[0].output["error"]["code"],
        "tool_outcome_unknown_after_restart"
    );
    assert_eq!(recovered[0].output["error"]["retryable"], false);
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionError {
            code,
            retryable: false,
            ..
        } if code == "tool_outcome_unknown_after_restart"
    )));
    let inspection = reopened
        .inspect_tool_call(session.session_id, call_id.clone())
        .expect("tool inspection")
        .expect("recovered tool");
    assert_eq!(inspection.execution_status.as_deref(), Some("completed"));
    assert_eq!(
        inspection.result.as_ref().expect("result")["output"]["error"]["code"],
        "tool_outcome_unknown_after_restart"
    );
    drop(reopened);

    let reopened_again = BelltowerRuntime::open(config, file.path()).expect("second reopen");
    let terminal_count = reopened_again
        .all_events(session.session_id)
        .expect("events")
        .into_iter()
        .filter(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolExecutionFinished {
                    call_id: event_call_id,
                    ..
                } if *event_call_id == call_id
            )
        })
        .count();
    assert_eq!(terminal_count, 1, "recovery must be idempotent");
}

#[test]
fn runtime_round_trips_raw_chunks() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, _branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    let chunk_id = runtime
        .append_raw_chunk(
            session.session_id,
            _branch.branch_id,
            None,
            "openai-compatible",
            "completion",
            None,
            b"{\"delta\":\"hello\"}",
        )
        .expect("append raw chunk");
    assert_eq!(chunk_id, 1);

    let chunks = runtime
        .raw_chunks(session.session_id, 10)
        .expect("raw chunks");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].branch_id, Some(_branch.branch_id));
    assert_eq!(chunks[0].provider, "openai-compatible");
    assert_eq!(chunks[0].stream_name, "completion");
    assert_eq!(chunks[0].content, b"{\"delta\":\"hello\"}");
}

#[test]
fn session_inspection_reports_cost_summary() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");
    let turn_id = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "gpt-5.4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("start completion turn");
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai-compatible".to_owned(),
                model: "gpt-5.4-mini".to_owned(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    prompt_tokens: 1_000_000,
                    completion_tokens: 1_000_000,
                    total_tokens: 2_000_000,
                    cache_read_tokens: Some(1_000_000),
                    cache_write_tokens: None,
                    reasoning_tokens: Some(1_000_000),
                },
                cost: None,
                latency_ms: 42,
            },
            1,
            turn_id,
        )
        .expect("record completion");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "gpt-5.4-mini".to_owned(),
            "completed".to_owned(),
            Some("stop".to_owned()),
            42,
        )
        .expect("finish completion turn");

    let inspection = runtime
        .inspect_session(session.session_id)
        .expect("inspect session")
        .expect("session inspection");
    let cost = inspection.cost_summary.expect("cost summary");
    assert_eq!(cost.prompt_tokens, 1_000_000);
    assert_eq!(cost.completion_tokens, 1_000_000);
    assert_eq!(cost.total_tokens, 2_000_000);
    assert_cost_close(cost.total_cost_usd, 9.825);
    assert_eq!(cost.unpriced_completion_count, 0);
}

#[test]
fn checkpoint_session_budget_is_restart_safe_and_does_not_double_count_resumed_turns() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("budget-checkpoint".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .configure_session_budget(
            session.session_id,
            branch.branch_id,
            BudgetConfig {
                max_wall_clock_seconds: Some(60),
                max_tokens: Some(100),
                max_turns: Some(3),
                max_cost_usd: Some(5.0),
            },
        )
        .expect("configure budget");

    let turn_id = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "gpt-5.4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");
    thread::sleep(Duration::from_millis(1100));
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai-compatible".to_owned(),
                model: "gpt-5.4-mini".to_owned(),
                finish_reason: FinishReason::ToolUse,
                usage: TokenUsage {
                    prompt_tokens: 8,
                    completion_tokens: 4,
                    total_tokens: 12,
                    cache_read_tokens: Some(0),
                    cache_write_tokens: Some(0),
                    reasoning_tokens: Some(0),
                },
                cost: Some(bt_core::CostBreakdown {
                    prompt_usd: 0.01,
                    completion_usd: 0.02,
                    total_usd: 0.03,
                    cache_read_usd: Some(0.0),
                    cache_write_usd: Some(0.0),
                    reasoning_usd: Some(0.0),
                }),
                latency_ms: 1_100,
            },
            1,
            turn_id,
        )
        .expect("first completion");
    assert_eq!(
        runtime
            .checkpoint_session_budget(session.session_id, branch.branch_id, turn_id)
            .expect("first checkpoint"),
        BudgetEnforcementOutcome::WithinBudget
    );

    let first_budget = runtime
        .session_budget(session.session_id)
        .expect("budget lookup")
        .expect("budget projection");
    assert_eq!(first_budget.tokens_used, 12);
    assert_eq!(first_budget.turns_used, 1);
    assert_eq!(first_budget.cost_used_usd, Some(0.03));
    assert!(first_budget.elapsed_seconds >= 1);

    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai-compatible".to_owned(),
                model: "gpt-5.4-mini".to_owned(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                    cache_read_tokens: Some(0),
                    cache_write_tokens: Some(0),
                    reasoning_tokens: Some(0),
                },
                cost: Some(bt_core::CostBreakdown {
                    prompt_usd: 0.02,
                    completion_usd: 0.01,
                    total_usd: 0.03,
                    cache_read_usd: Some(0.0),
                    cache_write_usd: Some(0.0),
                    reasoning_usd: Some(0.0),
                }),
                latency_ms: 50,
            },
            2,
            turn_id,
        )
        .expect("resumed completion");
    assert_eq!(
        runtime
            .checkpoint_session_budget(session.session_id, branch.branch_id, turn_id)
            .expect("second checkpoint"),
        BudgetEnforcementOutcome::WithinBudget
    );
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "gpt-5.4-mini".to_owned(),
            "completed".to_owned(),
            Some("Stop".to_owned()),
            1_150,
        )
        .expect("final turn finish");
    drop(runtime);
    let runtime = BelltowerRuntime::open(config, file.path()).expect("reopened runtime");

    let final_budget = runtime
        .session_budget(session.session_id)
        .expect("budget lookup")
        .expect("budget projection");
    assert_eq!(final_budget.tokens_used, 17);
    assert_eq!(final_budget.turns_used, 1);
    assert_eq!(final_budget.cost_used_usd, Some(0.06));
    assert!(final_budget.elapsed_seconds >= first_budget.elapsed_seconds);
}

#[test]
fn cost_budget_admits_fresh_work_and_stops_on_unknown_completion_cost() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            Some("qwen3.5:latest".to_owned()),
            SessionToolMode::Extended,
            Some("cost-budget".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .configure_session_budget(
            session.session_id,
            branch.branch_id,
            BudgetConfig {
                max_cost_usd: Some(5.0),
                ..BudgetConfig::default()
            },
        )
        .expect("configure budget");
    let admitted = match runtime
        .admit_user_message(&session, &branch, Message::text(Role::User, "run"))
        .expect("admit work")
    {
        UserMessageAdmission::Started(admitted) => admitted,
        UserMessageAdmission::Queued { .. } => panic!("fresh cost budget must admit work"),
    };
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai-compatible".to_owned(),
                model: "qwen3.5:latest".to_owned(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: None,
                latency_ms: 1,
            },
            1,
            admitted.turn_id(),
        )
        .expect("unpriced completion");

    assert_eq!(
        runtime
            .record_active_turn_finished(
                session.session_id,
                branch.branch_id,
                admitted.turn_id(),
                "openai-compatible".to_owned(),
                "qwen3.5:latest".to_owned(),
                "completed".to_owned(),
                Some("Stop".to_owned()),
                1,
            )
            .expect("finish turn"),
        BudgetEnforcementOutcome::Exhausted
    );
    let budget = runtime
        .session_budget(session.session_id)
        .expect("budget lookup")
        .expect("budget projection");
    assert_eq!(budget.cost_used_usd, None);
    assert!(
        runtime
            .inspect_session(session.session_id)
            .expect("inspection")
            .expect("session")
            .cancel_requested
    );
}

#[test]
fn runtime_updates_session_connection_and_model() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, _branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    let updated = runtime
        .update_session_settings(
            session.session_id,
            Some(ConnectionId::new("openai")),
            Some(Some("o4-mini".to_owned())),
            None,
        )
        .expect("update session");
    assert_eq!(updated.connection_id, ConnectionId::new("openai"));
    assert_eq!(updated.model_id.as_deref(), Some("o4-mini"));

    let reset = runtime
        .update_session_settings(session.session_id, None, Some(None), None)
        .expect("reset model");
    assert_eq!(reset.connection_id, ConnectionId::new("openai"));
    assert_eq!(reset.model_id, None);
}

#[test]
fn runtime_rejects_unconfigured_connection_before_settings_update() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, _branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    let error = runtime
        .update_session_settings(
            session.session_id,
            Some(ConnectionId::new("missing")),
            Some(Some("ghost-model".to_owned())),
            None,
        )
        .expect_err("unconfigured connection should fail");

    assert!(matches!(
        error,
        BelltowerError::Config(message)
            if message.starts_with("connection `missing` is not configured")
    ));
    let unchanged = runtime
        .load_session(session.session_id)
        .expect("load session")
        .expect("session remains");
    assert_eq!(unchanged.connection_id, ConnectionId::new("local"));
    assert_eq!(unchanged.model_id, None);
    assert_eq!(unchanged.settings_revision_id, session.settings_revision_id);
}

#[test]
fn compact_branch_context_is_runtime_owned_and_records_event() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .append_message(&session, &branch, Role::User, "first task context")
        .expect("first message");
    runtime
        .append_message(&session, &branch, Role::Assistant, "acknowledged")
        .expect("assistant message");
    runtime
        .append_message(&session, &branch, Role::User, "latest request")
        .expect("latest message");

    let prepared = runtime
        .compact_branch_context(&session, &branch, None, Vec::new(), None)
        .expect("compact branch context");
    let compaction = prepared.compaction.expect("forced compaction");

    assert_eq!(prepared.connection.id, ConnectionId::new("openai"));
    assert_eq!(prepared.model_id, "o4-mini");
    assert!(
        compaction
            .summary
            .contains("Earlier conversation compacted")
    );
    assert_eq!(compaction.trigger, ContextCompactionTrigger::Forced);
    assert_eq!(compaction.phase, ContextCompactionPhase::Manual);
    assert_eq!(compaction.status, ContextCompactionStatus::Completed);
    assert_eq!(compaction.provider.as_deref(), Some("openai-compatible"));
    assert_eq!(compaction.model.as_deref(), Some("o4-mini"));
    assert!(compaction.context_boundary_seq_id.is_some());
    assert!(compaction.source_event_id.is_some());
    assert!(compaction.summary_message_id.is_some());
    assert!(compaction.first_kept_message_id.is_some());
    assert_eq!(compaction.first_kept_branch_id, Some(branch.branch_id));
    assert!(compaction.first_kept_seq_id.is_some());

    let events = runtime
        .all_events(session.session_id)
        .expect("events after compaction");
    let event = events
        .iter()
        .find(|event| matches!(event.payload, EventPayload::ContextCompacted { .. }))
        .expect("context compacted event");
    assert_eq!(compaction.source_event_id, Some(event.event_id));
    match &event.payload {
        EventPayload::ContextCompacted {
            compaction_id,
            trigger,
            phase,
            status,
            reason,
            provider,
            model,
            context_boundary_seq_id,
            summary_message_id,
            first_kept_message_id,
            first_kept_branch_id,
            first_kept_seq_id,
            latency_ms,
            summary,
            ..
        } => {
            assert_eq!(*compaction_id, compaction.compaction_id);
            assert_eq!(*trigger, ContextCompactionTrigger::Forced);
            assert_eq!(*phase, ContextCompactionPhase::Manual);
            assert_eq!(*status, ContextCompactionStatus::Completed);
            assert_eq!(reason.as_deref(), Some("forced"));
            assert_eq!(provider.as_deref(), Some("openai-compatible"));
            assert_eq!(model.as_deref(), Some("o4-mini"));
            assert_eq!(*context_boundary_seq_id, compaction.context_boundary_seq_id);
            assert_eq!(*summary_message_id, compaction.summary_message_id);
            assert_eq!(*first_kept_message_id, compaction.first_kept_message_id);
            assert_eq!(*first_kept_branch_id, Some(branch.branch_id));
            assert_eq!(*first_kept_seq_id, compaction.first_kept_seq_id);
            assert!(latency_ms.is_some());
            assert!(summary.contains("Earlier conversation compacted"));
        }
        _ => unreachable!("matched context compacted event"),
    }
}

#[tokio::test]
async fn turn_preflight_builds_provider_before_recording_context_compaction() {
    let mut config = BelltowerConfig::from_embedded().expect("config");
    config.context.reserve_tokens = 250_000;
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("provider-preflight".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .append_message(&session, &branch, Role::User, "first task context")
        .expect("first message");
    runtime
        .append_message(&session, &branch, Role::Assistant, "acknowledged")
        .expect("assistant message");
    runtime
        .configure_session_budget(
            session.session_id,
            branch.branch_id,
            BudgetConfig {
                max_turns: Some(1),
                ..BudgetConfig::default()
            },
        )
        .expect("configure single-turn budget");
    let admitted_turn = match runtime
        .admit_user_message(
            &session,
            &branch,
            Message::text(Role::User, "latest request"),
        )
        .expect("admit latest request")
    {
        UserMessageAdmission::Started(admitted_turn) => admitted_turn,
        UserMessageAdmission::Queued { .. } => panic!("idle session should admit the turn"),
    };

    let result = runtime
        .turn_orchestrator()
        .run_session_turns(
            &FailingProviderPreflightAdapters,
            TurnRunRequest::new(session.clone(), branch.clone(), admitted_turn)
                .expect("admitted turn request"),
        )
        .await;

    let error = result.expect_err("provider preflight should fail");
    assert!(matches!(error, BelltowerError::Auth(_)));

    let events = runtime.all_events(session.session_id).expect("events");
    assert!(
        events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::SessionError { .. })),
        "preflight failure should be durable"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::ContextCompacted { .. })),
        "context compaction must not be recorded before executable provider preflight"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event.payload,
            EventPayload::TurnContextManifestRecorded { .. }
        )),
        "context manifests describe executable LLM calls, not failed provider preflight"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::CompletionRequested { .. })),
        "completion request should not be recorded before provider preflight succeeds"
    );
    let seq_for = |predicate: fn(&EventPayload) -> bool| {
        events
            .iter()
            .find(|event| predicate(&event.payload))
            .and_then(|event| event.seq_id)
            .expect("expected durable event")
    };
    let checkpoint_seq =
        seq_for(|payload| matches!(payload, EventPayload::BudgetCheckpoint { .. }));
    let cancellation_seq =
        seq_for(|payload| matches!(payload, EventPayload::SessionCancelled { .. }));
    let finish_seq = seq_for(|payload| matches!(payload, EventPayload::TurnFinished { .. }));
    assert!(checkpoint_seq < cancellation_seq);
    assert!(cancellation_seq < finish_seq);
    assert_eq!(
        runtime
            .inspect_session(session.session_id)
            .expect("inspect session")
            .expect("session exists")
            .runtime_state,
        SessionRuntimeState::Idle
    );
}

#[test]
fn admitted_turn_cannot_be_rebound_to_another_session_or_branch() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session_a, branch_a) = runtime
        .create_session(
            "/tmp/project-a".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("capability-a".to_owned()),
            None,
        )
        .expect("session a");
    let (session_b, branch_b) = runtime
        .create_session(
            "/tmp/project-b".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("capability-b".to_owned()),
            None,
        )
        .expect("session b");
    let admitted_a = match runtime
        .admit_user_message(&session_a, &branch_a, Message::text(Role::User, "task a"))
        .expect("admit a")
    {
        UserMessageAdmission::Started(admitted) => admitted,
        UserMessageAdmission::Queued { .. } => panic!("idle session should admit task a"),
    };
    let admitted_b = match runtime
        .admit_user_message(&session_b, &branch_b, Message::text(Role::User, "task b"))
        .expect("admit b")
    {
        UserMessageAdmission::Started(admitted) => admitted,
        UserMessageAdmission::Queued { .. } => panic!("idle session should admit task b"),
    };

    let mut wrong_branch = branch_a.clone();
    wrong_branch.branch_id = bt_core::BranchId::new();
    assert!(matches!(
        TurnRunRequest::new(session_a.clone(), wrong_branch, admitted_a),
        Err(BelltowerError::InvalidState(_))
    ));
    assert!(matches!(
        TurnRunRequest::new(session_a, branch_a, admitted_b),
        Err(BelltowerError::InvalidState(_))
    ));
}

#[test]
fn session_queue_inspection_reports_canonical_pending_controls() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");
    let paused_turn_id = TurnId::new();

    runtime
        .queue_message(
            session.session_id,
            branch.branch_id,
            Message::text(Role::User, "queued follow-up"),
        )
        .expect("queue message");
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-shell"),
            "shell".to_owned(),
            json!({"command": "pwd"}),
            Some(paused_turn_id),
        )
        .expect("shell tool call requested");
    runtime
        .record_approval_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-shell"),
            "shell".to_owned(),
            Some(paused_turn_id),
        )
        .expect("approval requested");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-ask"),
            "ask".to_owned(),
            json!({"question": "Continue?", "choices": ["yes", "no"]}),
            Some(paused_turn_id),
        )
        .expect("ask tool call requested");

    let inspection = runtime
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");

    assert_eq!(inspection.queued_messages.len(), 1);
    assert_eq!(inspection.pending_approvals.len(), 1);
    assert_eq!(inspection.pending_inputs.len(), 1);
    assert_eq!(inspection.pending_approvals[0].tool_name, "shell");
    assert_eq!(inspection.pending_approvals[0].branch_id, branch.branch_id);
    assert_eq!(inspection.pending_approvals[0].turn_id, paused_turn_id);
    assert_eq!(
        inspection.pending_approvals[0].settings_revision_id,
        session.settings_revision_id
    );
    assert_eq!(inspection.pending_inputs[0].branch_id, branch.branch_id);
    assert_eq!(inspection.pending_inputs[0].turn_id, paused_turn_id);
    assert_eq!(
        inspection.pending_inputs[0].settings_revision_id,
        session.settings_revision_id
    );
    assert!(matches!(
        inspection.queued_messages[0].message.parts.as_slice(),
        [bt_core::MessagePart::Text { text }] if text == "queued follow-up"
    ));
}

#[test]
fn resumable_approval_call_uses_original_branch() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");
    let child_branch = runtime
        .create_branch(session.session_id, branch.branch_id, None, false, None)
        .expect("child branch");
    let paused_turn_id = TurnId::new();

    runtime
        .record_turn_started(
            session.session_id,
            child_branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn");
    runtime
        .record_tool_call_requested(
            session.session_id,
            child_branch.branch_id,
            ToolCallId::new("call-shell"),
            "shell".to_owned(),
            json!({"command": "pwd"}),
            Some(paused_turn_id),
        )
        .expect("tool call requested");
    runtime
        .record_approval_requested(
            session.session_id,
            child_branch.branch_id,
            ToolCallId::new("call-shell"),
            "shell".to_owned(),
            Some(paused_turn_id),
        )
        .expect("approval requested");

    let resumable = runtime
        .resumable_approval_call(session.session_id, ToolCallId::new("call-shell"), "shell")
        .expect("approval lookup")
        .expect("resumable approval");

    assert_eq!(resumable.session.session_id, session.session_id);
    assert_eq!(resumable.branch.branch_id, child_branch.branch_id);
    assert_eq!(resumable.turn_id, paused_turn_id);
    assert_eq!(resumable.settings_revision_id, session.settings_revision_id);
    assert_eq!(resumable.tool_call.call_id, "call-shell");
    assert_eq!(resumable.tool_call.arguments["command"], json!("pwd"));
}

#[test]
fn resumable_input_call_uses_original_branch() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");
    let child_branch = runtime
        .create_branch(session.session_id, branch.branch_id, None, false, None)
        .expect("child branch");
    let paused_turn_id = TurnId::new();

    runtime
        .record_turn_started(
            session.session_id,
            child_branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn");
    runtime
        .record_tool_call_requested(
            session.session_id,
            child_branch.branch_id,
            ToolCallId::new("call-ask"),
            "ask".to_owned(),
            json!({"question": "Continue?", "choices": ["yes", "no"]}),
            Some(paused_turn_id),
        )
        .expect("tool call requested");

    let resumable = runtime
        .resumable_input_call(session.session_id, ToolCallId::new("call-ask"))
        .expect("input lookup")
        .expect("resumable input");

    assert_eq!(resumable.session.session_id, session.session_id);
    assert_eq!(resumable.branch.branch_id, child_branch.branch_id);
    assert_eq!(resumable.turn_id, paused_turn_id);
    assert_eq!(resumable.settings_revision_id, session.settings_revision_id);
    assert_eq!(resumable.tool_call.call_id, "call-ask");
    assert_eq!(
        resumable.tool_call.arguments["question"],
        json!("Continue?")
    );
}

#[test]
fn resumable_input_call_requires_turn_provenance() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-ask"),
            "ask".to_owned(),
            json!({"question": "Continue?"}),
            None,
        )
        .expect("tool call requested");

    let error = runtime
        .resumable_input_call(session.session_id, ToolCallId::new("call-ask"))
        .expect_err("missing turn provenance should fail");
    assert!(
        error
            .to_string()
            .contains("missing canonical turn metadata")
    );
}

#[test]
fn completed_tool_calls_are_not_resumable() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");
    let paused_turn_id = TurnId::new();

    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-ask"),
            "ask".to_owned(),
            json!({"question": "Continue?"}),
            Some(paused_turn_id),
        )
        .expect("tool call requested");
    runtime
        .record_tool_execution(
            session.session_id,
            branch.branch_id,
            ToolResultEnvelope {
                call_id: ToolCallId::new("call-ask"),
                tool_name: "ask".to_owned(),
                is_error: false,
                output: json!({"response": "yes"}),
                duration_ms: None,
            },
            Some(paused_turn_id),
        )
        .expect("tool execution");

    assert!(
        runtime
            .resumable_input_call(session.session_id, ToolCallId::new("call-ask"))
            .expect("input lookup after completion")
            .is_none()
    );
}

#[test]
fn clear_queued_messages_drains_session_control_queue() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    runtime
        .queue_message(
            session.session_id,
            branch.branch_id,
            Message::text(Role::User, "queued follow-up"),
        )
        .expect("queue first");
    runtime
        .queue_message(
            session.session_id,
            branch.branch_id,
            Message::text(Role::User, "queued second"),
        )
        .expect("queue second");

    let cleared = runtime
        .clear_queued_messages(
            session.session_id,
            "Dropped by explicit queue clear.",
            "Dropped by explicit queue clear.",
            true,
        )
        .expect("clear queue");
    assert_eq!(cleared.len(), 2);

    let inspection = runtime
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");
    assert!(inspection.queued_messages.is_empty());
}

#[test]
fn queued_follow_up_keeps_enqueued_settings_revision_after_later_change() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("revision-queue".to_owned()),
            None,
        )
        .expect("session creation");

    runtime
        .queue_message(
            session.session_id,
            branch.branch_id,
            Message::text(Role::User, "queued follow-up"),
        )
        .expect("queue message");

    let updated = runtime
        .update_session_settings(
            session.session_id,
            Some(ConnectionId::new("local")),
            None,
            None,
        )
        .expect("update session");
    assert_eq!(
        updated.settings_revision_id,
        session.settings_revision_id + 1
    );

    let dispatch = runtime
        .dispatch_next_queued_message(&updated)
        .expect("dispatch queued")
        .expect("queued dispatch");
    assert_eq!(dispatch.settings_revision_id, session.settings_revision_id);

    let prepared = runtime
        .prepare_turn_context(
            &updated,
            &branch,
            None,
            Vec::new(),
            None,
            None,
            dispatch.settings_revision_id,
        )
        .expect("prepare queued turn");
    assert_eq!(prepared.settings_revision_id, session.settings_revision_id);
    assert_eq!(prepared.connection.id, ConnectionId::new("openai"));
    assert_eq!(prepared.model_id, "o4-mini");
}

#[test]
fn reopened_runtime_preserves_pending_control_state() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    runtime
        .queue_message(
            session.session_id,
            branch.branch_id,
            Message::text(Role::User, "queued follow-up"),
        )
        .expect("queue message");
    runtime
        .steer_session(
            session.session_id,
            branch.branch_id,
            "keep going".to_owned(),
        )
        .expect("steer session");
    runtime
        .cancel_session(session.session_id, branch.branch_id, "user request")
        .expect("cancel session");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config.clone(), file.path()).expect("reopened runtime");
    let inspection = reopened
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");
    assert!(inspection.cancel_requested);
    assert_eq!(inspection.pending_steer_count, 1);
    assert_eq!(inspection.queued_messages.len(), 1);
}

#[test]
fn reopened_runtime_dispatches_queued_message_once() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    runtime
        .queue_message(
            session.session_id,
            branch.branch_id,
            Message::text(Role::User, "queued follow-up"),
        )
        .expect("queue message");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config.clone(), file.path()).expect("reopened");
    let dispatched = reopened
        .dispatch_next_queued_message(&session)
        .expect("dispatch queued")
        .expect("queued branch");
    assert_eq!(dispatched.branch_id, branch.branch_id);
    drop(reopened);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopened again");
    assert!(
        reopened
            .dispatch_next_queued_message(&session)
            .expect("dispatch after replay")
            .is_none()
    );
    let queue = reopened
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");
    assert!(queue.queued_messages.is_empty());
    let messages = reopened
        .messages(session.session_id, Some(branch.branch_id))
        .expect("messages");
    assert_eq!(messages.len(), 1);
}

#[test]
fn resumed_approval_turn_keeps_paused_settings_revision_after_later_change() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("revision-approval".to_owned()),
            None,
        )
        .expect("session creation");
    let paused_turn_id = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-shell"),
            "shell".to_owned(),
            json!({ "command": "printf approved" }),
            Some(paused_turn_id),
        )
        .expect("tool call requested");
    runtime
        .record_approval_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-shell"),
            "shell".to_owned(),
            Some(paused_turn_id),
        )
        .expect("approval requested");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "awaiting_approval".to_owned(),
            Some("ToolCalls".to_owned()),
            1,
        )
        .expect("paused turn finished");

    let updated = runtime
        .update_session_settings(
            session.session_id,
            Some(ConnectionId::new("local")),
            None,
            None,
        )
        .expect("update session");
    assert_eq!(
        updated.settings_revision_id,
        session.settings_revision_id + 1
    );

    let resumable = runtime
        .resumable_approval_call(session.session_id, ToolCallId::new("call-shell"), "shell")
        .expect("resumable approval lookup")
        .expect("resumable approval");
    let resumed = runtime
        .bootstrap_resumed_approval_turn(
            &resumable,
            &approval_request(
                session.session_id,
                "call-shell",
                "shell",
                json!({ "command": "printf approved" }),
            ),
            ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
        )
        .expect("bootstrap resumed approval");
    assert_eq!(resumed.settings_revision_id, session.settings_revision_id);
    assert!(
        runtime
            .pending_approvals(session.session_id)
            .expect("pending approvals after bootstrap")
            .is_empty()
    );

    let events = runtime.all_events(session.session_id).expect("events");
    assert!(!events.iter().any(|event| {
        matches!(
            &event.payload,
            EventPayload::ToolExecutionFinished { call_id, .. }
                if *call_id == ToolCallId::new("call-shell")
        )
    }));
    let resumed_start = events
        .into_iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnStarted {
                    turn_id,
                    source: TurnStartSource::ApprovalResume,
                    settings_revision_id,
                    ..
                } if *turn_id == resumed.turn_id
                    && *settings_revision_id == session.settings_revision_id
            )
        })
        .expect("resumed turn started");

    match resumed_start.payload {
        EventPayload::TurnStarted {
            provider,
            model,
            settings_revision_id,
            ..
        } => {
            assert_eq!(provider, "openai-compatible");
            assert_eq!(model, "o4-mini");
            assert_eq!(settings_revision_id, session.settings_revision_id);
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[test]
fn exhausted_budget_blocks_resumed_approval_without_resolving_pending_work() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("budgeted-approval-resume".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .configure_session_budget(
            session.session_id,
            branch.branch_id,
            BudgetConfig {
                max_turns: Some(1),
                ..BudgetConfig::default()
            },
        )
        .expect("configure budget");

    let paused_turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-budgeted-shell");
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            json!({ "command": "printf approved" }),
            Some(paused_turn_id),
        )
        .expect("tool call requested");
    runtime
        .record_approval_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            Some(paused_turn_id),
        )
        .expect("approval requested");
    assert_eq!(
        runtime
            .record_active_turn_finished(
                session.session_id,
                branch.branch_id,
                paused_turn_id,
                "openai-compatible".to_owned(),
                "o4-mini".to_owned(),
                "awaiting_approval".to_owned(),
                Some("ToolCalls".to_owned()),
                1,
            )
            .expect("paused turn finished"),
        BudgetEnforcementOutcome::Exhausted
    );

    let resumable = runtime
        .resumable_approval_call(session.session_id, call_id.clone(), "shell")
        .expect("resumable approval lookup")
        .expect("resumable approval");
    let event_count = runtime
        .all_events(session.session_id)
        .expect("events before rejected resume")
        .len();
    let error = runtime
        .bootstrap_resumed_approval_turn(
            &resumable,
            &approval_request(
                session.session_id,
                "call-budgeted-shell",
                "shell",
                json!({ "command": "printf approved" }),
            ),
            ApprovalDecision::Approved {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
        )
        .expect_err("exhausted budget must reject approval resume");
    assert!(error.to_string().contains("session budget exhausted"));

    let queue = runtime
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");
    assert_eq!(queue.pending_approvals.len(), 1);
    assert!(queue.cancel_requested);
    assert_eq!(
        runtime
            .all_events(session.session_id)
            .expect("events after rejected resume")
            .len(),
        event_count
    );
}

#[test]
fn pending_cancellation_blocks_resumed_input_without_clearing_control_state() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("cancelled-input-resume".to_owned()),
            None,
        )
        .expect("session creation");

    let paused_turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-cancelled-ask");
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "ask".to_owned(),
            json!({ "question": "Continue?" }),
            Some(paused_turn_id),
        )
        .expect("ask requested");
    assert_eq!(
        runtime
            .record_active_turn_finished(
                session.session_id,
                branch.branch_id,
                paused_turn_id,
                "openai-compatible".to_owned(),
                "o4-mini".to_owned(),
                "awaiting_input".to_owned(),
                Some("ToolCalls".to_owned()),
                1,
            )
            .expect("paused turn finished"),
        BudgetEnforcementOutcome::NotConfigured
    );
    runtime
        .cancel_session(
            session.session_id,
            branch.branch_id,
            "operator cancelled before answering",
        )
        .expect("cancel session");

    let resumable = runtime
        .resumable_input_call(session.session_id, call_id.clone())
        .expect("resumable input lookup")
        .expect("resumable input");
    let event_count = runtime
        .all_events(session.session_id)
        .expect("events before rejected resume")
        .len();
    let error = runtime
        .bootstrap_resumed_input_turn(
            &resumable,
            ToolResultEnvelope {
                call_id,
                tool_name: "ask".to_owned(),
                is_error: false,
                output: json!({ "response": "yes" }),
                duration_ms: Some(1),
            },
        )
        .expect_err("pending cancellation must reject input resume");
    assert!(
        error
            .to_string()
            .contains("session cancellation is pending")
    );

    let queue = runtime
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");
    assert_eq!(queue.pending_inputs.len(), 1);
    assert!(queue.cancel_requested);
    assert_eq!(
        runtime
            .all_events(session.session_id)
            .expect("events after rejected resume")
            .len(),
        event_count
    );
}

#[test]
fn reopened_runtime_closes_interrupted_resumed_approval_turns() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("resume-recovery-approval".to_owned()),
            None,
        )
        .expect("session creation");
    let paused_turn_id = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-shell"),
            "shell".to_owned(),
            json!({ "command": "printf approved" }),
            Some(paused_turn_id),
        )
        .expect("tool call requested");
    runtime
        .record_approval_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-shell"),
            "shell".to_owned(),
            Some(paused_turn_id),
        )
        .expect("approval requested");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "awaiting_approval".to_owned(),
            Some("ToolCalls".to_owned()),
            1,
        )
        .expect("paused turn finished");
    let resumable = runtime
        .resumable_approval_call(session.session_id, ToolCallId::new("call-shell"), "shell")
        .expect("resumable approval lookup")
        .expect("resumable approval");
    runtime
        .bootstrap_resumed_approval_turn(
            &resumable,
            &approval_request(
                session.session_id,
                "call-shell",
                "shell",
                json!({ "command": "printf approved" }),
            ),
            ApprovalDecision::Approved {
                decided_at: time::OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
        )
        .expect("bootstrap resumed approval");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config.clone(), file.path()).expect("reopened runtime");
    let queue = reopened
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");
    assert_eq!(queue.runtime_state, SessionRuntimeState::Idle);
    assert!(queue.pending_approvals.is_empty());

    let execution = reopened
        .session_execution(session.session_id)
        .expect("execution inspection")
        .expect("execution exists");
    let resumed = execution
        .turns
        .into_iter()
        .find(|turn| matches!(turn.source, Some(TurnStartSource::ApprovalResume)))
        .expect("resumed approval turn");
    assert_eq!(resumed.status.as_deref(), Some("failed"));
    assert_eq!(
        resumed.finish_reason.as_deref(),
        Some("interrupted_after_resume")
    );
    assert!(resumed.finished_at.is_some());

    let events = reopened.all_events(session.session_id).expect("events");
    let recovered_results = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolExecutionFinished {
                call_id, result, ..
            } if *call_id == ToolCallId::new("call-shell") => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(recovered_results.len(), 1);
    assert!(recovered_results[0].is_error);
    assert_eq!(
        recovered_results[0].output["error"]["code"],
        "tool_outcome_unknown_after_restart"
    );
    drop(reopened);

    let reopened_again = BelltowerRuntime::open(config, file.path()).expect("second reopen");
    let terminal_count = reopened_again
        .all_events(session.session_id)
        .expect("events")
        .into_iter()
        .filter(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolExecutionFinished { call_id, .. }
                    if *call_id == ToolCallId::new("call-shell")
            )
        })
        .count();
    assert_eq!(terminal_count, 1);
}

#[test]
fn reopened_runtime_closes_interrupted_resumed_input_turns() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("resume-recovery-input".to_owned()),
            None,
        )
        .expect("session creation");
    let paused_turn_id = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("paused turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-ask"),
            "ask".to_owned(),
            json!({ "question": "Continue?" }),
            Some(paused_turn_id),
        )
        .expect("ask requested");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            paused_turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "awaiting_input".to_owned(),
            Some("ToolCalls".to_owned()),
            1,
        )
        .expect("paused turn finished");
    let resumable = runtime
        .resumable_input_call(session.session_id, ToolCallId::new("call-ask"))
        .expect("resumable input lookup")
        .expect("resumable input");
    runtime
        .bootstrap_resumed_input_turn(
            &resumable,
            ToolResultEnvelope {
                call_id: ToolCallId::new("call-ask"),
                tool_name: "ask".to_owned(),
                is_error: false,
                output: json!({ "response": "yes" }),
                duration_ms: Some(1),
            },
        )
        .expect("bootstrap resumed input");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config.clone(), file.path()).expect("reopened runtime");
    let queue = reopened
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");
    assert_eq!(queue.runtime_state, SessionRuntimeState::Idle);
    assert!(queue.pending_inputs.is_empty());

    let execution = reopened
        .session_execution(session.session_id)
        .expect("execution inspection")
        .expect("execution exists");
    let resumed = execution
        .turns
        .into_iter()
        .find(|turn| matches!(turn.source, Some(TurnStartSource::InputResume)))
        .expect("resumed input turn");
    assert_eq!(resumed.status.as_deref(), Some("failed"));
    assert_eq!(
        resumed.finish_reason.as_deref(),
        Some("interrupted_after_resume")
    );
    assert!(resumed.finished_at.is_some());
    drop(reopened);

    let reopened_again = BelltowerRuntime::open(config, file.path()).expect("second reopen");
    let terminal_count = reopened_again
        .all_events(session.session_id)
        .expect("events")
        .into_iter()
        .filter(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolExecutionFinished { call_id, .. }
                    if *call_id == ToolCallId::new("call-ask")
            )
        })
        .count();
    assert_eq!(terminal_count, 1);
}

#[test]
fn reopened_runtime_closes_interrupted_user_message_turns() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("active-turn-recovery".to_owned()),
            None,
        )
        .expect("session creation");
    let turn_id = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-list"),
            "list".to_owned(),
            json!({ "path": "." }),
            Some(turn_id),
        )
        .expect("tool call requested");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopened runtime");
    let execution = reopened
        .session_execution(session.session_id)
        .expect("execution inspection")
        .expect("execution exists");
    let recovered = execution
        .turns
        .into_iter()
        .find(|turn| turn.turn_id == turn_id)
        .expect("recovered turn");
    assert_eq!(recovered.status.as_deref(), Some("failed"));
    assert_eq!(
        recovered.finish_reason.as_deref(),
        Some("interrupted_after_restart")
    );
    assert!(recovered.finished_at.is_some());
    let result = reopened
        .all_events(session.session_id)
        .expect("events")
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ToolExecutionFinished {
                call_id, result, ..
            } if call_id == ToolCallId::new("call-list") => Some(result),
            _ => None,
        })
        .expect("recovered tool result");
    assert_eq!(
        result.output["error"]["code"],
        "tool_outcome_unknown_after_restart"
    );
}

#[test]
fn stale_runtime_cannot_append_tool_lifecycle_after_recovery() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let stale = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");
    let (session, branch) = stale
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("stale-tool-writer".to_owned()),
            None,
        )
        .expect("session creation");
    let turn_id = TurnId::new();
    stale
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");

    let recovered = BelltowerRuntime::open(config, file.path()).expect("recovered runtime");
    let event_count = recovered
        .all_events(session.session_id)
        .expect("events after recovery")
        .len();
    let raw_chunk_count = recovered
        .raw_chunks(session.session_id, 100)
        .expect("raw chunks after recovery")
        .len();
    let call_id = ToolCallId::new("call-after-recovery");
    let arguments = json!({ "path": "README.md" });
    let request_error = stale
        .record_tool_request_transition(
            session.session_id,
            branch.branch_id,
            turn_id,
            call_id.clone(),
            "read".to_owned(),
            arguments.clone(),
            ToolOperationContext::default(),
            Message::from_part(
                Role::Assistant,
                MessagePart::ToolCall {
                    call: ToolCall {
                        tool_name: "read".to_owned(),
                        call_id: call_id.to_string(),
                        arguments,
                    },
                },
            ),
        )
        .expect_err("recovered turn rejects a stale tool request");
    assert!(
        request_error
            .to_string()
            .contains("does not own the active turn")
    );

    let result = ToolResultEnvelope {
        call_id,
        tool_name: "read".to_owned(),
        is_error: false,
        output: json!({ "content": "stale" }),
        duration_ms: Some(1),
    };
    let terminal_error = stale
        .record_tool_terminal_transition(
            session.session_id,
            branch.branch_id,
            turn_id,
            result.clone(),
            Message::from_part(Role::Tool, MessagePart::ToolResult { result }),
        )
        .expect_err("recovered turn rejects a stale tool result");
    assert!(
        terminal_error
            .to_string()
            .contains("does not own the active turn")
    );

    let approval_error = stale
        .record_approval(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("approval-after-recovery"),
            "shell".to_owned(),
            ApprovalDecision::Denied {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "stale-runtime".to_owned(),
                reason: Some("stale".to_owned()),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Runtime,
            },
            Some(turn_id),
        )
        .expect_err("recovered turn rejects a legacy approval write");
    assert!(
        approval_error
            .to_string()
            .contains("does not own the active turn")
    );

    let legacy_request_error = stale
        .record_tool_call_requested_with_context(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("legacy-request-after-recovery"),
            "read".to_owned(),
            json!({ "path": "README.md" }),
            ToolOperationContext::default(),
            Some(turn_id),
        )
        .expect_err("recovered turn rejects a legacy tool request write");
    assert!(
        legacy_request_error
            .to_string()
            .contains("does not own the active turn")
    );

    let legacy_execution_error = stale
        .record_tool_execution(
            session.session_id,
            branch.branch_id,
            ToolResultEnvelope {
                call_id: ToolCallId::new("legacy-result-after-recovery"),
                tool_name: "read".to_owned(),
                is_error: false,
                output: json!({ "content": "stale" }),
                duration_ms: Some(1),
            },
            Some(turn_id),
        )
        .expect_err("recovered turn rejects a legacy tool result write");
    assert!(
        legacy_execution_error
            .to_string()
            .contains("does not own the active turn")
    );

    let completion_request_error = stale
        .record_completion_requested(
            session.session_id,
            branch.branch_id,
            1,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            turn_id,
        )
        .expect_err("recovered turn rejects a completion request write");
    assert!(
        completion_request_error
            .to_string()
            .contains("does not own the active turn")
    );

    let completion_chunk_error = stale
        .record_completion_chunk(
            session.session_id,
            branch.branch_id,
            Some(1),
            Vec::new(),
            None,
            Some(turn_id),
        )
        .expect_err("recovered turn rejects a completion chunk write");
    assert!(
        completion_chunk_error
            .to_string()
            .contains("does not own the active turn")
    );

    let raw_chunk_error = stale
        .record_raw_chunk(
            session.session_id,
            branch.branch_id,
            "openai-compatible".to_owned(),
            "completion".to_owned(),
            Some(1),
            b"stale raw bytes",
            Some(turn_id),
        )
        .expect_err("recovered turn rejects a raw chunk write");
    assert!(
        raw_chunk_error
            .to_string()
            .contains("does not own the active turn")
    );

    let unpaired_raw_chunk_error = stale
        .append_raw_chunk(
            session.session_id,
            branch.branch_id,
            Some(turn_id),
            "openai-compatible",
            "completion",
            Some(1),
            b"stale unpaired raw bytes",
        )
        .expect_err("recovered turn rejects an unpaired raw chunk write");
    assert!(
        unpaired_raw_chunk_error
            .to_string()
            .contains("does not own the active turn")
    );

    let provenance_error = stale
        .record_turn_instruction_provenance(
            session.session_id,
            branch.branch_id,
            TurnInstructionProvenance {
                turn_id,
                provider: "openai-compatible".to_owned(),
                model: "o4-mini".to_owned(),
                settings_revision_id: session.settings_revision_id,
                core_prompt: InstructionDocument {
                    source: "embedded".to_owned(),
                    title: "core".to_owned(),
                    body: "stale instructions".to_owned(),
                },
                provider_overlay: None,
                instructions: Vec::new(),
                rendered_system_prompt: "stale instructions".to_owned(),
            },
        )
        .expect_err("recovered turn rejects stale instruction provenance");
    assert!(
        provenance_error
            .to_string()
            .contains("does not own the active turn")
    );
    assert_eq!(
        recovered
            .all_events(session.session_id)
            .expect("events after stale writes")
            .len(),
        event_count
    );
    assert_eq!(
        recovered
            .raw_chunks(session.session_id, 100)
            .expect("raw chunks after stale writes")
            .len(),
        raw_chunk_count
    );
}

#[test]
fn reopened_runtime_recovers_denied_tool_with_deterministic_result() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("denied-tool-recovery".to_owned()),
            None,
        )
        .expect("session creation");
    let turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-denied");
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            json!({ "command": "false" }),
            Some(turn_id),
        )
        .expect("tool request");
    runtime
        .record_approval_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            Some(turn_id),
        )
        .expect("approval request");
    runtime
        .record_approval(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            ApprovalDecision::Denied {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                reason: Some("not permitted".to_owned()),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
            Some(turn_id),
        )
        .expect("approval denied");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopened runtime");
    let result = reopened
        .all_events(session.session_id)
        .expect("events")
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ToolExecutionFinished {
                call_id: event_call_id,
                result,
                ..
            } if event_call_id == call_id => Some(result),
            _ => None,
        })
        .expect("recovered denied result");
    assert!(result.is_error);
    assert_eq!(
        result.output["error"],
        "invalid state: tool `shell` was denied"
    );
    assert!(result.output["error"]["code"].is_null());
}

#[test]
fn reused_call_id_inspection_and_resume_target_latest_pending_request() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("latest-request-inspection".to_owned()),
            None,
        )
        .expect("session creation");
    let call_id = ToolCallId::new("call-reused");

    let first_turn = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            first_turn,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("first turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            json!({"command":"pwd"}),
            Some(first_turn),
        )
        .expect("first request");
    runtime
        .record_approval_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            Some(first_turn),
        )
        .expect("first approval request");
    runtime
        .record_approval(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            ApprovalDecision::Approved {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
            Some(first_turn),
        )
        .expect("first approval resolution");
    let first_result = ToolResultEnvelope {
        call_id: call_id.clone(),
        tool_name: "shell".to_owned(),
        is_error: false,
        output: json!({"stdout":"first"}),
        duration_ms: Some(1),
    };
    runtime
        .record_tool_terminal_transition(
            session.session_id,
            branch.branch_id,
            first_turn,
            first_result.clone(),
            Message::from_part(
                Role::Tool,
                MessagePart::ToolResult {
                    result: first_result,
                },
            ),
        )
        .expect("first terminal");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            first_turn,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "completed".to_owned(),
            Some("ToolCalls".to_owned()),
            1,
        )
        .expect("first turn finished");

    let second_turn = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            second_turn,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            2,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("second turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            json!({"command":"ls"}),
            Some(second_turn),
        )
        .expect("second request");
    let pre_approval = runtime
        .inspect_tool_call(session.session_id, call_id.clone())
        .expect("inspect new request")
        .expect("new request inspection");
    assert!(pre_approval.approval_status.is_none());
    assert!(pre_approval.approval_request_snapshot.is_none());
    assert!(pre_approval.approval_resolution.is_none());
    runtime
        .record_approval_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            Some(second_turn),
        )
        .expect("second approval request");

    let inspection = runtime
        .inspect_tool_call(session.session_id, call_id.clone())
        .expect("inspect call")
        .expect("call inspection");
    assert_eq!(inspection.turn_id, Some(second_turn));
    assert_eq!(inspection.execution_status.as_deref(), Some("requested"));
    assert_eq!(inspection.approval_status.as_deref(), Some("pending"));
    assert_eq!(inspection.arguments, Some(json!({"command":"ls"})));
    assert!(inspection.completed_seq_id.is_none());
    assert!(inspection.completed_at.is_none());
    assert!(inspection.result.is_none());
    assert!(
        runtime
            .resumable_tool_call(
                session.session_id,
                call_id,
                "shell",
                ResumableToolCallKind::Approval,
            )
            .expect("resumable call")
            .is_some()
    );
}

#[test]
fn same_turn_reused_call_id_resets_turn_and_trace_summaries() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            None,
            SessionToolMode::Extended,
            Some("same-turn-call-reuse".to_owned()),
            None,
        )
        .expect("session creation");
    let turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-reused-in-turn");
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            json!({"command":"pwd"}),
            Some(turn_id),
        )
        .expect("first request");
    runtime
        .record_approval_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            Some(turn_id),
        )
        .expect("first approval request");
    runtime
        .record_approval(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            ApprovalDecision::Approved {
                decided_at: OffsetDateTime::now_utc(),
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: ApprovalDecisionSource::Human,
            },
            Some(turn_id),
        )
        .expect("first approval");
    runtime
        .record_tool_execution(
            session.session_id,
            branch.branch_id,
            ToolResultEnvelope {
                call_id: call_id.clone(),
                tool_name: "shell".to_owned(),
                is_error: false,
                output: json!({"stdout":"old"}),
                duration_ms: Some(1),
            },
            Some(turn_id),
        )
        .expect("first result");

    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "read".to_owned(),
            json!({"path":"README.md"}),
            Some(turn_id),
        )
        .expect("second request");

    let turn = runtime
        .turn_history(session.session_id)
        .expect("turn history")
        .into_iter()
        .find(|turn| turn.turn_id == turn_id)
        .expect("turn summary");
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].call_id, call_id);
    assert_eq!(turn.tool_calls[0].tool_name, "read");
    assert!(turn.tool_calls[0].approval_status.is_none());
    assert!(turn.tool_calls[0].execution_status.is_none());

    let trace_turn = runtime
        .session_execution(session.session_id)
        .expect("session execution")
        .expect("execution exists")
        .turns
        .into_iter()
        .find(|turn| turn.turn_id == turn_id)
        .expect("trace turn");
    assert_eq!(trace_turn.tool_calls.len(), 1);
    assert_eq!(trace_turn.tool_calls[0].tool_name, "read");
    assert!(trace_turn.tool_calls[0].approval_status.is_none());
    assert!(trace_turn.tool_calls[0].execution_status.is_none());
}

#[test]
fn reopened_runtime_scopes_recovery_to_reused_call_request() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("reused-call-id-recovery".to_owned()),
            None,
        )
        .expect("session creation");
    let completed_turn = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            completed_turn,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("completed turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-reused"),
            "list".to_owned(),
            json!({ "path": "." }),
            Some(completed_turn),
        )
        .expect("historical tool request");
    let historical_result = ToolResultEnvelope {
        call_id: ToolCallId::new("call-reused"),
        tool_name: "list".to_owned(),
        is_error: false,
        output: json!({ "entries": [] }),
        duration_ms: Some(1),
    };
    runtime
        .record_tool_terminal_transition(
            session.session_id,
            branch.branch_id,
            completed_turn,
            historical_result.clone(),
            Message::from_part(
                Role::Tool,
                MessagePart::ToolResult {
                    result: historical_result,
                },
            ),
        )
        .expect("historical terminal transition");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            completed_turn,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "completed".to_owned(),
            Some("stop".to_owned()),
            1,
        )
        .expect("historical turn finished");

    let interrupted_turn = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            interrupted_turn,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            2,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("interrupted turn started");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            ToolCallId::new("call-reused"),
            "list".to_owned(),
            json!({ "path": "src" }),
            Some(interrupted_turn),
        )
        .expect("reused tool request");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopened runtime");
    let terminals = reopened
        .all_events(session.session_id)
        .expect("events")
        .into_iter()
        .filter_map(|event| match event.payload {
            EventPayload::ToolExecutionFinished {
                call_id, result, ..
            } if call_id == ToolCallId::new("call-reused") => Some((event.turn_id, result)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 2);
    let recovered = terminals
        .iter()
        .find(|(turn_id, _)| *turn_id == Some(interrupted_turn))
        .expect("recovered terminal for interrupted request");
    assert_eq!(
        recovered.1.output["error"]["code"],
        "tool_outcome_unknown_after_restart"
    );
}

#[test]
fn idle_cancelled_session_stays_idle_for_queue_inspection() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("idle-cancel".to_owned()),
            None,
        )
        .expect("session creation");

    runtime
        .cancel_session(session.session_id, branch.branch_id, "cancel idle session")
        .expect("cancel session");

    let inspection = runtime
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");
    assert_eq!(inspection.runtime_state, SessionRuntimeState::Idle);
    assert!(inspection.cancel_requested);
}

#[test]
fn reopened_runtime_applies_steer_once() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    runtime
        .steer_session(
            session.session_id,
            branch.branch_id,
            "keep going".to_owned(),
        )
        .expect("steer session");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config.clone(), file.path()).expect("reopened");
    assert!(
        reopened
            .apply_pending_steers(&session, &branch)
            .expect("apply steers")
            .is_some()
    );
    drop(reopened);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopened again");
    assert!(
        reopened
            .apply_pending_steers(&session, &branch)
            .expect("apply steers again")
            .is_none()
    );
    let messages = reopened
        .messages(session.session_id, Some(branch.branch_id))
        .expect("messages");
    assert_eq!(messages.len(), 1);
}

#[test]
fn pending_steers_apply_only_to_their_recorded_branch() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("branch-steer".to_owned()),
            None,
        )
        .expect("session creation");
    let child = runtime
        .create_branch(session.session_id, branch.branch_id, None, false, None)
        .expect("create branch");

    runtime
        .steer_session(
            session.session_id,
            branch.branch_id,
            "stay on root".to_owned(),
        )
        .expect("steer root branch");

    assert!(
        runtime
            .apply_pending_steers(&session, &child)
            .expect("apply child steers")
            .is_none()
    );
    let child_messages = runtime
        .messages(session.session_id, Some(child.branch_id))
        .expect("child messages");
    assert!(child_messages.is_empty());

    let dispatch = runtime
        .apply_pending_steers(&session, &branch)
        .expect("apply root steers")
        .expect("root steer dispatch");
    assert_eq!(dispatch.branch_id, branch.branch_id);
    assert_eq!(dispatch.settings_revision_id, session.settings_revision_id);
    let root_messages = runtime
        .messages(session.session_id, Some(branch.branch_id))
        .expect("root messages");
    assert_eq!(root_messages.len(), 1);
}

#[test]
fn branch_from_event_replays_only_history_before_fork_boundary() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("branch-from-event".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .append_message(&session, &branch, Role::User, "first")
        .expect("append first");
    runtime
        .append_message(&session, &branch, Role::Assistant, "second")
        .expect("append second");

    let fork_event_id = runtime
        .all_events(session.session_id)
        .expect("events")
        .into_iter()
        .find(|event| {
            matches!(
                &event.payload,
                EventPayload::MessageAppended { message }
                    if message.parts.iter().any(|part| {
                        matches!(part, MessagePart::Text { text } if text == "first")
                    })
            )
        })
        .expect("first message event")
        .event_id;
    let child = runtime
        .create_branch(
            session.session_id,
            branch.branch_id,
            Some(fork_event_id),
            false,
            None,
        )
        .expect("create branch from event");
    runtime
        .append_message(&session, &child, Role::User, "child")
        .expect("append child");

    let child_messages = runtime
        .messages(session.session_id, Some(child.branch_id))
        .expect("child messages");
    let child_texts = child_messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(child_texts.contains(&"first"));
    assert!(child_texts.contains(&"child"));
    assert!(!child_texts.contains(&"second"));
}

#[test]
fn cancel_consumption_drops_pending_controls_across_restart() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    runtime
        .queue_message(
            session.session_id,
            branch.branch_id,
            Message::text(Role::User, "queued follow-up"),
        )
        .expect("queue message");
    runtime
        .steer_session(
            session.session_id,
            branch.branch_id,
            "keep going".to_owned(),
        )
        .expect("steer session");
    runtime
        .cancel_session(session.session_id, branch.branch_id, "user request")
        .expect("cancel session");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config.clone(), file.path()).expect("reopened");
    assert_eq!(
        reopened
            .consume_post_turn_controls(&session, &branch)
            .expect("consume post turn controls"),
        PostTurnControlAction::Stop
    );
    drop(reopened);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopened again");
    let queue = reopened
        .inspect_queue(session.session_id)
        .expect("queue inspection")
        .expect("queue exists");
    assert!(!queue.cancel_requested);
    assert_eq!(queue.pending_steer_count, 0);
    assert!(queue.queued_messages.is_empty());
}

#[test]
fn current_plan_round_trips_and_new_branch_inherits_it() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    let items = vec![
        PlanItem {
            id: "1".to_owned(),
            content: "Inspect the parser".to_owned(),
            status: PlanStatus::InProgress,
        },
        PlanItem {
            id: "2".to_owned(),
            content: "Add tests".to_owned(),
            status: PlanStatus::Pending,
        },
    ];
    runtime
        .record_plan_updated(session.session_id, branch.branch_id, items.clone(), None)
        .expect("record plan");

    let current = runtime
        .current_plan(session.session_id, branch.branch_id)
        .expect("current plan")
        .expect("plan exists");
    assert_eq!(current.items, items);

    let child_branch = runtime
        .create_branch(session.session_id, branch.branch_id, None, false, None)
        .expect("create child branch");
    let inherited = runtime
        .current_plan(session.session_id, child_branch.branch_id)
        .expect("child branch plan")
        .expect("inherited plan exists");
    assert_eq!(inherited.items, current.items);
}

#[test]
fn session_errors_returns_canonical_errors_latest_last() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            Some("qwen3.5:latest".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    let turn_id = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "qwen3.5:latest".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");
    runtime
        .record_session_error(
            session.session_id,
            branch.branch_id,
            &bt_core::BelltowerError::Provider("first provider failure".to_owned()),
            Some(turn_id),
            bt_core::SpanKind::Llm,
        )
        .expect("first session error");
    runtime
        .record_session_error(
            session.session_id,
            branch.branch_id,
            &bt_core::BelltowerError::Runtime("second runtime failure".to_owned()),
            Some(turn_id),
            bt_core::SpanKind::Agent,
        )
        .expect("second session error");

    let errors = runtime
        .session_errors(session.session_id)
        .expect("session errors")
        .expect("errors exist");

    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].code, "provider_error");
    assert_eq!(errors[0].turn_id, Some(turn_id));
    assert_eq!(errors[0].span_kind, bt_core::SpanKind::Llm);
    assert!(errors[0].retryable);
    assert_eq!(errors[1].code, "runtime_error");
    assert_eq!(errors[1].span_kind, bt_core::SpanKind::Agent);
}

#[test]
fn session_lineage_walks_root_and_child_sessions() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (parent_session, parent_branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("parent".to_owned()),
            None,
        )
        .expect("parent session");
    let (child_session, _) = runtime
        .create_session(
            "/tmp/project/child".into(),
            ConnectionId::new("local"),
            Some("qwen3.5:latest".to_owned()),
            SessionToolMode::Extended,
            Some("child".to_owned()),
            None,
        )
        .expect("child session");
    let child_session = runtime
        .update_session_parent(
            child_session.session_id,
            parent_session.session_id,
            parent_branch.branch_id,
            None,
        )
        .expect("parent update");

    let inspection = runtime
        .session_lineage(child_session.session_id)
        .expect("lineage inspection")
        .expect("lineage exists");

    assert_eq!(inspection.root_session_id, parent_session.session_id);
    assert_eq!(inspection.nodes.len(), 2);
    assert_eq!(
        inspection.nodes[0].session.session_id,
        parent_session.session_id
    );
    assert_eq!(inspection.nodes[0].depth, 0);
    assert_eq!(
        inspection.nodes[1].session.session_id,
        child_session.session_id
    );
    assert_eq!(inspection.nodes[1].depth, 1);
    assert!(inspection.nodes[1].is_focus);
}

#[test]
fn session_workflow_reports_runtime_and_status_counts() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (root_session, root_branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("root".to_owned()),
            None,
        )
        .expect("root session");
    runtime
        .append_message(&root_session, &root_branch, Role::User, "hello")
        .expect("append root message");
    runtime
        .record_turn_started(
            root_session.session_id,
            root_branch.branch_id,
            TurnId::new(),
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            root_session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("root turn started");

    let (working_child, _) = runtime
        .spawn_child_session(
            root_session.session_id,
            root_branch.branch_id,
            None,
            "inspect logs".to_owned(),
            Some("worker".to_owned()),
            None,
            None,
        )
        .expect("spawn child");
    runtime
        .append_message(
            &working_child,
            &runtime
                .default_branch(working_child.session_id)
                .expect("default branch lookup")
                .expect("default branch"),
            Role::User,
            "check recent failures",
        )
        .expect("append child message");
    runtime
        .record_approval_requested(
            working_child.session_id,
            runtime
                .default_branch(working_child.session_id)
                .expect("default branch lookup")
                .expect("default branch")
                .branch_id,
            ToolCallId::new("call-shell"),
            "shell".to_owned(),
            None,
        )
        .expect("approval requested");

    let workflow = runtime
        .session_workflow(working_child.session_id)
        .expect("workflow inspection")
        .expect("workflow exists");

    assert_eq!(workflow.root_session_id, root_session.session_id);
    assert_eq!(workflow.node_count, 2);
    assert_eq!(workflow.runtime_counts.working, 1);
    assert_eq!(workflow.runtime_counts.waiting_on_approval, 1);
    assert_eq!(workflow.status_counts.active, 2);
    assert_eq!(workflow.status_counts.completed, 0);
    assert_eq!(workflow.nodes[0].child_session_count, 1);
    assert!(workflow.nodes[1].is_focus);
}

#[test]
fn spawn_child_session_records_delegation_events_and_derived_origin_turn() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (parent_session, parent_branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("parent".to_owned()),
            Some("investigate repo layout".to_owned()),
        )
        .expect("parent session");

    let turn_id = TurnId::new();
    runtime
        .record_turn_started(
            parent_session.session_id,
            parent_branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            parent_session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");
    runtime
        .record_turn_finished(
            parent_session.session_id,
            parent_branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "completed".to_owned(),
            Some("Stop".to_owned()),
            42,
        )
        .expect("turn finished");

    let (child_session, child_branch) = runtime
        .spawn_child_session(
            parent_session.session_id,
            parent_branch.branch_id,
            None,
            "inspect the tests and summarize risks".to_owned(),
            Some("child".to_owned()),
            None,
            None,
        )
        .expect("spawn child session");

    assert_eq!(
        child_session.parent_session_id,
        Some(parent_session.session_id)
    );
    assert_eq!(
        child_session.parent_branch_id,
        Some(parent_branch.branch_id)
    );
    assert_eq!(child_session.parent_turn_id, Some(turn_id));
    assert_eq!(child_session.connection_id, parent_session.connection_id);
    assert_eq!(child_session.model_id, parent_session.model_id);
    assert_eq!(child_branch.session_id, child_session.session_id);

    let parent_events = runtime
        .all_events(parent_session.session_id)
        .expect("parent events");
    assert!(parent_events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionSpawnRequested {
            child_session_id,
            objective,
            connection_id,
            model_id,
        } if *child_session_id == child_session.session_id
            && objective == "inspect the tests and summarize risks"
            && connection_id == "openai"
            && model_id.as_deref() == Some("o4-mini")
            && event.turn_id == Some(turn_id)
    )));
    assert!(parent_events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionSpawned {
            child_session_id,
            child_branch_id,
            objective,
        } if *child_session_id == child_session.session_id
            && *child_branch_id == child_branch.branch_id
            && objective == "inspect the tests and summarize risks"
            && event.turn_id == Some(turn_id)
    )));

    let child_events = runtime
        .all_events(child_session.session_id)
        .expect("child events");
    assert!(matches!(
        child_events.first().map(|event| &event.payload),
        Some(EventPayload::SessionStarted { .. })
    ));
    assert!(child_events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionHandoffRecorded {
            parent_session_id,
            parent_branch_id,
            parent_turn_id,
            objective,
            summary,
        } if *parent_session_id == parent_session.session_id
            && *parent_branch_id == parent_branch.branch_id
            && *parent_turn_id == Some(turn_id)
            && objective == "inspect the tests and summarize risks"
            && summary == "inspect the tests and summarize risks"
    )));

    let listed = runtime.list_sessions().expect("list sessions");
    assert!(
        listed
            .iter()
            .any(|session| session.session_id == child_session.session_id)
    );
}

#[test]
fn spawn_child_session_accepts_explicit_connection_and_model() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (parent_session, parent_branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("parent".to_owned()),
            Some("coordinate proof review".to_owned()),
        )
        .expect("parent session");

    let (child_session, _child_branch) = runtime
        .spawn_child_session(
            parent_session.session_id,
            parent_branch.branch_id,
            None,
            "try an independent proof search".to_owned(),
            None,
            Some(ConnectionId::new("openai")),
            Some("gpt-5.4-mini".to_owned()),
        )
        .expect("spawn child session");

    assert_eq!(child_session.connection_id, ConnectionId::new("openai"));
    assert_eq!(child_session.model_id.as_deref(), Some("gpt-5.4-mini"));

    let parent_events = runtime
        .all_events(parent_session.session_id)
        .expect("parent events");
    assert!(parent_events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::SessionSpawnRequested {
            child_session_id,
            objective,
            connection_id,
            model_id,
        } if *child_session_id == child_session.session_id
            && objective == "try an independent proof search"
            && connection_id == "openai"
            && model_id.as_deref() == Some("gpt-5.4-mini")
    )));
}

#[test]
fn related_session_wake_uses_child_settings_and_enters_model_context() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (parent, parent_branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("coordinator".to_owned()),
            None,
        )
        .expect("parent session");
    let (child, child_branch) = runtime
        .spawn_child_session(
            parent.session_id,
            parent_branch.branch_id,
            None,
            "seek a counterexample".to_owned(),
            Some("critic".to_owned()),
            Some(ConnectionId::new("openai")),
            Some("gpt-5.4-mini".to_owned()),
        )
        .expect("child session");

    let receipt = runtime
        .send_related_session_message(
            parent.session_id,
            parent_branch.branch_id,
            None,
            child.session_id,
            child_branch.branch_id,
            RelatedSessionMessageKind::Instruction,
            RelatedSessionDeliveryMode::Wake,
            None,
            "Try to disprove the theorem and report the smallest counterexample.".to_owned(),
            Vec::new(),
        )
        .expect("send wake message");
    assert_eq!(receipt.status, RelatedSessionMessageStatus::Pending);

    let admitted = runtime
        .claim_next_related_session_message(child.session_id)
        .expect("claim wake")
        .expect("admitted child turn");
    assert_eq!(admitted.settings_revision_id(), child.settings_revision_id);
    assert!(
        runtime
            .pending_related_session_messages(child.session_id)
            .expect("pending messages")
            .is_empty()
    );
    let messages = runtime
        .related_session_messages(child.session_id)
        .expect("child messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].direction,
        RelatedSessionMessageDirection::Received
    );
    assert_eq!(messages[0].status, RelatedSessionMessageStatus::Claimed);
    assert_eq!(messages[0].resulting_turn_id, Some(admitted.turn_id()));

    let prepared = runtime
        .prepare_turn_context(
            &child,
            &child_branch,
            None,
            Vec::new(),
            None,
            Some(admitted.turn_id()),
            admitted.settings_revision_id(),
        )
        .expect("prepare child context");
    assert_eq!(prepared.model_id, "gpt-5.4-mini");
    assert!(prepared.request.messages.iter().any(|message| {
        message.text_parts().any(|text| {
            text.contains("Try to disprove the theorem and report the smallest counterexample.")
        })
    }));

    runtime
        .send_related_session_message(
            child.session_id,
            child_branch.branch_id,
            Some(admitted.turn_id()),
            parent.session_id,
            parent_branch.branch_id,
            RelatedSessionMessageKind::Progress,
            RelatedSessionDeliveryMode::Notify,
            Some(receipt.message_id),
            "No counterexample below 100.".to_owned(),
            Vec::new(),
        )
        .expect("send progress to parent");
    let parent_messages = runtime
        .related_session_messages(parent.session_id)
        .expect("parent messages");
    assert_eq!(parent_messages.len(), 2);
    assert_eq!(
        parent_messages
            .last()
            .expect("progress message")
            .message
            .text,
        "No counterexample below 100."
    );
}

#[test]
fn reopened_runtime_reports_interrupted_subagent_without_retrying_it() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");
    let (parent, parent_branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("coordinator".to_owned()),
            None,
        )
        .expect("parent session");
    let (child, child_branch) = runtime
        .spawn_child_session(
            parent.session_id,
            parent_branch.branch_id,
            None,
            "run a side-effecting experiment".to_owned(),
            Some("experimenter".to_owned()),
            Some(ConnectionId::new("openai")),
            Some("gpt-5.4-mini".to_owned()),
        )
        .expect("child session");
    let objective = runtime
        .send_related_session_message(
            parent.session_id,
            parent_branch.branch_id,
            None,
            child.session_id,
            child_branch.branch_id,
            RelatedSessionMessageKind::Instruction,
            RelatedSessionDeliveryMode::Wake,
            None,
            "Run the experiment once and report the result.".to_owned(),
            Vec::new(),
        )
        .expect("send objective");
    let admitted = runtime
        .claim_next_related_session_message(child.session_id)
        .expect("claim objective")
        .expect("child turn");
    drop(runtime);

    let reopened = BelltowerRuntime::open(config.clone(), file.path()).expect("reopen runtime");
    assert!(
        reopened
            .all_events(child.session_id)
            .expect("child events")
            .iter()
            .any(|event| matches!(
                &event.payload,
                EventPayload::TurnFinished {
                    turn_id,
                    finish_reason: Some(reason),
                    ..
                } if *turn_id == admitted.turn_id() && reason == "interrupted_after_restart"
            ))
    );
    let interruption = reopened
        .related_session_messages(parent.session_id)
        .expect("parent mailbox")
        .into_iter()
        .find(|record| {
            record.direction == RelatedSessionMessageDirection::Received
                && record.message.in_reply_to == Some(objective.message_id)
                && record.message.kind == RelatedSessionMessageKind::Error
        })
        .expect("interruption notification");
    assert!(
        interruption
            .message
            .text
            .contains("not retried automatically")
    );
    drop(reopened);

    let reopened_again = BelltowerRuntime::open(config, file.path()).expect("second reopen");
    let interruption_count = reopened_again
        .related_session_messages(parent.session_id)
        .expect("parent mailbox")
        .into_iter()
        .filter(|record| {
            record.direction == RelatedSessionMessageDirection::Received
                && record.message.in_reply_to == Some(objective.message_id)
                && record.message.kind == RelatedSessionMessageKind::Error
        })
        .count();
    assert_eq!(interruption_count, 1);
}

#[test]
fn reopened_runtime_repairs_interrupted_subagent_notification_after_terminal_commit() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config.clone(), file.path()).expect("runtime");
    let (parent, parent_branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("coordinator".to_owned()),
            None,
        )
        .expect("parent session");
    let (child, child_branch) = runtime
        .spawn_child_session(
            parent.session_id,
            parent_branch.branch_id,
            None,
            "run a side-effecting experiment".to_owned(),
            Some("experimenter".to_owned()),
            None,
            None,
        )
        .expect("child session");
    let objective = runtime
        .send_related_session_message(
            parent.session_id,
            parent_branch.branch_id,
            None,
            child.session_id,
            child_branch.branch_id,
            RelatedSessionMessageKind::Instruction,
            RelatedSessionDeliveryMode::Wake,
            None,
            "Run the experiment once and report the result.".to_owned(),
            Vec::new(),
        )
        .expect("send objective");
    let admitted = runtime
        .claim_next_related_session_message(child.session_id)
        .expect("claim objective")
        .expect("child turn");
    drop(runtime);

    let mut store = SqliteSessionStore::open(file.path()).expect("store");
    let terminal = EventEnvelope::new(
        child.session_id,
        child_branch.branch_id,
        SpanKind::Agent,
        EventPayload::TurnFinished {
            turn_id: admitted.turn_id(),
            provider: "openai-compatible".to_owned(),
            model: "o4-mini".to_owned(),
            status: "failed".to_owned(),
            finish_reason: Some("interrupted_after_restart".to_owned()),
            latency_ms: 0,
        },
    )
    .with_turn_id(admitted.turn_id());
    store
        .commit_turn_terminal_transition(
            child.session_id,
            child_branch.branch_id,
            admitted.turn_id(),
            &[terminal],
        )
        .expect("terminalize child before notification");
    drop(store);

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopen runtime");
    let notifications = reopened
        .related_session_messages(parent.session_id)
        .expect("parent mailbox")
        .into_iter()
        .filter(|record| {
            record.direction == RelatedSessionMessageDirection::Received
                && record.message.in_reply_to == Some(objective.message_id)
                && record.message.kind == RelatedSessionMessageKind::Error
        })
        .collect::<Vec<_>>();
    assert_eq!(notifications.len(), 1);
    assert!(
        notifications[0]
            .message
            .text
            .contains("not retried automatically")
    );
}

#[test]
fn session_execution_summarizes_tool_approval_flow() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("demo".to_owned()),
            None,
        )
        .expect("session creation");

    let turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-shell");

    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");
    runtime
        .record_completion_requested(
            session.session_id,
            branch.branch_id,
            1,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            turn_id,
        )
        .expect("completion requested");
    runtime
        .record_tool_call_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            json!({"command":"pwd"}),
            Some(turn_id),
        )
        .expect("tool call");
    runtime
        .record_approval_requested(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            Some(turn_id),
        )
        .expect("approval requested");
    runtime
        .record_approval(
            session.session_id,
            branch.branch_id,
            call_id.clone(),
            "shell".to_owned(),
            ApprovalDecision::Approved {
                decided_at: OffsetDateTime::UNIX_EPOCH,
                decided_by: "test".to_owned(),
                scope: ApprovalScope::Once,
                source: bt_core::ApprovalDecisionSource::Runtime,
            },
            Some(turn_id),
        )
        .expect("approval resolved");
    runtime
        .record_tool_execution(
            session.session_id,
            branch.branch_id,
            ToolResultEnvelope {
                call_id: call_id.clone(),
                tool_name: "shell".to_owned(),
                is_error: false,
                output: json!({"stdout":"ok"}),
                duration_ms: Some(10),
            },
            Some(turn_id),
        )
        .expect("tool execution");
    runtime
        .record_completion_requested(
            session.session_id,
            branch.branch_id,
            2,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            2,
            turn_id,
        )
        .expect("follow-up completion requested");
    runtime
        .record_raw_chunk_persisted(
            session.session_id,
            branch.branch_id,
            "openai-compatible".to_owned(),
            1,
            "completion".to_owned(),
            Some(2),
            Some(turn_id),
        )
        .expect("raw chunk");
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai-compatible".to_owned(),
                model: "o4-mini".to_owned(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 12,
                    total_tokens: 22,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: None,
                latency_ms: 123,
            },
            2,
            turn_id,
        )
        .expect("completion finished");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "completed".to_owned(),
            Some("stop".to_owned()),
            123,
        )
        .expect("turn finished");

    let execution = runtime
        .session_execution(session.session_id)
        .expect("execution inspection")
        .expect("execution exists");

    assert_eq!(execution.session_id, session.session_id);
    assert_eq!(execution.turns.len(), 1);
    assert!(execution.event_counts.session >= 1);
    let turn = &execution.turns[0];
    assert_eq!(turn.source, Some(TurnStartSource::UserMessage));
    assert_eq!(turn.resumed_from_call_id, None);
    assert_eq!(turn.llm_call_count, 2);
    assert_eq!(turn.approval_pause_count, 1);
    assert!(!turn.resumed_after_approval);
    assert_eq!(turn.raw_chunk_count, 1);
    assert_eq!(turn.event_counts.tool, 5);
    assert_eq!(
        turn.usage.as_ref().map(|usage| usage.total_tokens),
        Some(22)
    );
    assert!(turn.cost.as_ref().is_some_and(|cost| cost.total_usd > 0.0));
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(
        turn.tool_calls[0].approval_status.as_deref(),
        Some("approved")
    );
    assert_eq!(
        turn.tool_calls[0].execution_status.as_deref(),
        Some("completed")
    );
}

#[test]
fn session_execution_reports_turn_start_provenance() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("execution-provenance".to_owned()),
            None,
        )
        .expect("session creation");

    let turn_id = TurnId::new();
    let call_id = ToolCallId::new("call-resume");
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            2,
            session.settings_revision_id,
            TurnStartSource::ApprovalResume,
            Some(call_id.clone()),
        )
        .expect("turn started");
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            "completed".to_owned(),
            Some("tool_result".to_owned()),
            12,
        )
        .expect("turn finished");

    let execution = runtime
        .session_execution(session.session_id)
        .expect("execution inspection")
        .expect("execution exists");
    let turn = &execution.turns[0];
    assert_eq!(turn.source, Some(TurnStartSource::ApprovalResume));
    assert_eq!(turn.resumed_from_call_id, Some(call_id));
    assert!(turn.resumed_after_approval);
}

#[test]
fn session_execution_includes_projected_context_manifests() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("openai"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("execution-context-manifest".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .append_message(&session, &branch, Role::User, "hello")
        .expect("message");
    let message_event = runtime
        .all_events(session.session_id)
        .expect("events")
        .into_iter()
        .find(|event| matches!(event.payload, EventPayload::MessageAppended { .. }))
        .expect("message event");
    let message_seq_id = message_event.seq_id.expect("message seq id");
    let EventPayload::MessageAppended { message } = message_event.payload else {
        unreachable!("filtered for message event");
    };

    let turn_id = TurnId::new();
    runtime
        .record_turn_started(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "o4-mini".to_owned(),
            1,
            session.settings_revision_id,
            TurnStartSource::UserMessage,
            None,
        )
        .expect("turn started");
    let request = CompletionRequest {
        connection_id: session.connection_id.clone(),
        model: "o4-mini".to_owned(),
        system_prompt: Some("system".to_owned()),
        messages: vec![message],
        tools: Vec::new(),
        structured_output: None,
        max_tokens: None,
        temperature: None,
        thinking: None,
    };
    runtime
        .record_context_manifest(
            session.session_id,
            branch.branch_id,
            ContextManifest::from_completion_request(
                turn_id,
                branch.branch_id,
                1,
                "openai-compatible",
                "o4-mini",
                session.settings_revision_id,
                Some(message_seq_id),
                &request,
                false,
                &[ContextMessageSourceRef {
                    message_id: request.messages[0].message_id,
                    branch_id: branch.branch_id,
                    seq_id: message_seq_id,
                }],
            ),
        )
        .expect("context manifest");

    let execution = runtime
        .session_execution(session.session_id)
        .expect("execution inspection")
        .expect("execution exists");
    let turn = &execution.turns[0];
    assert_eq!(turn.context_manifests.len(), 1);
    let manifest = &turn.context_manifests[0];
    assert_eq!(manifest.turn_id, turn_id);
    assert_eq!(manifest.branch_id, branch.branch_id);
    assert_eq!(manifest.context_boundary_seq_id, Some(message_seq_id));
    assert_eq!(
        manifest.messages[0].source_branch_id,
        Some(branch.branch_id)
    );
    assert_eq!(manifest.messages[0].source_seq_id, Some(message_seq_id));
}

#[test]
fn observed_provider_usage_triggers_proactive_compaction_with_window_chain() {
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            Some("qwen3:latest".to_owned()),
            SessionToolMode::Extended,
            Some("proactive-compaction".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .append_message(&session, &branch, Role::User, "seed question")
        .expect("seed question");
    runtime
        .append_message(&session, &branch, Role::Assistant, "seed answer")
        .expect("seed answer");
    let admitted = match runtime
        .admit_user_message(
            &session,
            &branch,
            Message::text(Role::User, "latest request"),
        )
        .expect("admit latest request")
    {
        UserMessageAdmission::Started(admitted) => admitted,
        UserMessageAdmission::Queued { .. } => panic!("idle session should start the turn"),
    };
    // Provider-observed usage from the last real completion: well over the
    // 0.9 * 32768 threshold for qwen3, while the local estimate is tiny.
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai-compatible".to_owned(),
                model: "qwen3:latest".to_owned(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    prompt_tokens: 30_500,
                    completion_tokens: 500,
                    total_tokens: 31_000,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: None,
                latency_ms: 5,
            },
            1,
            admitted.turn_id(),
        )
        .expect("record observed usage");

    let prepared = runtime
        .prepare_turn_context(
            &session,
            &branch,
            None,
            Vec::new(),
            None,
            Some(admitted.turn_id()),
            admitted.settings_revision_id(),
        )
        .expect("prepare turn context");
    let compaction = prepared
        .compaction
        .expect("observed usage beats the small estimate and triggers compaction");
    assert_eq!(compaction.trigger, ContextCompactionTrigger::TokenBudget);
    assert_eq!(compaction.window_number, Some(1));
    assert_eq!(compaction.previous_compaction_id, None);
    assert_eq!(
        compaction.first_compaction_id,
        Some(compaction.compaction_id)
    );
    assert!(compaction.summary_message_id.is_some());
    assert!(compaction.first_kept_message_id.is_some());
    assert_eq!(compaction.first_kept_branch_id, Some(branch.branch_id));

    // A later prepare chains onto the recorded window instead of stacking.
    let second = runtime
        .prepare_turn_context(
            &session,
            &branch,
            None,
            Vec::new(),
            None,
            Some(admitted.turn_id()),
            admitted.settings_revision_id(),
        )
        .expect("prepare again")
        .compaction
        .expect("still over the observed threshold");
    assert_eq!(second.window_number, Some(2));
    assert_eq!(
        second.previous_compaction_id,
        Some(compaction.compaction_id)
    );
    assert_eq!(second.first_compaction_id, Some(compaction.compaction_id));

    let events = runtime.all_events(session.session_id).expect("events");
    let compacted_events = events
        .iter()
        .filter(|event| matches!(event.payload, EventPayload::ContextCompacted { .. }))
        .count();
    assert_eq!(compacted_events, 2);
}

#[test]
fn compaction_trigger_fraction_config_gates_observed_usage() {
    let mut config = BelltowerConfig::from_embedded().expect("config");
    config.context.compaction_trigger_fraction = 1.0;
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            Some("qwen3:latest".to_owned()),
            SessionToolMode::Extended,
            Some("fraction-gate".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .append_message(&session, &branch, Role::User, "seed question")
        .expect("seed question");
    let admitted = match runtime
        .admit_user_message(
            &session,
            &branch,
            Message::text(Role::User, "latest request"),
        )
        .expect("admit latest request")
    {
        UserMessageAdmission::Started(admitted) => admitted,
        UserMessageAdmission::Queued { .. } => panic!("idle session should start the turn"),
    };
    // 31_000 observed tokens sit under a 1.0 fraction of the 32768 window,
    // so the proactive trigger must not fire.
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai-compatible".to_owned(),
                model: "qwen3:latest".to_owned(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    prompt_tokens: 30_500,
                    completion_tokens: 500,
                    total_tokens: 31_000,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: None,
                latency_ms: 5,
            },
            1,
            admitted.turn_id(),
        )
        .expect("record observed usage");

    let prepared = runtime
        .prepare_turn_context(
            &session,
            &branch,
            None,
            Vec::new(),
            None,
            Some(admitted.turn_id()),
            admitted.settings_revision_id(),
        )
        .expect("prepare turn context");
    assert!(
        prepared.compaction.is_none(),
        "raising the trigger fraction must defer proactive compaction"
    );
}

#[test]
fn model_downshift_triggers_pre_turn_compaction_under_new_window() {
    // Downshift handling is pre-turn detection: the provider-observed usage
    // accumulated under the old model is compared against the *new* model's
    // window at the next preflight, so switching to a smaller window forces
    // a compaction before the next completion is built.
    let config = BelltowerConfig::from_embedded().expect("config");
    let file = NamedTempFile::new().expect("tempfile");
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");
    let (session, branch) = runtime
        .create_session(
            "/tmp/project".into(),
            ConnectionId::new("local"),
            Some("o4-mini".to_owned()),
            SessionToolMode::Extended,
            Some("downshift".to_owned()),
            None,
        )
        .expect("session creation");
    runtime
        .append_message(&session, &branch, Role::User, "seed question")
        .expect("seed question");
    let admitted = match runtime
        .admit_user_message(
            &session,
            &branch,
            Message::text(Role::User, "latest request"),
        )
        .expect("admit latest request")
    {
        UserMessageAdmission::Started(admitted) => admitted,
        UserMessageAdmission::Queued { .. } => panic!("idle session should start the turn"),
    };
    // 100k observed tokens: comfortable inside o4-mini's 200k window.
    runtime
        .record_completion_finished(
            session.session_id,
            branch.branch_id,
            CompletionSummary {
                provider: "openai-compatible".to_owned(),
                model: "o4-mini".to_owned(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    prompt_tokens: 99_000,
                    completion_tokens: 1_000,
                    total_tokens: 100_000,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                },
                cost: None,
                latency_ms: 5,
            },
            1,
            admitted.turn_id(),
        )
        .expect("record observed usage");

    let before_downshift = runtime
        .prepare_turn_context(
            &session,
            &branch,
            None,
            Vec::new(),
            None,
            Some(admitted.turn_id()),
            admitted.settings_revision_id(),
        )
        .expect("prepare under the old model");
    assert!(
        before_downshift.compaction.is_none(),
        "100k observed tokens fit the 200k window"
    );

    let updated = runtime
        .update_session_settings(
            session.session_id,
            None,
            Some(Some("qwen3:latest".to_owned())),
            None,
        )
        .expect("downshift to a smaller-window model");

    let after_downshift = runtime
        .prepare_turn_context(
            &session,
            &branch,
            None,
            Vec::new(),
            None,
            Some(admitted.turn_id()),
            updated.settings_revision_id,
        )
        .expect("prepare under the new model");
    let compaction = after_downshift
        .compaction
        .expect("window shrank below the observed usage: compaction must fire");
    assert_eq!(compaction.trigger, ContextCompactionTrigger::TokenBudget);
    assert_eq!(compaction.model.as_deref(), Some("qwen3:latest"));
}
