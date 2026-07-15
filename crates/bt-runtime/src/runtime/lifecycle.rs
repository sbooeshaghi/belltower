// lifecycle.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;

impl BelltowerRuntime {
    pub fn open(
        config: BelltowerConfig,
        database_path: impl AsRef<std::path::Path>,
    ) -> Result<Self> {
        let mut trace = StartupTrace::from_env("bt-runtime");
        trace.mark("runtime.open.start");
        trace.mark("store.open.start");
        let store = SqliteSessionStore::open(database_path)?;
        trace.mark("store.open.done");
        let (event_bus, _) = broadcast::channel(1024);
        let global_skills = bt_core::config_dir().join("skills");
        let prompt_assets = bt_core::data_dir().join("prompts");
        let approvals = Arc::new(ApprovalState::default());
        trace.mark("approval.rehydrate.start");
        rehydrate_approval_state(&store, &approvals)?;
        trace.mark("approval.rehydrate.done");
        let approval_patterns = config.approval.auto_approve_patterns.clone();
        trace.mark("registries.construct.start");
        let runtime = Self {
            connections: ConnectionRegistry::from_config(&config),
            mcp: McpRegistry::from_config(&config),
            models: LocalModelManager::from_config(&config)?,
            instructions: MarkdownInstructionResolver::new(global_skills, prompt_assets)?,
            config,
            store: Mutex::new(store),
            #[cfg(any(test, feature = "test-support"))]
            store_append_fault: Mutex::new(None),
            approval_evaluator: Arc::new(PolicyApprovalEvaluator::new(
                approvals.clone(),
                approval_patterns,
            )),
            approvals,
            event_bus,
        };
        trace.mark("registries.construct.done");
        trace.mark("turn_recovery.start");
        runtime.recover_interrupted_turns()?;
        trace.mark("turn_recovery.done");
        trace.mark("operator_tool_recovery.start");
        runtime.recover_interrupted_operator_tools()?;
        trace.mark("operator_tool_recovery.done");
        trace.mark("runtime.open.done");
        Ok(runtime)
    }

    fn recover_interrupted_turns(&self) -> Result<()> {
        let interrupted_turns = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_unfinished_turns()?;

        for turn in interrupted_turns {
            let (message, finish_reason) = match turn.source {
                TurnStartSource::ApprovalResume | TurnStartSource::InputResume => (
                    "resumed turn interrupted before completion; recovered as failed during runtime startup",
                    "interrupted_after_resume",
                ),
                _ => (
                    "turn interrupted before completion; recovered as failed during runtime startup",
                    "interrupted_after_restart",
                ),
            };
            let error = bt_core::BelltowerError::Runtime(message.to_owned());
            let events = self
                .store
                .lock()
                .map_err(|_| {
                    bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned())
                })?
                .load_all_events(turn.session_id)?;
            let terminal_results = interrupted_tool_results(&events, &turn);
            self.record_turn_failure_transition_with_reason(
                turn.session_id,
                turn.branch_id,
                &turn.provider,
                &turn.model,
                turn.turn_id,
                terminal_results,
                &error,
                finish_reason,
                0,
            )?;
        }

        Ok(())
    }

    fn recover_interrupted_operator_tools(&self) -> Result<()> {
        let interrupted = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_unfinished_unbound_tool_operations()?;

        for operation in interrupted.into_iter().filter(|operation| {
            operation.operation.initiator == bt_core::ToolOperationInitiator::Human
        }) {
            let result = ToolResultEnvelope {
                call_id: operation.call_id.clone(),
                tool_name: operation.tool_name.clone(),
                is_error: true,
                output: serde_json::json!({
                    "error": {
                        "class": "runtime",
                        "code": "tool_outcome_unknown_after_restart",
                        "message": "runtime restarted after operator tool execution began; the side effect may have occurred and the tool will not be retried automatically",
                        "retryable": false,
                    }
                }),
                duration_ms: None,
            };
            self.append_events(vec![
                EventEnvelope::new(
                    operation.session_id,
                    operation.branch_id,
                    SpanKind::Tool,
                    EventPayload::ToolExecutionFinished {
                        call_id: result.call_id.clone(),
                        tool_name: result.tool_name.clone(),
                        result,
                    },
                ),
                EventEnvelope::new(
                    operation.session_id,
                    operation.branch_id,
                    SpanKind::Session,
                    EventPayload::SessionError {
                        class: bt_core::ErrorClass::Runtime,
                        code: "tool_outcome_unknown_after_restart".to_owned(),
                        message: format!(
                            "operator tool `{}` outcome is unknown after runtime restart",
                            operation.tool_name
                        ),
                        retryable: false,
                    },
                ),
            ])?;
        }

        Ok(())
    }
}

fn interrupted_tool_results(
    events: &[EventEnvelope],
    turn: &bt_session::TurnRecoveryRecord,
) -> Vec<ToolResultEnvelope> {
    #[derive(Clone)]
    struct Candidate {
        call_id: ToolCallId,
        tool_name: String,
        request_seq_id: i64,
        request_turn_id: Option<TurnId>,
    }

    let mut candidates = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolCallRequested {
                call_id, tool_name, ..
            } if event.turn_id == Some(turn.turn_id) => Some(Candidate {
                call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                request_seq_id: event.seq_id?,
                request_turn_id: event.turn_id,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();

    if let Some(resumed_call_id) = &turn.resumed_from_call_id {
        let resumed_request = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ToolCallRequested {
                    call_id, tool_name, ..
                } if call_id == resumed_call_id
                    && event
                        .seq_id
                        .is_some_and(|seq_id| seq_id < turn.started_seq_id) =>
                {
                    Some(Candidate {
                        call_id: call_id.clone(),
                        tool_name: tool_name.clone(),
                        request_seq_id: event.seq_id?,
                        request_turn_id: event.turn_id,
                    })
                }
                _ => None,
            })
            .max_by_key(|candidate| candidate.request_seq_id);
        if let Some(candidate) = resumed_request
            && !candidates
                .iter()
                .any(|existing| existing.request_seq_id == candidate.request_seq_id)
        {
            candidates.push(candidate);
        }
    }

    candidates.sort_by_key(|candidate| candidate.request_seq_id);
    candidates
        .into_iter()
        .filter_map(|candidate| {
            let mut terminal_recorded = false;
            let mut latest_approval_request = None;
            let mut latest_approval_resolution = None;

            for event in events.iter().filter(|event| {
                event
                    .seq_id
                    .is_some_and(|seq_id| seq_id > candidate.request_seq_id)
            }) {
                match &event.payload {
                    EventPayload::ToolExecutionFinished { call_id, .. }
                        if *call_id == candidate.call_id
                            && event.turn_id == Some(turn.turn_id) =>
                    {
                        terminal_recorded = true;
                    }
                    EventPayload::ToolApprovalRequested { call_id, .. }
                        if *call_id == candidate.call_id
                            && matches!(
                                event.turn_id,
                                Some(event_turn_id)
                                    if Some(event_turn_id) == candidate.request_turn_id
                                        || event_turn_id == turn.turn_id
                            ) =>
                    {
                        latest_approval_request = event.seq_id;
                    }
                    EventPayload::ToolApprovalResolved {
                        call_id, decision, ..
                    }
                        if *call_id == candidate.call_id
                            && matches!(
                                event.turn_id,
                                Some(event_turn_id)
                                    if Some(event_turn_id) == candidate.request_turn_id
                                        || event_turn_id == turn.turn_id
                            ) =>
                    {
                        latest_approval_resolution = event
                            .seq_id
                            .map(|seq_id| (seq_id, decision.clone()));
                    }
                    _ => {}
                }
            }

            let approval_pending = latest_approval_request
                .is_some_and(|requested| {
                    latest_approval_resolution
                        .as_ref()
                        .is_none_or(|(resolved, _)| requested > *resolved)
                });
            if terminal_recorded || approval_pending || candidate.tool_name == "ask" {
                return None;
            }
            if latest_approval_resolution
                .as_ref()
                .is_some_and(|(_, decision)| {
                    matches!(decision, bt_core::ApprovalDecision::Denied { .. })
                })
            {
                return Some(ToolResultEnvelope {
                    call_id: candidate.call_id,
                    tool_name: candidate.tool_name.clone(),
                    is_error: true,
                    output: serde_json::json!({
                        "error": format!("invalid state: tool `{}` was denied", candidate.tool_name),
                    }),
                    duration_ms: None,
                });
            }
            Some(ToolResultEnvelope {
                call_id: candidate.call_id,
                tool_name: candidate.tool_name,
                is_error: true,
                output: serde_json::json!({
                    "error": {
                        "class": "runtime",
                        "code": "tool_outcome_unknown_after_restart",
                        "message": "runtime restarted after tool execution began; the side effect may have occurred and the tool will not be retried automatically",
                        "retryable": false,
                    }
                }),
                duration_ms: None,
            })
        })
        .collect()
}
