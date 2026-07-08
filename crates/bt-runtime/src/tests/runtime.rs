use super::support::completion_cost_breakdown;
use super::{BelltowerRuntime, BudgetEnforcementOutcome, PostTurnControlAction};
use crate::{TurnAdapterFuture, TurnExecutionAdapters, TurnRunRequest};
use bt_core::{
    ApprovalDecision, ApprovalDecisionSource, ApprovalRequest, ApprovalRequirement, ApprovalScope,
    BelltowerConfig, BelltowerError, BudgetConfig, CompletionRequest, CompletionSummary,
    ConnectionDescriptor, ConnectionId, ContextCompactionPhase, ContextCompactionStatus,
    ContextCompactionTrigger, ContextManifest, ContextMessageSourceRef, EventPayload, FinishReason,
    Message, MessagePart, PlanItem, PlanStatus, Role, SessionRuntimeState, SessionToolMode,
    TokenUsage, ToolCallId, ToolDisplayGroup, ToolExecutionMode, ToolInterruptBehavior,
    ToolMetadata, ToolResultEnvelope, ToolRiskClass, TurnId, TurnStartSource,
};
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
    assert_eq!(
        runtime
            .apply_pending_steers(&session, &branch)
            .expect("apply steer"),
        Some(session.settings_revision_id)
    );
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
            if message == "connection `missing` is not configured"
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
            TurnId::new(),
        )
        .expect("record completion");

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
    let runtime = BelltowerRuntime::open(config, file.path()).expect("runtime");

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
    runtime
        .record_turn_finished(
            session.session_id,
            branch.branch_id,
            turn_id,
            "openai-compatible".to_owned(),
            "gpt-5.4-mini".to_owned(),
            "awaiting_input".to_owned(),
            Some("ToolUse".to_owned()),
            1_100,
        )
        .expect("pause turn");
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
    assert_eq!(
        runtime
            .checkpoint_session_budget(session.session_id, branch.branch_id, turn_id)
            .expect("second checkpoint"),
        BudgetEnforcementOutcome::WithinBudget
    );

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
            if message == "connection `missing` is not configured"
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
        .append_message(&session, &branch, Role::User, "latest request")
        .expect("latest message");

    let result = runtime
        .turn_orchestrator()
        .run_session_turns(
            &FailingProviderPreflightAdapters,
            TurnRunRequest {
                session: session.clone(),
                branch: branch.clone(),
                initial_turn_id: Some(TurnId::new()),
                initial_turn_started: false,
                initial_settings_revision_id: session.settings_revision_id,
                initial_source: TurnStartSource::UserMessage,
                resumed_from_call_id: None,
            },
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

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopened runtime");
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

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopened runtime");
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

    let reopened = BelltowerRuntime::open(config, file.path()).expect("reopened runtime");
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

    let applied_revision = runtime
        .apply_pending_steers(&session, &branch)
        .expect("apply root steers");
    assert_eq!(applied_revision, Some(session.settings_revision_id));
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
