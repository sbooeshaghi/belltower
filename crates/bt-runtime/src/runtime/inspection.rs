// inspection.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;

impl BelltowerRuntime {
    pub fn inspect_session(&self, session_id: SessionId) -> Result<Option<SessionInspection>> {
        let store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let Some(session) = store.load_session(session_id)? else {
            return Ok(None);
        };
        let branches = store.load_branches(session_id)?;
        let active_branch = branches.iter().find(|branch| branch.is_default).cloned();
        let metrics = store.load_session_inspection_metrics(session_id)?;
        let cost_summary = store
            .load_cost_summary(session_id)?
            .map(|summary| SessionCostSummary {
                prompt_tokens: summary.prompt_tokens,
                completion_tokens: summary.completion_tokens,
                total_tokens: summary.total_tokens,
                total_cost_usd: summary.total_cost_usd,
                unpriced_completion_count: summary.unpriced_completion_count,
                updated_at: summary.updated_at,
            });
        let budget = store.load_session_budget(session_id)?.map(|projection| {
            let exhausted = budget_exhausted(
                &projection.budget,
                projection.tokens_used,
                projection.turns_used,
                projection.elapsed_seconds,
                projection.cost_used_usd,
            );
            SessionBudgetInspection {
                budget: projection.budget,
                tokens_used: projection.tokens_used,
                turns_used: projection.turns_used,
                elapsed_seconds: projection.elapsed_seconds,
                cost_used_usd: projection.cost_used_usd,
                exhausted,
                updated_at: projection.updated_at,
            }
        });

        let mut related_sessions = Vec::new();
        if let Some(parent_session_id) = session.parent_session_id
            && let Some(parent) = store.load_session(parent_session_id)?
        {
            related_sessions.push(RelatedSessionSummary {
                relation: SessionRelationKind::Parent,
                session_id: parent.session_id,
                display_name: parent.display_name.clone(),
                objective: parent.objective.clone(),
                status: parent.status.clone(),
                connection_id: parent.connection_id.clone(),
                model_id: parent.model_id.clone(),
                origin_branch_id: session.parent_branch_id,
                origin_turn_id: session.parent_turn_id,
                updated_at: parent.updated_at,
            });
        }

        related_sessions.extend(
            store
                .load_child_sessions(session_id)?
                .into_iter()
                .map(|child| RelatedSessionSummary {
                    relation: SessionRelationKind::Child,
                    session_id: child.session_id,
                    display_name: child.display_name.clone(),
                    objective: child.objective.clone(),
                    status: child.status.clone(),
                    connection_id: child.connection_id.clone(),
                    model_id: child.model_id.clone(),
                    origin_branch_id: child.parent_branch_id,
                    origin_turn_id: child.parent_turn_id,
                    updated_at: child.updated_at,
                }),
        );
        drop(store);

        let control = self.load_session_control(session_id)?;
        let pending_steers = self.load_pending_steers(session_id)?;
        let pending_input_count = self.pending_inputs(session_id)?.len() as u32;
        let cancel_requested = control.as_ref().is_some_and(|state| state.cancel_requested);
        let active_turn_in_progress = metrics.active_turn_count > 0;
        let runtime_state = if pending_input_count > 0 {
            SessionRuntimeState::WaitingOnInput
        } else if metrics.pending_approval_count > 0 {
            SessionRuntimeState::WaitingOnApproval
        } else if active_turn_in_progress && cancel_requested {
            SessionRuntimeState::CancelRequested
        } else if active_turn_in_progress {
            SessionRuntimeState::Working
        } else {
            SessionRuntimeState::Idle
        };

        Ok(Some(SessionInspection {
            session,
            active_branch,
            branches,
            runtime_state,
            turn_count: metrics.turn_count,
            message_count: metrics.message_count,
            tool_call_count: metrics.tool_call_count,
            approval_count: metrics.approval_count,
            pending_approval_count: metrics.pending_approval_count,
            pending_input_count,
            raw_chunk_count: metrics.raw_chunk_count,
            last_seq_id: metrics.last_seq_id,
            cancel_requested,
            pending_steer_count: pending_steers.len() as u32,
            related_sessions,
            cost_summary,
            budget,
        }))
    }

    pub fn inspect_queue(&self, session_id: SessionId) -> Result<Option<SessionQueueInspection>> {
        let Some(session_inspection) = self.inspect_session(session_id)? else {
            return Ok(None);
        };

        let pending_approvals = self.pending_approvals(session_id)?;
        let pending_inputs = self.pending_inputs(session_id)?;
        let queued_messages = self.load_pending_queued_messages(session_id)?;

        Ok(Some(SessionQueueInspection {
            session_id,
            runtime_state: session_inspection.runtime_state,
            cancel_requested: session_inspection.cancel_requested,
            pending_steer_count: session_inspection.pending_steer_count,
            queued_messages: queued_messages
                .into_iter()
                .map(|queued| QueuedMessageInspection {
                    branch_id: queued.branch_id,
                    enqueued_at: queued.enqueued_at,
                    message: queued.message,
                    settings_revision_id: queued.settings_revision_id,
                })
                .collect(),
            pending_approvals,
            pending_inputs,
        }))
    }

    pub fn session_errors(
        &self,
        session_id: SessionId,
    ) -> Result<Option<Vec<SessionErrorInspection>>> {
        if self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_session(session_id)?
            .is_none()
        {
            return Ok(None);
        }

        let errors = self
            .all_events(session_id)?
            .into_iter()
            .filter_map(|event| match event.payload {
                EventPayload::SessionError {
                    class,
                    code,
                    message,
                    retryable,
                } => Some(SessionErrorInspection {
                    seq_id: event.seq_id,
                    event_id: event.event_id,
                    branch_id: event.branch_id,
                    turn_id: event.turn_id,
                    span_kind: event.span_kind,
                    occurred_at: event.occurred_at,
                    class,
                    code,
                    message,
                    retryable,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();

        Ok(Some(errors))
    }

    pub fn inspect_tool_call(
        &self,
        session_id: SessionId,
        call_id: ToolCallId,
    ) -> Result<Option<SessionToolCallInspection>> {
        let store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        if store.load_session(session_id)?.is_none() {
            return Ok(None);
        }

        let tool_run = store
            .load_tool_runs(session_id)?
            .into_iter()
            .find(|run| run.call_id == call_id);
        let approval = store
            .load_approvals(session_id)?
            .into_iter()
            .find(|decision| decision.call_id == call_id);
        drop(store);

        let mut branch_id = None;
        let mut turn_id = None;
        let mut requested_seq_id = None;
        let mut completed_seq_id = None;
        let mut requested_at = None;
        let mut completed_at = None;
        let mut event_tool_name = None;

        for event in self.all_events(session_id)? {
            match &event.payload {
                EventPayload::ToolCallRequested {
                    call_id: event_call_id,
                    tool_name,
                    ..
                } if *event_call_id == call_id => {
                    branch_id = Some(event.branch_id);
                    turn_id = event.turn_id;
                    requested_seq_id = event.seq_id;
                    completed_seq_id = None;
                    requested_at = Some(event.occurred_at);
                    completed_at = None;
                    event_tool_name = Some(tool_name.clone());
                }
                EventPayload::ToolApprovalRequested {
                    call_id: event_call_id,
                    tool_name,
                    ..
                }
                | EventPayload::ToolApprovalResolved {
                    call_id: event_call_id,
                    tool_name,
                    ..
                } if *event_call_id == call_id => {
                    branch_id.get_or_insert(event.branch_id);
                    if turn_id.is_none() {
                        turn_id = event.turn_id;
                    }
                    event_tool_name.get_or_insert_with(|| tool_name.clone());
                }
                EventPayload::ToolExecutionFinished {
                    call_id: event_call_id,
                    tool_name,
                    ..
                } if *event_call_id == call_id => {
                    if requested_seq_id.is_none_or(|requested| {
                        event.seq_id.is_some_and(|completed| completed > requested)
                    }) {
                        branch_id.get_or_insert(event.branch_id);
                        if turn_id.is_none() {
                            turn_id = event.turn_id;
                        }
                        completed_seq_id = event.seq_id;
                        completed_at = Some(event.occurred_at);
                        event_tool_name.get_or_insert_with(|| tool_name.clone());
                    }
                }
                _ => {}
            }
        }

        let tool_name = tool_run
            .as_ref()
            .map(|run| run.tool_name.clone())
            .or_else(|| approval.as_ref().map(|approval| approval.tool_name.clone()))
            .or(event_tool_name);

        let Some(tool_name) = tool_name else {
            return Ok(None);
        };

        Ok(Some(SessionToolCallInspection {
            session_id,
            call_id,
            tool_name,
            branch_id,
            turn_id,
            requested_seq_id,
            completed_seq_id,
            requested_at,
            completed_at,
            approval_status: approval.as_ref().map(|approval| approval.status.clone()),
            approval_decision: approval
                .as_ref()
                .and_then(|approval| approval.decision.clone()),
            approval_request_snapshot: approval
                .as_ref()
                .and_then(|approval| approval.request_snapshot.clone()),
            approval_resolution: approval
                .as_ref()
                .and_then(|approval| approval.resolution.clone()),
            approval_updated_at: approval.as_ref().map(|approval| approval.updated_at),
            execution_status: tool_run.as_ref().map(|run| run.status.clone()),
            arguments: tool_run.as_ref().and_then(|run| run.arguments.clone()),
            result: tool_run.as_ref().and_then(|run| run.result.clone()),
        }))
    }

    pub fn branch_for_tool_call(
        &self,
        session_id: SessionId,
        call_id: &ToolCallId,
    ) -> Result<Option<BranchRecord>> {
        let branch_id = self
            .inspect_tool_call(session_id, call_id.clone())?
            .and_then(|inspection| inspection.branch_id);
        branch_id.map_or(Ok(None), |branch_id| {
            self.load_branch(session_id, branch_id)
        })
    }

    pub fn resumable_approval_call(
        &self,
        session_id: SessionId,
        call_id: ToolCallId,
        tool_name: &str,
    ) -> Result<Option<ResumableToolCall>> {
        self.resumable_tool_call(
            session_id,
            call_id,
            tool_name,
            ResumableToolCallKind::Approval,
        )
    }

    pub fn resumable_input_call(
        &self,
        session_id: SessionId,
        call_id: ToolCallId,
    ) -> Result<Option<ResumableToolCall>> {
        self.resumable_tool_call(session_id, call_id, "ask", ResumableToolCallKind::Input)
    }

    pub fn current_plan(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<Option<PlanInspection>> {
        let projection = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_branch_plan(session_id, branch_id)?;
        Ok(projection.map(|plan| PlanInspection {
            session_id: plan.session_id,
            branch_id: plan.branch_id,
            items: plan.items,
            updated_at: plan.updated_at,
        }))
    }

    pub fn pending_approvals(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<PendingApprovalInspection>> {
        let approvals = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_approvals(session_id)?;
        approvals
            .into_iter()
            .filter(|approval| approval.status == "pending")
            .map(|approval| {
                let context = self.pending_tool_call_context(session_id, &approval.call_id)?;
                Ok(PendingApprovalInspection {
                    call_id: approval.call_id,
                    tool_name: approval.tool_name,
                    branch_id: context.branch_id,
                    turn_id: context.turn_id,
                    settings_revision_id: context.settings_revision_id,
                    requested_at: approval.updated_at,
                    request_snapshot: approval.request_snapshot,
                })
            })
            .collect()
    }

    pub fn has_pending_approvals_on_branch(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<bool> {
        Ok(self
            .pending_approvals(session_id)?
            .into_iter()
            .any(|approval| approval.branch_id == branch_id))
    }

    pub fn pending_inputs(&self, session_id: SessionId) -> Result<Vec<PendingInputInspection>> {
        let tool_runs = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_tool_runs(session_id)?;
        tool_runs
            .into_iter()
            .filter(|run| run.tool_name == "ask" && run.status == "requested")
            .filter_map(|run| {
                let prompt = run
                    .arguments
                    .as_ref()
                    .and_then(|arguments| arguments.get("question"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|prompt| !prompt.is_empty())?
                    .to_owned();
                let choices = run
                    .arguments
                    .as_ref()
                    .and_then(|arguments| arguments.get("choices"))
                    .and_then(serde_json::Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(ToOwned::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Some((run.call_id, prompt, choices, run.updated_at))
            })
            .map(|(call_id, prompt, choices, requested_at)| {
                let context = self.pending_tool_call_context(session_id, &call_id)?;
                Ok(PendingInputInspection {
                    call_id,
                    prompt,
                    choices,
                    branch_id: context.branch_id,
                    turn_id: context.turn_id,
                    settings_revision_id: context.settings_revision_id,
                    requested_at,
                })
            })
            .collect()
    }

    pub fn turn_history(&self, session_id: SessionId) -> Result<Vec<TurnInspection>> {
        let events = self.all_events(session_id)?;
        let mut turns: Vec<TurnInspection> = Vec::new();

        for event in &events {
            let Some(turn_id) = event.turn_id.or_else(|| payload_turn_id(&event.payload)) else {
                continue;
            };

            let turn = ensure_turn_summary(&mut turns, event, turn_id);
            turn.event_count += 1;
            if turn.event_seq_start.is_none() {
                turn.event_seq_start = event.seq_id;
            }
            turn.event_seq_end = event.seq_id.or(turn.event_seq_end);

            match &event.payload {
                EventPayload::TurnStarted {
                    provider,
                    model,
                    message_count,
                    settings_revision_id,
                    ..
                } => {
                    turn.provider = provider.clone();
                    turn.model = model.clone();
                    turn.message_count = *message_count;
                    turn.settings_revision_id = *settings_revision_id;
                    turn.started_at = event.occurred_at;
                    turn.event_seq_start = event.seq_id;
                }
                EventPayload::CompletionRequested {
                    llm_call_ordinal: _,
                    provider,
                    model,
                    message_count,
                } => {
                    if turn.provider.is_empty() {
                        turn.provider = provider.clone();
                    }
                    if turn.model.is_empty() {
                        turn.model = model.clone();
                    }
                    if turn.message_count == 0 {
                        turn.message_count = *message_count;
                    }
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
                    reset_turn_tool_call(turn, call_id.clone(), tool_name.clone());
                }
                EventPayload::ToolApprovalRequested {
                    call_id, tool_name, ..
                } => {
                    let tool = ensure_turn_tool_call(turn, call_id.clone(), tool_name.clone());
                    tool.approval_status = Some("pending".to_owned());
                    turn.pending_approval_count += 1;
                }
                EventPayload::ToolApprovalResolved {
                    call_id,
                    tool_name,
                    decision,
                    ..
                } => {
                    let tool = ensure_turn_tool_call(turn, call_id.clone(), tool_name.clone());
                    tool.approval_status = Some(match decision {
                        ApprovalDecision::Approved { .. } => "approved".to_owned(),
                        ApprovalDecision::Denied { .. } => "denied".to_owned(),
                    });
                    if turn.pending_approval_count > 0 {
                        turn.pending_approval_count -= 1;
                    }
                }
                EventPayload::ToolExecutionFinished {
                    call_id,
                    tool_name,
                    result,
                } => {
                    let tool = ensure_turn_tool_call(turn, call_id.clone(), tool_name.clone());
                    tool.execution_status = Some(if result.is_error {
                        "error".to_owned()
                    } else {
                        "completed".to_owned()
                    });
                }
                _ => {}
            }
        }

        Ok(turns)
    }

    pub fn session_execution(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionExecutionInspection>> {
        let Some(inspection) = self.inspect_session(session_id)? else {
            return Ok(None);
        };
        let events = self.all_events(session_id)?;
        let (mut turns, event_counts) = build_trace_turns(&events);
        {
            let store = self.store.lock().map_err(|_| {
                bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned())
            })?;
            for turn in &mut turns {
                turn.context_manifests = store
                    .load_turn_context_manifests(session_id, turn.turn_id)?
                    .into_iter()
                    .map(|projection| projection.manifest)
                    .collect();
            }
        }

        Ok(Some(SessionExecutionInspection {
            session_id: inspection.session.session_id,
            total_events: events.len() as u32,
            event_counts,
            related_sessions: inspection.related_sessions,
            turns,
        }))
    }
}
