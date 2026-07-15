// support.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::*;

pub(super) fn to_runtime_page<T>(
    page: TranscriptPage<T>,
    last_seq_id: Option<i64>,
) -> BranchTranscriptPage<T> {
    BranchTranscriptPage {
        items: page.items,
        oldest_seq_id: page.oldest_seq_id,
        newest_seq_id: page.newest_seq_id,
        has_more_before: page.has_more_before,
        last_seq_id,
    }
}

pub(super) fn compaction_report_from_context(
    compaction: bt_context::ContextCompaction,
) -> ContextCompactionReport {
    ContextCompactionReport {
        compaction_id: bt_core::CompactionId::new(),
        trigger: compaction.trigger,
        phase: bt_core::ContextCompactionPhase::PreTurn,
        status: bt_core::ContextCompactionStatus::Completed,
        reason: compaction.reason,
        provider: None,
        model: None,
        context_boundary_seq_id: None,
        source_event_id: None,
        summary_message_id: compaction.summary_message_id,
        first_kept_message_id: compaction.first_kept_message_id,
        first_kept_branch_id: None,
        first_kept_seq_id: None,
        latency_ms: None,
        summary: compaction.summary,
        messages_before: compaction.messages_before,
        messages_after: compaction.messages_after,
        tokens_before: compaction.tokens_before,
        tokens_after: compaction.tokens_after,
        files_read: compaction.files_read,
        files_modified: compaction.files_modified,
    }
}

pub(super) fn rehydrate_approval_state(
    store: &SqliteSessionStore,
    approvals: &ApprovalState,
) -> Result<()> {
    for approval in store.load_reusable_approvals()? {
        approvals.seed_reusable_decision(
            approval.session_id,
            approval.request_fingerprint,
            approval.decision,
        )?;
    }
    Ok(())
}

pub(super) fn combine_steer_messages(messages: impl IntoIterator<Item = String>) -> String {
    messages.into_iter().collect::<Vec<_>>().join("\n\n")
}

pub(super) fn tool_result_message(result: ToolResultEnvelope) -> Message {
    Message::from_part(Role::Tool, MessagePart::ToolResult { result })
}

pub(super) fn lineage_root_session(
    runtime: &BelltowerRuntime,
    mut session: SessionRecord,
) -> Result<SessionRecord> {
    while let Some(parent_session_id) = session.parent_session_id {
        let Some(parent) = runtime.load_session(parent_session_id)? else {
            break;
        };
        session = parent;
    }
    Ok(session)
}

pub(super) fn collect_lineage_nodes(
    runtime: &BelltowerRuntime,
    session: SessionRecord,
    depth: u32,
    focus_session_id: SessionId,
    nodes: &mut Vec<SessionLineageNode>,
) -> Result<()> {
    nodes.push(SessionLineageNode {
        is_focus: session.session_id == focus_session_id,
        depth,
        session: session.clone(),
    });

    let children = runtime
        .store
        .lock()
        .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
        .load_child_sessions(session.session_id)?;
    for child in children {
        collect_lineage_nodes(runtime, child, depth + 1, focus_session_id, nodes)?;
    }
    Ok(())
}

pub(super) fn collect_workflow_nodes(
    runtime: &BelltowerRuntime,
    session: SessionRecord,
    depth: u32,
    focus_session_id: SessionId,
    nodes: &mut Vec<WorkflowSessionNode>,
    runtime_counts: &mut WorkflowRuntimeCounts,
    status_counts: &mut WorkflowStatusCounts,
) -> Result<()> {
    let inspection = runtime
        .inspect_session(session.session_id)?
        .ok_or_else(|| bt_core::BelltowerError::InvalidState("session not found".to_owned()))?;
    let children = runtime
        .store
        .lock()
        .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
        .load_child_sessions(session.session_id)?;

    increment_workflow_runtime_counts(runtime_counts, inspection.runtime_state);
    increment_workflow_status_counts(status_counts, &inspection.session.status);

    nodes.push(WorkflowSessionNode {
        session: inspection.session,
        depth,
        is_focus: session.session_id == focus_session_id,
        runtime_state: inspection.runtime_state,
        active_branch_id: inspection
            .active_branch
            .as_ref()
            .map(|branch| branch.branch_id),
        turn_count: inspection.turn_count,
        message_count: inspection.message_count,
        tool_call_count: inspection.tool_call_count,
        pending_approval_count: inspection.pending_approval_count,
        child_session_count: children.len() as u32,
        last_seq_id: inspection.last_seq_id,
    });

    for child in children {
        collect_workflow_nodes(
            runtime,
            child,
            depth + 1,
            focus_session_id,
            nodes,
            runtime_counts,
            status_counts,
        )?;
    }

    Ok(())
}

pub(super) fn increment_workflow_runtime_counts(
    counts: &mut WorkflowRuntimeCounts,
    state: SessionRuntimeState,
) {
    match state {
        SessionRuntimeState::Idle => counts.idle += 1,
        SessionRuntimeState::Working => counts.working += 1,
        SessionRuntimeState::WaitingOnInput => counts.waiting_on_input += 1,
        SessionRuntimeState::WaitingOnApproval => counts.waiting_on_approval += 1,
        SessionRuntimeState::CancelRequested => counts.cancel_requested += 1,
    }
}

pub(super) fn increment_workflow_status_counts(
    counts: &mut WorkflowStatusCounts,
    status: &SessionStatus,
) {
    match status {
        SessionStatus::Active => counts.active += 1,
        SessionStatus::Completed => counts.completed += 1,
        SessionStatus::Failed => counts.failed += 1,
        SessionStatus::Abandoned => counts.abandoned += 1,
    }
}

pub(super) fn apply_turn_id(event: EventEnvelope, turn_id: Option<TurnId>) -> EventEnvelope {
    match turn_id {
        Some(turn_id) => event.with_turn_id(turn_id),
        None => event,
    }
}

pub(super) fn completion_cost_breakdown(
    provider: &str,
    model: &str,
    usage: &TokenUsage,
) -> Result<Option<CostBreakdown>> {
    if usage.total_tokens == 0 {
        return Ok(None);
    }

    let catalog = BelltowerConfig::pricing_catalog()?;
    let Some(pricing) = best_pricing_entry(&catalog.pricing, provider, model) else {
        return Ok(None);
    };

    let prompt_usd = usage.prompt_tokens as f64 * pricing.prompt_usd_per_million / 1_000_000.0;
    let completion_usd =
        usage.completion_tokens as f64 * pricing.completion_usd_per_million / 1_000_000.0;
    let Some(cache_read_usd) =
        token_class_cost_usd(usage.cache_read_tokens, pricing.cache_read_usd_per_million)
    else {
        return Ok(None);
    };
    let Some(cache_write_usd) = token_class_cost_usd(
        usage.cache_write_tokens,
        pricing.cache_write_usd_per_million,
    ) else {
        return Ok(None);
    };
    let Some(reasoning_usd) =
        token_class_cost_usd(usage.reasoning_tokens, pricing.reasoning_usd_per_million)
    else {
        return Ok(None);
    };
    let token_class_total = cache_read_usd.unwrap_or(0.0)
        + cache_write_usd.unwrap_or(0.0)
        + reasoning_usd.unwrap_or(0.0);

    Ok(Some(CostBreakdown {
        prompt_usd,
        completion_usd,
        total_usd: prompt_usd + completion_usd + token_class_total,
        cache_read_usd,
        cache_write_usd,
        reasoning_usd,
    }))
}

pub(super) fn token_class_cost_usd(
    tokens: Option<u64>,
    usd_per_million: Option<f64>,
) -> Option<Option<f64>> {
    let Some(tokens) = tokens else {
        return Some(None);
    };
    if tokens == 0 {
        return Some(Some(0.0));
    }
    usd_per_million.map(|rate| Some(tokens as f64 * rate / 1_000_000.0))
}

pub(super) fn best_pricing_entry<'a>(
    entries: &'a [PricingEntry],
    provider: &str,
    model: &str,
) -> Option<&'a PricingEntry> {
    entries
        .iter()
        .filter(|entry| entry.provider == provider && model.starts_with(&entry.model_pattern))
        .max_by_key(|entry| entry.model_pattern.len())
}

pub(super) fn payload_turn_id(payload: &EventPayload) -> Option<TurnId> {
    match payload {
        EventPayload::TurnStarted { turn_id, .. } | EventPayload::TurnFinished { turn_id, .. } => {
            Some(*turn_id)
        }
        EventPayload::TurnInstructionProvenanceRecorded { provenance } => Some(provenance.turn_id),
        _ => None,
    }
}

pub(super) fn ensure_turn_summary<'a>(
    turns: &'a mut Vec<TurnInspection>,
    event: &EventEnvelope,
    turn_id: TurnId,
) -> &'a mut TurnInspection {
    if let Some(index) = turns.iter().position(|turn| turn.turn_id == turn_id) {
        return &mut turns[index];
    }

    turns.push(TurnInspection {
        turn_id,
        branch_id: event.branch_id,
        provider: String::new(),
        model: String::new(),
        message_count: 0,
        settings_revision_id: default_settings_revision_id(),
        started_at: event.occurred_at,
        finished_at: None,
        status: None,
        finish_reason: None,
        latency_ms: None,
        event_seq_start: event.seq_id,
        event_seq_end: event.seq_id,
        event_count: 0,
        pending_approval_count: 0,
        raw_chunk_count: 0,
        tool_calls: Vec::new(),
    });
    turns
        .last_mut()
        .expect("turn summary exists immediately after push")
}

pub(super) fn ensure_turn_tool_call(
    turn: &mut TurnInspection,
    call_id: ToolCallId,
    tool_name: String,
) -> &mut TurnToolCallSummary {
    if let Some(index) = turn
        .tool_calls
        .iter()
        .position(|tool| tool.call_id == call_id)
    {
        return &mut turn.tool_calls[index];
    }

    turn.tool_calls.push(TurnToolCallSummary {
        call_id,
        tool_name,
        approval_status: None,
        execution_status: None,
    });
    turn.tool_calls
        .last_mut()
        .expect("tool call exists immediately after push")
}

pub(super) fn reset_turn_tool_call(
    turn: &mut TurnInspection,
    call_id: ToolCallId,
    tool_name: String,
) -> &mut TurnToolCallSummary {
    let tool = ensure_turn_tool_call(turn, call_id, tool_name.clone());
    tool.tool_name = tool_name;
    tool.approval_status = None;
    tool.execution_status = None;
    tool
}

pub(super) fn increment_trace_event_counts(counts: &mut TraceEventCounts, kind: SpanKind) {
    match kind {
        SpanKind::Session => counts.session += 1,
        SpanKind::Agent => counts.agent += 1,
        SpanKind::Llm => counts.llm += 1,
        SpanKind::Tool => counts.tool += 1,
        SpanKind::Chain => counts.chain += 1,
    }
}

pub(super) fn build_trace_turns(
    events: &[EventEnvelope],
) -> (Vec<TraceTurnInspection>, TraceEventCounts) {
    let mut turns: Vec<TraceTurnInspection> = Vec::new();
    let mut event_counts = TraceEventCounts::default();

    for event in events {
        increment_trace_event_counts(&mut event_counts, event.span_kind.clone());

        let Some(turn_id) = event.turn_id.or_else(|| payload_turn_id(&event.payload)) else {
            continue;
        };

        let turn = ensure_trace_turn(&mut turns, event, turn_id);
        turn.event_count += 1;
        if turn.event_seq_start.is_none() {
            turn.event_seq_start = event.seq_id;
        }
        turn.event_seq_end = event.seq_id.or(turn.event_seq_end);
        increment_trace_event_counts(&mut turn.event_counts, event.span_kind.clone());

        match &event.payload {
            EventPayload::TurnStarted {
                provider,
                model,
                message_count: _,
                settings_revision_id,
                source,
                resumed_from_call_id,
                ..
            } => {
                turn.provider = provider.clone();
                turn.model = model.clone();
                turn.settings_revision_id = *settings_revision_id;
                turn.source = Some(source.clone());
                turn.resumed_from_call_id = resumed_from_call_id.clone();
                turn.resumed_after_approval = matches!(source, TurnStartSource::ApprovalResume);
                turn.started_at = event.occurred_at;
                turn.event_seq_start = event.seq_id;
            }
            EventPayload::CompletionRequested {
                llm_call_ordinal: _,
                provider,
                model,
                message_count: _,
            } => {
                if turn.provider.is_empty() {
                    turn.provider = provider.clone();
                }
                if turn.model.is_empty() {
                    turn.model = model.clone();
                }
                turn.llm_call_count += 1;
            }
            EventPayload::CompletionFinished {
                llm_call_ordinal: _,
                provider,
                model,
                usage,
                cost,
                finish_reason,
                latency_ms,
            } => {
                if turn.provider.is_empty() {
                    turn.provider = provider.clone();
                }
                if turn.model.is_empty() {
                    turn.model = model.clone();
                }
                turn.usage = Some(sum_optional_token_usage(turn.usage.as_ref(), usage));
                turn.cost = merge_optional_cost_breakdown(turn.cost.as_ref(), cost.as_ref());
                turn.finish_reason = Some(finish_reason.clone());
                turn.latency_ms = Some(turn.latency_ms.unwrap_or(0) + *latency_ms);
            }
            EventPayload::SessionError { code, .. } => {
                if turn.finish_reason.is_none() {
                    turn.finish_reason = Some(code.clone());
                }
            }
            EventPayload::TurnFinished {
                provider,
                model,
                status,
                finish_reason,
                latency_ms,
                ..
            } => {
                turn.provider = provider.clone();
                turn.model = model.clone();
                turn.status = Some(status.clone());
                turn.finish_reason = finish_reason.clone();
                turn.latency_ms = Some(*latency_ms);
                turn.finished_at = Some(event.occurred_at);
            }
            EventPayload::RawChunkPersisted { .. } => {
                turn.raw_chunk_count += 1;
            }
            EventPayload::ToolCallRequested {
                call_id, tool_name, ..
            } => {
                reset_trace_tool_call(turn, call_id.clone(), tool_name.clone());
            }
            EventPayload::ToolApprovalRequested {
                call_id, tool_name, ..
            } => {
                let tool = ensure_trace_tool_call(turn, call_id.clone(), tool_name.clone());
                tool.approval_status = Some("pending".to_owned());
                turn.approval_pause_count += 1;
            }
            EventPayload::ToolApprovalResolved {
                call_id,
                tool_name,
                decision,
                ..
            } => {
                let tool = ensure_trace_tool_call(turn, call_id.clone(), tool_name.clone());
                tool.approval_status = Some(match decision {
                    ApprovalDecision::Approved { .. } => "approved".to_owned(),
                    ApprovalDecision::Denied { .. } => "denied".to_owned(),
                });
            }
            EventPayload::ToolExecutionFinished {
                call_id,
                tool_name,
                result,
            } => {
                let tool = ensure_trace_tool_call(turn, call_id.clone(), tool_name.clone());
                tool.execution_status = Some(if result.is_error {
                    "error".to_owned()
                } else {
                    "completed".to_owned()
                });
            }
            _ => {}
        }
    }

    (turns, event_counts)
}

pub(super) fn zero_usage() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        cache_read_tokens: Some(0),
        cache_write_tokens: Some(0),
        reasoning_tokens: Some(0),
    }
}

pub(super) fn sum_token_usage(existing: &TokenUsage, next: &TokenUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: existing.prompt_tokens + next.prompt_tokens,
        completion_tokens: existing.completion_tokens + next.completion_tokens,
        total_tokens: existing.total_tokens + next.total_tokens,
        cache_read_tokens: Some(
            existing.cache_read_tokens.unwrap_or(0) + next.cache_read_tokens.unwrap_or(0),
        ),
        cache_write_tokens: Some(
            existing.cache_write_tokens.unwrap_or(0) + next.cache_write_tokens.unwrap_or(0),
        ),
        reasoning_tokens: Some(
            existing.reasoning_tokens.unwrap_or(0) + next.reasoning_tokens.unwrap_or(0),
        ),
    }
}

pub(super) fn sum_optional_token_usage(
    existing: Option<&TokenUsage>,
    next: &TokenUsage,
) -> TokenUsage {
    match existing {
        Some(existing) => sum_token_usage(existing, next),
        None => next.clone(),
    }
}

pub(super) fn merge_optional_cost_breakdown(
    existing: Option<&CostBreakdown>,
    next: Option<&CostBreakdown>,
) -> Option<CostBreakdown> {
    match (existing, next) {
        (Some(existing), Some(next)) => Some(CostBreakdown {
            prompt_usd: existing.prompt_usd + next.prompt_usd,
            completion_usd: existing.completion_usd + next.completion_usd,
            total_usd: existing.total_usd + next.total_usd,
            cache_read_usd: Some(
                existing.cache_read_usd.unwrap_or(0.0) + next.cache_read_usd.unwrap_or(0.0),
            ),
            cache_write_usd: Some(
                existing.cache_write_usd.unwrap_or(0.0) + next.cache_write_usd.unwrap_or(0.0),
            ),
            reasoning_usd: Some(
                existing.reasoning_usd.unwrap_or(0.0) + next.reasoning_usd.unwrap_or(0.0),
            ),
        }),
        (Some(existing), None) => Some(existing.clone()),
        (None, Some(next)) => Some(next.clone()),
        (None, None) => None,
    }
}

pub(super) fn merge_cost_totals(existing: Option<f64>, delta: Option<f64>) -> Option<f64> {
    match (existing, delta) {
        (Some(existing), Some(delta)) => Some(existing + delta),
        (Some(existing), None) => Some(existing),
        (None, Some(delta)) => Some(delta),
        (None, None) => None,
    }
}

pub(super) fn rounded_elapsed_seconds(
    started_at: time::OffsetDateTime,
    finished_at: Option<time::OffsetDateTime>,
) -> u64 {
    let Some(finished_at) = finished_at else {
        return 0;
    };
    let elapsed = finished_at - started_at;
    if elapsed.is_negative() {
        return 0;
    }
    let whole_seconds = elapsed.whole_seconds() as u64;
    if elapsed.subsec_nanoseconds() == 0 {
        whole_seconds
    } else {
        whole_seconds + 1
    }
}

pub(super) fn budget_exhausted(
    budget: &BudgetConfig,
    tokens_used: u64,
    turns_used: u32,
    elapsed_seconds: u64,
    cost_used_usd: Option<f64>,
) -> bool {
    budget
        .max_tokens
        .is_some_and(|max_tokens| tokens_used >= max_tokens)
        || budget
            .max_turns
            .is_some_and(|max_turns| turns_used >= max_turns)
        || budget
            .max_wall_clock_seconds
            .is_some_and(|max_elapsed| elapsed_seconds >= max_elapsed)
        || budget
            .max_cost_usd
            .is_some_and(|max_cost| cost_used_usd.is_none_or(|used| used >= max_cost))
}

pub(super) fn ensure_trace_turn<'a>(
    turns: &'a mut Vec<TraceTurnInspection>,
    event: &EventEnvelope,
    turn_id: TurnId,
) -> &'a mut TraceTurnInspection {
    if let Some(index) = turns.iter().position(|turn| turn.turn_id == turn_id) {
        return &mut turns[index];
    }

    turns.push(TraceTurnInspection {
        turn_id,
        branch_id: event.branch_id,
        settings_revision_id: default_settings_revision_id(),
        source: None,
        resumed_from_call_id: None,
        provider: String::new(),
        model: String::new(),
        started_at: event.occurred_at,
        finished_at: None,
        status: None,
        finish_reason: None,
        latency_ms: None,
        event_seq_start: event.seq_id,
        event_seq_end: event.seq_id,
        event_count: 0,
        raw_chunk_count: 0,
        llm_call_count: 0,
        approval_pause_count: 0,
        resumed_after_approval: false,
        event_counts: TraceEventCounts::default(),
        usage: None,
        cost: None,
        context_manifests: Vec::new(),
        tool_calls: Vec::new(),
    });
    turns
        .last_mut()
        .expect("trace turn exists immediately after push")
}

pub(super) fn ensure_trace_tool_call(
    turn: &mut TraceTurnInspection,
    call_id: ToolCallId,
    tool_name: String,
) -> &mut TurnToolCallSummary {
    if let Some(index) = turn
        .tool_calls
        .iter()
        .position(|tool| tool.call_id == call_id)
    {
        return &mut turn.tool_calls[index];
    }

    turn.tool_calls.push(TurnToolCallSummary {
        call_id,
        tool_name,
        approval_status: None,
        execution_status: None,
    });
    turn.tool_calls
        .last_mut()
        .expect("trace tool call exists immediately after push")
}

pub(super) fn reset_trace_tool_call(
    turn: &mut TraceTurnInspection,
    call_id: ToolCallId,
    tool_name: String,
) -> &mut TurnToolCallSummary {
    let tool = ensure_trace_tool_call(turn, call_id, tool_name.clone());
    tool.tool_name = tool_name;
    tool.approval_status = None;
    tool.execution_status = None;
    tool
}

pub(super) fn branch_depth(
    session_id: SessionId,
    store: &SqliteSessionStore,
    branch: &BranchRecord,
) -> Result<u32> {
    let mut depth = 0u32;
    let mut parent_branch_id = branch.parent_branch_id;
    while let Some(parent_id) = parent_branch_id {
        let Some(parent) = store.load_branch(session_id, parent_id)? else {
            break;
        };
        depth += 1;
        parent_branch_id = parent.parent_branch_id;
    }
    Ok(depth)
}

pub(super) fn latest_message_preview(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        message.parts.iter().find_map(|part| match part {
            MessagePart::Text { text } => Some(truncate_preview(text)),
            MessagePart::ToolCall { call } => Some(format!(
                "tool_call {} {}",
                call.tool_name,
                truncate_preview(&call.arguments.to_string())
            )),
            MessagePart::ToolResult { result } => Some(format!(
                "tool_result {} {}",
                result.tool_name,
                truncate_preview(&result.output.to_string())
            )),
            MessagePart::Reasoning {
                text: Some(text),
                redacted: false,
                ..
            } => Some(format!("[reasoning] {}", truncate_preview(text))),
            MessagePart::Reasoning { redacted: true, .. } => {
                Some("[reasoning redacted]".to_owned())
            }
            MessagePart::Refusal {
                text: Some(text),
                provider_reason,
                ..
            } => Some(truncate_preview(&format!(
                "{} {text}",
                format_refusal_preview(provider_reason.as_deref())
            ))),
            MessagePart::Refusal {
                text: None,
                provider_reason,
                ..
            } => provider_reason
                .as_ref()
                .map(|reason| format!("[refusal] {}", truncate_preview(reason)))
                .or_else(|| Some("[refusal]".to_owned())),
            MessagePart::Structured { schema_name, value } => Some(truncate_preview(
                &format_structured_preview(schema_name.as_deref(), value),
            )),
            MessagePart::Reasoning {
                text: None,
                redacted: false,
                ..
            } => Some("[reasoning]".to_owned()),
        })
    })
}

pub(super) fn format_refusal_preview(provider_reason: Option<&str>) -> String {
    provider_reason
        .map(|reason| format!("[refusal: {reason}]"))
        .unwrap_or_else(|| "[refusal]".to_owned())
}

pub(super) fn format_structured_preview(
    schema_name: Option<&str>,
    value: &serde_json::Value,
) -> String {
    let prefix = schema_name
        .map(|name| format!("[structured: {name}]"))
        .unwrap_or_else(|| "[structured]".to_owned());
    format!("{prefix} {value}")
}

pub(super) fn truncate_preview(raw: &str) -> String {
    const LIMIT: usize = 56;
    let mut chars = raw.chars();
    let truncated = chars.by_ref().take(LIMIT).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
