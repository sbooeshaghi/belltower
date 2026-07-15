// control_plane.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;

impl BelltowerRuntime {
    pub fn cancel_session(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        reason: impl Into<String>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::SessionCancelled {
                reason: reason.into(),
            },
        );
        self.append_event(event)
    }

    pub fn steer_session(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        message: String,
    ) -> Result<i64> {
        let settings_revision_id = self.current_settings_revision(session_id)?;
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::SessionSteered {
                message,
                settings_revision_id,
            },
        );
        self.append_event(event)
    }

    pub fn record_operator_command(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        command_type: String,
        raw_input: String,
        output: String,
        success: bool,
    ) -> Result<i64> {
        self.append_event(operator_command_event(
            session_id,
            branch_id,
            command_type,
            raw_input,
            output,
            success,
        ))
    }

    pub fn bootstrap_resumed_approval_turn(
        &self,
        resumable: &ResumableToolCall,
        approval_request: &ApprovalRequest,
        decision: ApprovalDecision,
    ) -> Result<AdmittedTurn> {
        let turn_id = TurnId::new();
        let (connection, model_id) = self
            .resolve_turn_settings(resumable.session.session_id, resumable.settings_revision_id)?;
        let current_message_count = self
            .messages(
                resumable.session.session_id,
                Some(resumable.branch.branch_id),
            )?
            .len();
        let pending_steers = self.load_pending_steers_for_branch(
            resumable.session.session_id,
            resumable.branch.branch_id,
        )?;
        let combined_steer = (!pending_steers.is_empty()).then(|| {
            combine_steer_messages(pending_steers.iter().map(|steer| steer.message.clone()))
        });
        let message_count = current_message_count as u32 + 1 + u32::from(combined_steer.is_some());
        let mut events = vec![
            self.turn_started_event(
                resumable.session.session_id,
                resumable.branch.branch_id,
                turn_id,
                connection.provider.clone(),
                model_id,
                message_count,
                resumable.settings_revision_id,
                TurnStartSource::ApprovalResume,
                Some(approval_request.call_id.clone()),
            ),
            apply_turn_id(
                EventEnvelope::new(
                    resumable.session.session_id,
                    resumable.branch.branch_id,
                    SpanKind::Tool,
                    EventPayload::ToolApprovalResolved {
                        call_id: approval_request.call_id.clone(),
                        tool_name: approval_request.tool_name.clone(),
                        request_fingerprint: Some(approval_request_fingerprint(approval_request)),
                        resolution: Some(bt_core::ApprovalResolution::from_request(
                            approval_request,
                            decision.clone(),
                        )),
                        decision: decision.clone(),
                    },
                ),
                Some(turn_id),
            ),
        ];
        self.append_resumed_control_events(
            &mut events,
            resumable.session.session_id,
            resumable.branch.branch_id,
            turn_id,
            &pending_steers,
            combined_steer,
        );
        require_resumed_continuation_claim(self.commit_resumed_continuation(
            resumable.session.session_id,
            resumable.branch.branch_id,
            &approval_request.call_id,
            &approval_request.tool_name,
            resumable.turn_id,
            resumable.requested_seq_id,
            resumable.settings_revision_id,
            bt_session::ResumedContinuationKind::Approval,
            events,
        )?)?;
        self.approvals.seed_reusable_decision(
            approval_request.session_id,
            approval_request_fingerprint(approval_request),
            decision,
        )?;
        Ok(AdmittedTurn::new(
            resumable.session.session_id,
            resumable.branch.branch_id,
            turn_id,
            resumable.settings_revision_id,
        ))
    }

    pub fn record_tool_terminal_transition(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        result: ToolResultEnvelope,
        message: Message,
    ) -> Result<()> {
        self.append_active_turn_events(
            session_id,
            branch_id,
            turn_id,
            tool_terminal_events(session_id, branch_id, turn_id, result, message),
        )?;
        Ok(())
    }

    pub fn record_tool_request_transition(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        call_id: ToolCallId,
        tool_name: String,
        arguments: serde_json::Value,
        operation: bt_core::ToolOperationContext,
        message: Message,
    ) -> Result<()> {
        self.append_active_turn_events(
            session_id,
            branch_id,
            turn_id,
            tool_request_events(
                session_id, branch_id, turn_id, call_id, tool_name, arguments, operation, message,
            ),
        )?;
        Ok(())
    }

    pub fn record_tool_operation_request_transition(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        call_id: ToolCallId,
        tool_name: String,
        arguments: serde_json::Value,
        operation: bt_core::ToolOperationContext,
        turn_id: Option<TurnId>,
    ) -> Result<()> {
        self.append_events(tool_operation_request_events(
            session_id, branch_id, call_id, tool_name, arguments, operation, turn_id,
        ))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_operator_tool_terminal_transition(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        result: ToolResultEnvelope,
        command_type: String,
        raw_input: String,
        output: String,
        success: bool,
    ) -> Result<()> {
        self.append_events(vec![
            tool_execution_finished_event(session_id, branch_id, None, result),
            operator_command_event(
                session_id,
                branch_id,
                command_type,
                raw_input,
                output,
                success,
            ),
        ])?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn recover_operator_tool_terminal_transition(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        result: ToolResultEnvelope,
        command_type: String,
        raw_input: String,
        output: String,
        success: bool,
        persistence_error: &bt_core::BelltowerError,
    ) -> Result<()> {
        self.append_events(vec![
            tool_execution_finished_event(session_id, branch_id, None, result),
            operator_command_event(
                session_id,
                branch_id,
                command_type,
                raw_input,
                output,
                success,
            ),
            session_error_event(
                session_id,
                branch_id,
                None,
                SpanKind::Tool,
                persistence_error,
            ),
        ])?;
        Ok(())
    }

    pub fn record_turn_failure_transition(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        provider: &str,
        model_id: &str,
        turn_id: TurnId,
        terminal_results: Vec<ToolResultEnvelope>,
        error: &bt_core::BelltowerError,
        latency_ms: u64,
    ) -> Result<()> {
        self.record_turn_failure_transition_with_reason(
            session_id,
            branch_id,
            provider,
            model_id,
            turn_id,
            terminal_results,
            error,
            error.code(),
            latency_ms,
        )
    }

    pub(super) fn record_turn_failure_transition_with_reason(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        provider: &str,
        model_id: &str,
        turn_id: TurnId,
        terminal_results: Vec<ToolResultEnvelope>,
        error: &bt_core::BelltowerError,
        finish_reason: &str,
        latency_ms: u64,
    ) -> Result<()> {
        let terminal_at = time::OffsetDateTime::now_utc();
        let mut events = terminal_results
            .into_iter()
            .flat_map(|result| {
                let message = tool_result_message(result.clone());
                tool_terminal_events(session_id, branch_id, turn_id, result, message)
            })
            .collect::<Vec<_>>();
        let error_span_kind = match error.class() {
            bt_core::ErrorClass::Provider | bt_core::ErrorClass::Protocol => SpanKind::Llm,
            bt_core::ErrorClass::Tool => SpanKind::Tool,
            _ => SpanKind::Agent,
        };
        events.push(apply_turn_id(
            EventEnvelope::new(
                session_id,
                branch_id,
                error_span_kind,
                EventPayload::SessionError {
                    class: error.class(),
                    code: error.code().to_owned(),
                    message: error.to_string(),
                    retryable: error.retryable(),
                },
            )
            .with_attribute(
                "error.class",
                serde_json::Value::String(error.class().to_string()),
            )
            .with_attribute(
                "error.code",
                serde_json::Value::String(error.code().to_owned()),
            )
            .with_attribute(
                "error.retryable",
                serde_json::Value::Bool(error.retryable()),
            ),
            Some(turn_id),
        ));
        events.push(
            EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Agent,
                EventPayload::TurnFinished {
                    turn_id,
                    provider: provider.to_owned(),
                    model: model_id.to_owned(),
                    status: "failed".to_owned(),
                    finish_reason: Some(finish_reason.to_owned()),
                    latency_ms,
                },
            )
            .with_turn_id(turn_id)
            .with_attribute(
                bt_core::oi_attrs::LLM_PROVIDER,
                serde_json::Value::String(provider.to_owned()),
            )
            .with_attribute(
                bt_core::oi_attrs::LLM_MODEL_NAME,
                serde_json::Value::String(model_id.to_owned()),
            )
            .with_attribute(
                "turn.status",
                serde_json::Value::String("failed".to_owned()),
            )
            .with_attribute(
                "turn.finish_reason",
                serde_json::Value::String(finish_reason.to_owned()),
            ),
        );
        self.commit_active_turn_terminal_events(
            session_id,
            branch_id,
            turn_id,
            terminal_at,
            events,
        )?;
        Ok(())
    }

    /// Records the terminal outcome when an approved, resumed tool call cannot execute.
    pub fn record_resumed_tool_failure(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        connection: &ConnectionDescriptor,
        model_id: &str,
        turn_id: TurnId,
        tool_call: &bt_core::ToolCall,
        error: &bt_core::BelltowerError,
        latency_ms: u64,
    ) -> Result<()> {
        let result = ToolResultEnvelope {
            call_id: ToolCallId::new(tool_call.call_id.clone()),
            tool_name: tool_call.tool_name.clone(),
            is_error: true,
            output: serde_json::json!({
                "error": {
                    "class": error.class().to_string(),
                    "code": error.code(),
                    "message": error.to_string(),
                    "retryable": error.retryable(),
                }
            }),
            duration_ms: Some(latency_ms),
        };
        self.record_turn_failure_transition(
            session.session_id,
            branch.branch_id,
            &connection.provider,
            model_id,
            turn_id,
            vec![result],
            error,
            latency_ms,
        )
    }

    pub fn bootstrap_resumed_input_turn(
        &self,
        resumable: &ResumableToolCall,
        tool_result: ToolResultEnvelope,
    ) -> Result<AdmittedTurn> {
        let turn_id = TurnId::new();
        let (connection, model_id) = self
            .resolve_turn_settings(resumable.session.session_id, resumable.settings_revision_id)?;
        let current_message_count = self
            .messages(
                resumable.session.session_id,
                Some(resumable.branch.branch_id),
            )?
            .len();
        let pending_steers = self.load_pending_steers_for_branch(
            resumable.session.session_id,
            resumable.branch.branch_id,
        )?;
        let combined_steer = (!pending_steers.is_empty()).then(|| {
            combine_steer_messages(pending_steers.iter().map(|steer| steer.message.clone()))
        });
        let message_count = current_message_count as u32 + 1 + u32::from(combined_steer.is_some());
        let mut events = vec![self.turn_started_event(
            resumable.session.session_id,
            resumable.branch.branch_id,
            turn_id,
            connection.provider.clone(),
            model_id,
            message_count,
            resumable.settings_revision_id,
            TurnStartSource::InputResume,
            Some(ToolCallId::new(resumable.tool_call.call_id.clone())),
        )];
        events.extend(tool_terminal_events(
            resumable.session.session_id,
            resumable.branch.branch_id,
            turn_id,
            tool_result.clone(),
            tool_result_message(tool_result),
        ));
        self.append_resumed_control_events(
            &mut events,
            resumable.session.session_id,
            resumable.branch.branch_id,
            turn_id,
            &pending_steers,
            combined_steer,
        );
        require_resumed_continuation_claim(self.commit_resumed_continuation(
            resumable.session.session_id,
            resumable.branch.branch_id,
            &ToolCallId::new(resumable.tool_call.call_id.clone()),
            &resumable.tool_call.tool_name,
            resumable.turn_id,
            resumable.requested_seq_id,
            resumable.settings_revision_id,
            bt_session::ResumedContinuationKind::Input,
            events,
        )?)?;
        Ok(AdmittedTurn::new(
            resumable.session.session_id,
            resumable.branch.branch_id,
            turn_id,
            resumable.settings_revision_id,
        ))
    }

    pub fn queue_message(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        message: Message,
    ) -> Result<usize> {
        let settings_revision_id = self
            .load_session(session_id)?
            .ok_or_else(|| bt_core::BelltowerError::InvalidState("session not found".to_owned()))?
            .settings_revision_id;
        self.append_event(EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::SessionQueuedMessageEnqueued {
                message,
                settings_revision_id,
            },
        ))?;
        Ok(self.load_pending_queued_messages(session_id)?.len())
    }

    pub fn admit_user_message(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        message: Message,
    ) -> Result<UserMessageAdmission> {
        let mut settings_revision_id = session.settings_revision_id;
        for _ in 0..8 {
            let turn_session = self.session_for_settings_revision(session, settings_revision_id)?;
            let (connection, model_id) =
                self.resolve_turn_settings(session.session_id, settings_revision_id)?;
            let turn_id = TurnId::new();
            let message_count = self
                .messages(session.session_id, Some(branch.branch_id))?
                .len()
                .saturating_add(1) as u32;
            let raw_input = render_queue_message_input(&message);
            let cancel_clear_event = EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::SessionCancelCleared {
                    reason: "superseded by direct user message".to_owned(),
                },
            );
            let message_event = EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: message.clone(),
                },
            )
            .with_turn_id(turn_id);
            let turn_started_event = self.turn_started_event(
                turn_session.session_id,
                branch.branch_id,
                turn_id,
                connection.provider,
                model_id,
                message_count,
                settings_revision_id,
                TurnStartSource::UserMessage,
                None,
            );
            let queued_event = EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::SessionQueuedMessageEnqueued {
                    message: message.clone(),
                    settings_revision_id,
                },
            );
            let queued_audit_event = operator_command_event(
                session.session_id,
                branch.branch_id,
                "queued_message".to_owned(),
                raw_input,
                "Queued behind active session work.".to_owned(),
                true,
            );
            match self.commit_turn_admission(
                session.session_id,
                settings_revision_id,
                cancel_clear_event,
                vec![message_event, turn_started_event],
                vec![queued_event, queued_audit_event],
            )? {
                SessionTurnAdmission::Started { .. } => {
                    return Ok(UserMessageAdmission::Started(AdmittedTurn::new(
                        session.session_id,
                        branch.branch_id,
                        turn_id,
                        settings_revision_id,
                    )));
                }
                SessionTurnAdmission::Queued { position, .. } => {
                    return Ok(UserMessageAdmission::Queued { position });
                }
                SessionTurnAdmission::RetryWithSettings {
                    settings_revision_id: current_settings_revision_id,
                } => {
                    settings_revision_id = current_settings_revision_id;
                }
                SessionTurnAdmission::BudgetExhausted => {
                    return Err(bt_core::BelltowerError::Protocol(
                        "session budget exhausted; adjust the session budget before sending more work"
                            .to_owned(),
                    ));
                }
            }
        }
        Err(bt_core::BelltowerError::InvalidState(
            "session settings changed repeatedly during turn admission".to_owned(),
        ))
    }

    pub fn clear_queued_messages(
        &self,
        session_id: SessionId,
        reason: &str,
        audit_output: &str,
        audit_success: bool,
    ) -> Result<Vec<QueuedMessageInspection>> {
        let pending = self.load_pending_queued_messages(session_id)?;
        if pending.is_empty() {
            return Ok(Vec::new());
        }
        let events = pending
            .iter()
            .flat_map(|queued| {
                [
                    EventEnvelope::new(
                        session_id,
                        queued.branch_id,
                        SpanKind::Agent,
                        EventPayload::SessionQueuedMessageResolved {
                            queue_event_id: queued.queue_event_id,
                            outcome: QueuedMessageResolutionOutcome::Dropped,
                            reason: Some(reason.to_owned()),
                        },
                    ),
                    EventEnvelope::new(
                        session_id,
                        queued.branch_id,
                        SpanKind::Agent,
                        EventPayload::OperatorCommandRecorded {
                            command_type: "queue_drop".to_owned(),
                            raw_input: render_queue_message_input(&queued.message),
                            output: audit_output.to_owned(),
                            success: audit_success,
                        },
                    )
                    .with_attribute(
                        bt_core::oi_attrs::INPUT_VALUE,
                        serde_json::Value::String(render_queue_message_input(&queued.message)),
                    )
                    .with_attribute(
                        bt_core::oi_attrs::OUTPUT_VALUE,
                        serde_json::Value::String(audit_output.to_owned()),
                    ),
                ]
            })
            .collect::<Vec<_>>();
        self.append_events(events)?;
        Ok(pending
            .into_iter()
            .map(|queued| QueuedMessageInspection {
                branch_id: queued.branch_id,
                enqueued_at: queued.enqueued_at,
                message: queued.message,
                settings_revision_id: queued.settings_revision_id,
            })
            .collect())
    }

    pub fn clear_cancel_request(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        reason: &str,
    ) -> Result<bool> {
        if !self.is_cancelled(session_id)? {
            return Ok(false);
        }
        self.append_event(EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::SessionCancelCleared {
                reason: reason.to_owned(),
            },
        ))?;
        Ok(true)
    }

    pub fn apply_pending_steers(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
    ) -> Result<Option<QueuedDispatch>> {
        let pending = self.load_pending_steers_for_branch(session.session_id, branch.branch_id)?;
        if pending.is_empty() {
            return Ok(None);
        }
        let settings_revision_id = pending
            .last()
            .map(|steer| steer.settings_revision_id)
            .unwrap_or_else(default_settings_revision_id);
        let combined = combine_steer_messages(pending.iter().map(|steer| steer.message.clone()));
        let steer_event_ids = pending
            .iter()
            .map(|steer| steer.steer_event_id)
            .collect::<Vec<_>>();
        let turn_id = TurnId::new();
        let (connection, model_id) =
            self.resolve_turn_settings(session.session_id, settings_revision_id)?;
        let message_count = self
            .messages(session.session_id, Some(branch.branch_id))?
            .len()
            .saturating_add(1) as u32;
        let events = vec![
            EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::SessionSteersResolved {
                    steer_event_ids: steer_event_ids.clone(),
                    outcome: SteerResolutionOutcome::Applied,
                    combined_message: Some(combined.clone()),
                    reason: None,
                },
            ),
            EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: Message::text(Role::User, combined),
                },
            )
            .with_turn_id(turn_id),
            self.turn_started_event(
                session.session_id,
                branch.branch_id,
                turn_id,
                connection.provider,
                model_id,
                message_count,
                settings_revision_id,
                TurnStartSource::SteerFollowUp,
                None,
            ),
        ];
        match self.commit_steer_continuation(
            session.session_id,
            branch.branch_id,
            &steer_event_ids,
            events,
        )? {
            ContinuationClaim::Claimed { .. } => Ok(Some(QueuedDispatch {
                branch_id: branch.branch_id,
                turn_id,
                settings_revision_id,
            })),
            ContinuationClaim::Busy
            | ContinuationClaim::BudgetExhausted
            | ContinuationClaim::CancelPending
            | ContinuationClaim::Stale => Ok(None),
        }
    }

    pub fn dispatch_next_queued_message(
        &self,
        session: &SessionRecord,
    ) -> Result<Option<QueuedDispatch>> {
        let Some(queued) = self
            .load_pending_queued_messages(session.session_id)?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let branch = self
            .load_branch(session.session_id, queued.branch_id)?
            .ok_or_else(|| {
                bt_core::BelltowerError::InvalidState("queued branch not found".to_owned())
            })?;
        let raw_input = render_queue_message_input(&queued.message);
        let turn_id = TurnId::new();
        let (connection, model_id) =
            self.resolve_turn_settings(session.session_id, queued.settings_revision_id)?;
        let message_count = self
            .messages(session.session_id, Some(queued.branch_id))?
            .len()
            .saturating_add(1) as u32;
        let events = vec![
            EventEnvelope::new(
                session.session_id,
                queued.branch_id,
                SpanKind::Agent,
                EventPayload::SessionQueuedMessageResolved {
                    queue_event_id: queued.queue_event_id,
                    outcome: QueuedMessageResolutionOutcome::Dispatched,
                    reason: Some("Dispatched from the session queue.".to_owned()),
                },
            ),
            EventEnvelope::new(
                session.session_id,
                queued.branch_id,
                SpanKind::Agent,
                EventPayload::OperatorCommandRecorded {
                    command_type: "queue_dispatch".to_owned(),
                    raw_input: raw_input.clone(),
                    output: "Dispatched from the session queue.".to_owned(),
                    success: true,
                },
            )
            .with_attribute(
                bt_core::oi_attrs::INPUT_VALUE,
                serde_json::Value::String(raw_input),
            )
            .with_attribute(
                bt_core::oi_attrs::OUTPUT_VALUE,
                serde_json::Value::String("Dispatched from the session queue.".to_owned()),
            ),
            EventEnvelope::new(
                session.session_id,
                queued.branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended {
                    message: queued.message,
                },
            )
            .with_turn_id(turn_id),
            self.turn_started_event(
                session.session_id,
                branch.branch_id,
                turn_id,
                connection.provider,
                model_id,
                message_count,
                queued.settings_revision_id,
                TurnStartSource::QueuedFollowUp,
                None,
            ),
        ];
        match self.commit_queued_continuation(session.session_id, queued.queue_event_id, events)? {
            ContinuationClaim::Claimed { .. } => Ok(Some(QueuedDispatch {
                branch_id: branch.branch_id,
                turn_id,
                settings_revision_id: queued.settings_revision_id,
            })),
            ContinuationClaim::Busy
            | ContinuationClaim::BudgetExhausted
            | ContinuationClaim::CancelPending
            | ContinuationClaim::Stale => Ok(None),
        }
    }

    pub fn consume_post_turn_controls(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
    ) -> Result<PostTurnControlAction> {
        if self.is_cancelled(session.session_id)? {
            let pending_steers = self.load_pending_steers(session.session_id)?;
            let pending_queue = self.load_pending_queued_messages(session.session_id)?;
            let mut events = vec![EventEnvelope::new(
                session.session_id,
                branch.branch_id,
                SpanKind::Agent,
                EventPayload::SessionCancelCleared {
                    reason: "cancel request consumed after turn completion".to_owned(),
                },
            )];
            if !pending_steers.is_empty() {
                events.push(EventEnvelope::new(
                    session.session_id,
                    branch.branch_id,
                    SpanKind::Agent,
                    EventPayload::SessionSteersResolved {
                        steer_event_ids: pending_steers
                            .iter()
                            .map(|steer| steer.steer_event_id)
                            .collect(),
                        outcome: SteerResolutionOutcome::Dropped,
                        combined_message: None,
                        reason: Some("Dropped because the active turn was cancelled.".to_owned()),
                    },
                ));
            }
            for queued in &pending_queue {
                let raw_input = render_queue_message_input(&queued.message);
                events.push(EventEnvelope::new(
                    session.session_id,
                    queued.branch_id,
                    SpanKind::Agent,
                    EventPayload::SessionQueuedMessageResolved {
                        queue_event_id: queued.queue_event_id,
                        outcome: QueuedMessageResolutionOutcome::Dropped,
                        reason: Some("Dropped because the active turn was cancelled.".to_owned()),
                    },
                ));
                events.push(
                    EventEnvelope::new(
                        session.session_id,
                        queued.branch_id,
                        SpanKind::Agent,
                        EventPayload::OperatorCommandRecorded {
                            command_type: "queue_drop".to_owned(),
                            raw_input: raw_input.clone(),
                            output: "Dropped because the active turn was cancelled.".to_owned(),
                            success: false,
                        },
                    )
                    .with_attribute(
                        bt_core::oi_attrs::INPUT_VALUE,
                        serde_json::Value::String(raw_input),
                    )
                    .with_attribute(
                        bt_core::oi_attrs::OUTPUT_VALUE,
                        serde_json::Value::String(
                            "Dropped because the active turn was cancelled.".to_owned(),
                        ),
                    ),
                );
            }
            self.append_events(events)?;
            return Ok(PostTurnControlAction::Stop);
        }
        if let Some(dispatch) = self.apply_pending_steers(session, branch)? {
            return Ok(PostTurnControlAction::ContinueCurrentBranch(dispatch));
        }
        if let Some(dispatch) = self.dispatch_next_queued_message(session)? {
            return Ok(PostTurnControlAction::ContinueQueuedBranch(dispatch));
        }
        Ok(PostTurnControlAction::Stop)
    }

    pub(super) fn load_session_control(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionControlProjection>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_session_control(session_id)
    }

    pub(super) fn load_pending_queued_messages(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<QueuedMessageProjection>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_pending_queued_messages(session_id)
    }

    pub(super) fn load_pending_steers(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<SteerProjection>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_pending_steers(session_id)
    }

    pub(super) fn load_pending_steers_for_branch(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
    ) -> Result<Vec<SteerProjection>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_pending_steers_for_branch(session_id, branch_id)
    }

    pub(super) fn load_session_settings_snapshot(
        &self,
        session_id: SessionId,
        settings_revision_id: u64,
    ) -> Result<Option<SessionSettingsSnapshot>> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .load_session_settings_snapshot(session_id, settings_revision_id)
    }

    pub(super) fn current_settings_revision(&self, session_id: SessionId) -> Result<u64> {
        Ok(self
            .load_session(session_id)?
            .ok_or_else(|| bt_core::BelltowerError::InvalidState("session not found".to_owned()))?
            .settings_revision_id)
    }

    pub(crate) fn resolve_turn_settings(
        &self,
        session_id: SessionId,
        settings_revision_id: u64,
    ) -> Result<(ConnectionDescriptor, String)> {
        let snapshot = if let Some(snapshot) =
            self.load_session_settings_snapshot(session_id, settings_revision_id)?
        {
            snapshot
        } else {
            let session = self.load_session(session_id)?.ok_or_else(|| {
                bt_core::BelltowerError::InvalidState("session not found".to_owned())
            })?;
            if session.settings_revision_id != settings_revision_id {
                return Err(bt_core::BelltowerError::InvalidState(format!(
                    "settings revision {settings_revision_id} not found for session {session_id}"
                )));
            }
            session.settings_snapshot()
        };
        let connection = self.connection(&snapshot.connection_id).ok_or_else(|| {
            bt_core::BelltowerError::Config(format!(
                "connection `{}` is not configured",
                snapshot.connection_id
            ))
        })?;
        let model_id = snapshot
            .model_id
            .clone()
            .unwrap_or_else(|| connection.default_model.clone());
        Ok((connection, model_id))
    }

    pub fn session_for_settings_revision(
        &self,
        session: &SessionRecord,
        settings_revision_id: u64,
    ) -> Result<SessionRecord> {
        let snapshot = if let Some(snapshot) =
            self.load_session_settings_snapshot(session.session_id, settings_revision_id)?
        {
            snapshot
        } else if session.settings_revision_id == settings_revision_id {
            session.settings_snapshot()
        } else {
            return Err(bt_core::BelltowerError::InvalidState(format!(
                "settings revision {settings_revision_id} not found for session {}",
                session.session_id
            )));
        };
        Ok(session.with_settings_snapshot(&snapshot))
    }

    pub(super) fn settings_revision_for_turn(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Result<u64> {
        self.all_events(session_id)?
            .into_iter()
            .find_map(|event| match event.payload {
                EventPayload::TurnStarted {
                    turn_id: event_turn_id,
                    settings_revision_id,
                    ..
                } if event_turn_id == turn_id => Some(settings_revision_id),
                _ => None,
            })
            .ok_or_else(|| {
                bt_core::BelltowerError::InvalidState(format!(
                    "turn `{turn_id}` is missing turn.started settings revision"
                ))
            })
    }

    pub(super) fn resumable_tool_call(
        &self,
        session_id: SessionId,
        call_id: ToolCallId,
        tool_name: &str,
        kind: ResumableToolCallKind,
    ) -> Result<Option<ResumableToolCall>> {
        let Some(inspection) = self.inspect_tool_call(session_id, call_id.clone())? else {
            return Ok(None);
        };
        if inspection.tool_name != tool_name {
            return Ok(None);
        }

        let is_pending = match kind {
            ResumableToolCallKind::Approval => {
                inspection.approval_status.as_deref() == Some("pending")
            }
            ResumableToolCallKind::Input => {
                inspection.execution_status.as_deref() == Some("requested")
            }
        };
        if !is_pending || inspection.completed_seq_id.is_some() || inspection.result.is_some() {
            return Ok(None);
        }

        let session = self
            .load_session(session_id)?
            .ok_or_else(|| bt_core::BelltowerError::InvalidState("session not found".to_owned()))?;
        let request_identity = {
            let store = self.store.lock().map_err(|_| {
                bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned())
            })?;
            match kind {
                ResumableToolCallKind::Approval => store
                    .load_approvals(session_id)?
                    .into_iter()
                    .find(|approval| {
                        approval.call_id == call_id
                            && approval.tool_name == tool_name
                            && approval.status == "pending"
                    })
                    .map(|approval| {
                        (
                            approval.branch_id,
                            approval.turn_id,
                            approval.requested_seq_id,
                        )
                    }),
                ResumableToolCallKind::Input => store
                    .load_tool_runs(session_id)?
                    .into_iter()
                    .find(|run| {
                        run.call_id == call_id
                            && run.tool_name == tool_name
                            && run.status == "requested"
                    })
                    .map(|run| (run.branch_id, run.turn_id, run.requested_seq_id)),
            }
        }
        .ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "tool call `{call_id}` changed before resume identity could be loaded"
            ))
        })?;
        let branch_id = request_identity.0.ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "tool call `{call_id}` is missing canonical branch metadata"
            ))
        })?;
        let turn_id = request_identity.1.ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "tool call `{call_id}` is missing canonical turn metadata"
            ))
        })?;
        let settings_revision_id = self.settings_revision_for_turn(session_id, turn_id)?;
        let requested_seq_id = request_identity.2.ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "tool call `{call_id}` is missing canonical request sequence metadata"
            ))
        })?;
        let arguments = inspection.arguments.ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "tool call `{call_id}` is missing canonical arguments"
            ))
        })?;
        let branch = self.load_branch(session_id, branch_id)?.ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "branch `{branch_id}` not found for tool call `{call_id}`"
            ))
        })?;
        let session = self.session_for_settings_revision(&session, settings_revision_id)?;

        Ok(Some(ResumableToolCall {
            session,
            branch,
            turn_id,
            requested_seq_id,
            settings_revision_id,
            tool_call: ToolCall {
                tool_name: inspection.tool_name,
                call_id: call_id.to_string(),
                arguments,
            },
            approval_request_snapshot: inspection.approval_request_snapshot,
        }))
    }

    pub(super) fn pending_tool_call_context(
        &self,
        session_id: SessionId,
        call_id: &ToolCallId,
    ) -> Result<PendingToolCallContext> {
        let inspection = self
            .inspect_tool_call(session_id, call_id.clone())?
            .ok_or_else(|| {
                bt_core::BelltowerError::InvalidState(format!(
                    "pending tool call `{call_id}` is missing canonical inspection data"
                ))
            })?;
        let branch_id = inspection.branch_id.ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "pending tool call `{call_id}` is missing canonical branch metadata"
            ))
        })?;
        let turn_id = inspection.turn_id.ok_or_else(|| {
            bt_core::BelltowerError::InvalidState(format!(
                "pending tool call `{call_id}` is missing canonical turn metadata"
            ))
        })?;
        let settings_revision_id = self.settings_revision_for_turn(session_id, turn_id)?;

        Ok(PendingToolCallContext {
            branch_id,
            turn_id,
            settings_revision_id,
        })
    }

    pub(super) fn append_resumed_control_events(
        &self,
        events: &mut Vec<EventEnvelope>,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        pending_steers: &[SteerProjection],
        combined_steer: Option<String>,
    ) {
        if !pending_steers.is_empty() {
            events.push(apply_turn_id(
                EventEnvelope::new(
                    session_id,
                    branch_id,
                    SpanKind::Agent,
                    EventPayload::SessionSteersResolved {
                        steer_event_ids: pending_steers
                            .iter()
                            .map(|steer| steer.steer_event_id)
                            .collect(),
                        outcome: SteerResolutionOutcome::Applied,
                        combined_message: combined_steer.clone(),
                        reason: None,
                    },
                ),
                Some(turn_id),
            ));
            if let Some(combined_steer) = combined_steer {
                events.push(apply_turn_id(
                    EventEnvelope::new(
                        session_id,
                        branch_id,
                        SpanKind::Agent,
                        EventPayload::MessageAppended {
                            message: Message::text(Role::User, combined_steer),
                        },
                    ),
                    Some(turn_id),
                ));
            }
        }
    }

    pub fn is_cancelled(&self, session_id: SessionId) -> Result<bool> {
        Ok(self
            .load_session_control(session_id)?
            .is_some_and(|state| state.cancel_requested))
    }
}

fn require_resumed_continuation_claim(outcome: ContinuationClaim) -> Result<()> {
    match outcome {
        ContinuationClaim::Claimed { .. } => Ok(()),
        ContinuationClaim::BudgetExhausted => Err(bt_core::BelltowerError::Protocol(
            "session budget exhausted; adjust the session budget before resuming work".to_owned(),
        )),
        ContinuationClaim::CancelPending => Err(bt_core::BelltowerError::Protocol(
            "session cancellation is pending; resumed work was not started".to_owned(),
        )),
        ContinuationClaim::Busy => Err(bt_core::BelltowerError::InvalidState(
            "session already has an active turn; resumed work was not started".to_owned(),
        )),
        ContinuationClaim::Stale => Err(bt_core::BelltowerError::InvalidState(
            "pending tool call changed before resumed work could start".to_owned(),
        )),
    }
}

fn tool_terminal_events(
    session_id: SessionId,
    branch_id: bt_core::BranchId,
    turn_id: TurnId,
    result: ToolResultEnvelope,
    message: Message,
) -> Vec<EventEnvelope> {
    vec![
        tool_execution_finished_event(session_id, branch_id, Some(turn_id), result.clone()),
        apply_turn_id(
            EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Agent,
                EventPayload::MessageAppended { message },
            ),
            Some(turn_id),
        ),
    ]
}

fn tool_request_events(
    session_id: SessionId,
    branch_id: bt_core::BranchId,
    turn_id: TurnId,
    call_id: ToolCallId,
    tool_name: String,
    arguments: serde_json::Value,
    operation: bt_core::ToolOperationContext,
    message: Message,
) -> Vec<EventEnvelope> {
    let mut events = tool_operation_request_events(
        session_id,
        branch_id,
        call_id,
        tool_name,
        arguments,
        operation,
        Some(turn_id),
    );
    events.push(apply_turn_id(
        EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended { message },
        ),
        Some(turn_id),
    ));
    events
}

#[allow(clippy::too_many_arguments)]
fn tool_operation_request_events(
    session_id: SessionId,
    branch_id: bt_core::BranchId,
    call_id: ToolCallId,
    tool_name: String,
    arguments: serde_json::Value,
    operation: bt_core::ToolOperationContext,
    turn_id: Option<TurnId>,
) -> Vec<EventEnvelope> {
    vec![
        apply_turn_id(
            EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Tool,
                EventPayload::ToolOperationRecorded {
                    call_id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    operation,
                },
            ),
            turn_id,
        ),
        apply_turn_id(
            EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Tool,
                EventPayload::ToolCallRequested {
                    call_id,
                    tool_name,
                    arguments,
                },
            ),
            turn_id,
        ),
    ]
}

fn tool_execution_finished_event(
    session_id: SessionId,
    branch_id: bt_core::BranchId,
    turn_id: Option<TurnId>,
    result: ToolResultEnvelope,
) -> EventEnvelope {
    apply_turn_id(
        EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolExecutionFinished {
                call_id: result.call_id.clone(),
                tool_name: result.tool_name.clone(),
                result,
            },
        ),
        turn_id,
    )
}

fn session_error_event(
    session_id: SessionId,
    branch_id: bt_core::BranchId,
    turn_id: Option<TurnId>,
    span_kind: SpanKind,
    error: &bt_core::BelltowerError,
) -> EventEnvelope {
    apply_turn_id(
        EventEnvelope::new(
            session_id,
            branch_id,
            span_kind,
            EventPayload::SessionError {
                class: error.class(),
                code: error.code().to_owned(),
                message: error.to_string(),
                retryable: error.retryable(),
            },
        )
        .with_attribute(
            "error.class",
            serde_json::Value::String(error.class().to_string()),
        )
        .with_attribute(
            "error.code",
            serde_json::Value::String(error.code().to_owned()),
        )
        .with_attribute(
            "error.retryable",
            serde_json::Value::Bool(error.retryable()),
        ),
        turn_id,
    )
}

fn operator_command_event(
    session_id: SessionId,
    branch_id: bt_core::BranchId,
    command_type: String,
    raw_input: String,
    output: String,
    success: bool,
) -> EventEnvelope {
    EventEnvelope::new(
        session_id,
        branch_id,
        SpanKind::Agent,
        EventPayload::OperatorCommandRecorded {
            command_type,
            raw_input: raw_input.clone(),
            output: output.clone(),
            success,
        },
    )
    .with_attribute(
        bt_core::oi_attrs::INPUT_VALUE,
        serde_json::Value::String(raw_input),
    )
    .with_attribute(
        bt_core::oi_attrs::OUTPUT_VALUE,
        serde_json::Value::String(output),
    )
}
