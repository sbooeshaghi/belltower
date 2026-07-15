// records.rs keeps one bounded runtime concern out of `runtime.rs`; cross-crate ownership remains unchanged.
use super::support::*;
use super::*;

impl BelltowerRuntime {
    pub fn append_message(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        role: Role,
        text: impl Into<String>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended {
                message: Message::text(role, text),
            },
        );
        self.append_event(event)
    }

    pub fn append_raw_message(
        &self,
        session: &SessionRecord,
        branch: &BranchRecord,
        message: Message,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session.session_id,
            branch.branch_id,
            SpanKind::Agent,
            EventPayload::MessageAppended { message },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_session_spawn_requested(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        child_session_id: SessionId,
        objective: String,
        connection_id: String,
        model_id: Option<String>,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Chain,
            EventPayload::SessionSpawnRequested {
                child_session_id,
                objective,
                connection_id,
                model_id,
            },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_session_spawned(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        child_session_id: SessionId,
        child_branch_id: bt_core::BranchId,
        objective: String,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Chain,
            EventPayload::SessionSpawned {
                child_session_id,
                child_branch_id,
                objective,
            },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_session_handoff(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        parent_session_id: SessionId,
        parent_branch_id: bt_core::BranchId,
        parent_turn_id: Option<TurnId>,
        objective: String,
        summary: String,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Chain,
            EventPayload::SessionHandoffRecorded {
                parent_session_id,
                parent_branch_id,
                parent_turn_id,
                objective,
                summary,
            },
        );
        self.append_event(event)
    }

    pub fn record_approval(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        call_id: ToolCallId,
        tool_name: String,
        decision: ApprovalDecision,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let call_id_for_state = call_id.clone();
        let decision_for_state = decision.clone();
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolApprovalResolved {
                call_id,
                tool_name,
                request_fingerprint: None,
                resolution: None,
                decision,
            },
        );
        let event = apply_turn_id(event, turn_id);
        let seq_id = self.append_event(event)?;
        self.approvals
            .record_call(session_id, call_id_for_state, decision_for_state)?;
        Ok(seq_id)
    }

    pub fn record_approval_for_request(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        request: &ApprovalRequest,
        decision: ApprovalDecision,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let decision_for_state = decision.clone();
        let (seq_id, _) =
            self.persist_approval_for_request(session_id, branch_id, request, decision, turn_id)?;
        self.approvals
            .record_for_request(request, decision_for_state)?;
        Ok(seq_id)
    }

    pub(crate) fn record_evaluated_approval_for_request(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        request: &ApprovalRequest,
        decision: ApprovalDecision,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let decision_for_state = decision.clone();
        let (seq_id, request_fingerprint) =
            self.persist_approval_for_request(session_id, branch_id, request, decision, turn_id)?;
        self.approvals.seed_reusable_decision(
            request.session_id,
            request_fingerprint,
            decision_for_state,
        )?;
        Ok(seq_id)
    }

    fn persist_approval_for_request(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        request: &ApprovalRequest,
        decision: ApprovalDecision,
        turn_id: Option<TurnId>,
    ) -> Result<(i64, String)> {
        let request_fingerprint = approval_request_fingerprint(request);
        let resolution = bt_core::ApprovalResolution {
            request_fingerprint: request_fingerprint.clone(),
            decision: decision.clone(),
        };
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolApprovalResolved {
                call_id: request.call_id.clone(),
                tool_name: request.tool_name.clone(),
                request_fingerprint: Some(request_fingerprint.clone()),
                resolution: Some(resolution),
                decision,
            },
        );
        let event = apply_turn_id(event, turn_id);
        let seq_id = self.append_event(event)?;
        Ok((seq_id, request_fingerprint))
    }

    pub fn record_approval_requested(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        call_id: ToolCallId,
        tool_name: String,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolApprovalRequested {
                call_id,
                tool_name,
                snapshot: None,
            },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_approval_requested_for_request(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        request: &ApprovalRequest,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolApprovalRequested {
                call_id: request.call_id.clone(),
                tool_name: request.tool_name.clone(),
                snapshot: Some(request.snapshot(
                    bt_core::ToolOperationInitiator::Agent,
                    "agent_turn",
                    None,
                    None,
                )),
            },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_tool_call_requested(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        call_id: ToolCallId,
        tool_name: String,
        arguments: serde_json::Value,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        self.record_tool_call_requested_with_context(
            session_id,
            branch_id,
            call_id,
            tool_name,
            arguments,
            bt_core::ToolOperationContext::default(),
            turn_id,
        )
    }

    pub fn record_tool_call_requested_with_context(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        call_id: ToolCallId,
        tool_name: String,
        arguments: serde_json::Value,
        operation: bt_core::ToolOperationContext,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        self.record_tool_operation(
            session_id,
            branch_id,
            call_id.clone(),
            tool_name.clone(),
            operation,
            turn_id,
        )?;
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolCallRequested {
                call_id,
                tool_name,
                arguments,
            },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_tool_operation(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        call_id: ToolCallId,
        tool_name: String,
        operation: bt_core::ToolOperationContext,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolOperationRecorded {
                call_id,
                tool_name,
                operation,
            },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_tool_execution(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        result: ToolResultEnvelope,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Tool,
            EventPayload::ToolExecutionFinished {
                call_id: result.call_id.clone(),
                tool_name: result.tool_name.clone(),
                result,
            },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_plan_updated(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        items: Vec<PlanItem>,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Chain,
            EventPayload::PlanUpdated { items },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_context_compacted(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        report: &ContextCompactionReport,
        turn_id: Option<TurnId>,
    ) -> Result<(i64, bt_core::EventId)> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Chain,
            EventPayload::ContextCompacted {
                compaction_id: report.compaction_id,
                trigger: report.trigger,
                phase: report.phase,
                status: report.status,
                reason: report.reason.clone(),
                provider: report.provider.clone(),
                model: report.model.clone(),
                context_boundary_seq_id: report.context_boundary_seq_id,
                summary_message_id: report.summary_message_id,
                first_kept_message_id: report.first_kept_message_id,
                first_kept_branch_id: report.first_kept_branch_id,
                first_kept_seq_id: report.first_kept_seq_id,
                latency_ms: report.latency_ms,
                summary: report.summary.clone(),
                messages_before: report.messages_before,
                messages_after: report.messages_after,
                tokens_before: report.tokens_before,
                tokens_after: report.tokens_after,
                files_read: report.files_read.clone(),
                files_modified: report.files_modified.clone(),
            },
        );
        let event = apply_turn_id(event, turn_id);
        let event_id = event.event_id;
        self.append_event(event).map(|seq_id| (seq_id, event_id))
    }

    pub fn append_raw_chunk(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: Option<TurnId>,
        provider: &str,
        stream_name: &str,
        llm_call_ordinal: Option<u32>,
        content: &[u8],
    ) -> Result<i64> {
        self.store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .append_raw_chunk(
                session_id,
                branch_id,
                turn_id,
                llm_call_ordinal,
                None,
                provider,
                stream_name,
                content,
            )
    }

    pub fn record_raw_chunk(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        provider: String,
        stream: String,
        llm_call_ordinal: Option<u32>,
        content: &[u8],
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let preview_event = apply_turn_id(
            EventEnvelope::new(
                session_id,
                branch_id,
                SpanKind::Llm,
                EventPayload::RawChunkPersisted {
                    provider: provider.clone(),
                    chunk_index: 0,
                    stream: stream.clone(),
                    llm_call_ordinal,
                },
            ),
            turn_id,
        );
        self.ensure_session_started(&preview_event)?;
        self.take_store_append_fault_for_test()?;

        let (chunk_index, mut event, seq_id) = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?
            .append_raw_chunk_with_event(
                session_id,
                branch_id,
                turn_id,
                llm_call_ordinal,
                &provider,
                &stream,
                content,
                |chunk_index| {
                    Ok(apply_turn_id(
                        EventEnvelope::new(
                            session_id,
                            branch_id,
                            SpanKind::Llm,
                            EventPayload::RawChunkPersisted {
                                provider: provider.clone(),
                                chunk_index,
                                stream: stream.clone(),
                                llm_call_ordinal,
                            },
                        ),
                        turn_id,
                    ))
                },
            )?;
        event.seq_id = Some(seq_id);
        bt_otel::mirror_event(&event);
        let _ = self.event_bus.send(event);
        Ok(chunk_index)
    }

    pub fn record_completion_chunk(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        llm_call_ordinal: Option<u32>,
        deltas: Vec<CompletionDelta>,
        raw_chunk_index: Option<i64>,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::CompletionChunk {
                llm_call_ordinal,
                deltas,
                raw_chunk_index,
            },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_raw_chunk_persisted(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        provider: String,
        chunk_index: i64,
        stream: String,
        llm_call_ordinal: Option<u32>,
        turn_id: Option<TurnId>,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::RawChunkPersisted {
                provider,
                chunk_index,
                stream,
                llm_call_ordinal,
            },
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_completion_requested(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        llm_call_ordinal: u32,
        provider: String,
        model: String,
        message_count: u32,
        turn_id: TurnId,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::CompletionRequested {
                llm_call_ordinal,
                provider,
                model,
                message_count,
            },
        );
        let event = event.with_turn_id(turn_id);
        self.append_event(event)
    }

    pub fn record_context_manifest(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        manifest: bt_core::ContextManifest,
    ) -> Result<i64> {
        let turn_id = manifest.turn_id;
        let provider = manifest.provider.clone();
        let model = manifest.model.clone();
        let settings_revision_id = manifest.settings_revision_id;
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::TurnContextManifestRecorded { manifest },
        )
        .with_turn_id(turn_id)
        .with_attribute(
            bt_core::oi_attrs::LLM_PROVIDER,
            serde_json::Value::String(provider),
        )
        .with_attribute(
            bt_core::oi_attrs::LLM_MODEL_NAME,
            serde_json::Value::String(model),
        )
        .with_attribute(
            "settings.revision_id",
            serde_json::Value::Number(settings_revision_id.into()),
        );
        self.append_event(event)
    }

    pub fn record_completion_finished(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        summary: CompletionSummary,
        llm_call_ordinal: u32,
        turn_id: TurnId,
    ) -> Result<i64> {
        let CompletionSummary {
            provider,
            model,
            finish_reason,
            usage,
            cost,
            latency_ms,
        } = summary;
        let cost = match cost {
            Some(cost) => Some(cost),
            None => completion_cost_breakdown(&provider, &model, &usage)?,
        };
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Llm,
            EventPayload::CompletionFinished {
                llm_call_ordinal,
                provider,
                model,
                usage,
                cost,
                finish_reason: format!("{finish_reason:?}"),
                latency_ms,
            },
        );
        let event = event.with_turn_id(turn_id);
        self.append_event(event)
    }

    pub fn record_session_error(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        error: &bt_core::BelltowerError,
        turn_id: Option<TurnId>,
        span_kind: SpanKind,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
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
        );
        let event = apply_turn_id(event, turn_id);
        self.append_event(event)
    }

    pub fn record_turn_started(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        provider: String,
        model: String,
        message_count: u32,
        settings_revision_id: u64,
        source: TurnStartSource,
        resumed_from_call_id: Option<ToolCallId>,
    ) -> Result<i64> {
        let event = self.turn_started_event(
            session_id,
            branch_id,
            turn_id,
            provider,
            model,
            message_count,
            settings_revision_id,
            source,
            resumed_from_call_id,
        );
        self.append_event(event)
    }

    pub fn record_turn_instruction_provenance(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        provenance: bt_core::TurnInstructionProvenance,
    ) -> Result<i64> {
        let turn_id = provenance.turn_id;
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnInstructionProvenanceRecorded {
                provenance: provenance.clone(),
            },
        )
        .with_turn_id(turn_id)
        .with_attribute(
            bt_core::oi_attrs::LLM_PROVIDER,
            serde_json::Value::String(provenance.provider),
        )
        .with_attribute(
            bt_core::oi_attrs::LLM_MODEL_NAME,
            serde_json::Value::String(provenance.model),
        )
        .with_attribute(
            "settings.revision_id",
            serde_json::Value::Number(provenance.settings_revision_id.into()),
        );
        self.append_event(event)
    }

    pub fn record_turn_finished(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        provider: String,
        model: String,
        status: String,
        finish_reason: Option<String>,
        latency_ms: u64,
    ) -> Result<i64> {
        let event = EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnFinished {
                turn_id,
                provider: provider.clone(),
                model: model.clone(),
                status: status.clone(),
                finish_reason: finish_reason.clone(),
                latency_ms,
            },
        )
        .with_turn_id(turn_id)
        .with_attribute(
            bt_core::oi_attrs::LLM_PROVIDER,
            serde_json::Value::String(provider),
        )
        .with_attribute(
            bt_core::oi_attrs::LLM_MODEL_NAME,
            serde_json::Value::String(model),
        )
        .with_attribute("turn.status", serde_json::Value::String(status));
        let event = if let Some(finish_reason) = finish_reason {
            event.with_attribute(
                "turn.finish_reason",
                serde_json::Value::String(finish_reason),
            )
        } else {
            event
        };
        self.append_event(event)
    }

    pub(super) fn turn_started_event(
        &self,
        session_id: SessionId,
        branch_id: bt_core::BranchId,
        turn_id: TurnId,
        provider: String,
        model: String,
        message_count: u32,
        settings_revision_id: u64,
        source: TurnStartSource,
        resumed_from_call_id: Option<ToolCallId>,
    ) -> EventEnvelope {
        EventEnvelope::new(
            session_id,
            branch_id,
            SpanKind::Agent,
            EventPayload::TurnStarted {
                turn_id,
                provider: provider.clone(),
                model: model.clone(),
                message_count,
                settings_revision_id,
                source,
                resumed_from_call_id,
            },
        )
        .with_turn_id(turn_id)
        .with_attribute(
            bt_core::oi_attrs::LLM_PROVIDER,
            serde_json::Value::String(provider),
        )
        .with_attribute(
            bt_core::oi_attrs::LLM_MODEL_NAME,
            serde_json::Value::String(model),
        )
    }

    pub(super) fn append_event(&self, event: EventEnvelope) -> Result<i64> {
        self.append_event_with_store_mutation(event, |store, event| store.append_event(event))
    }

    pub(super) fn append_event_with_store_mutation<F>(
        &self,
        event: EventEnvelope,
        commit: F,
    ) -> Result<i64>
    where
        F: FnOnce(&mut SqliteSessionStore, &EventEnvelope) -> Result<i64>,
    {
        if !matches!(&event.payload, EventPayload::SessionStarted { .. }) {
            self.ensure_session_started(&event)?;
        }
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let seq_id = commit(&mut store, &event)?;
        self.publish_committed_event(event, seq_id);
        Ok(seq_id)
    }

    fn publish_committed_event(&self, mut event: EventEnvelope, seq_id: i64) {
        event.seq_id = Some(seq_id);
        bt_otel::mirror_event(&event);
        let _ = self.event_bus.send(event);
    }

    pub(super) fn append_events(&self, events: Vec<EventEnvelope>) -> Result<Vec<i64>> {
        for event in &events {
            if !matches!(&event.payload, EventPayload::SessionStarted { .. }) {
                self.ensure_session_started(event)?;
            }
        }
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let seq_ids = store.append_events(&events)?;
        for (event, seq_id) in events.into_iter().zip(seq_ids.iter().copied()) {
            self.publish_committed_event(event, seq_id);
        }
        Ok(seq_ids)
    }

    pub(super) fn commit_turn_admission(
        &self,
        session_id: SessionId,
        expected_settings_revision_id: u64,
        cancel_clear_event: EventEnvelope,
        started_events: Vec<EventEnvelope>,
        queued_events: Vec<EventEnvelope>,
    ) -> Result<SessionTurnAdmission> {
        if let Some(event) = started_events.first().or_else(|| queued_events.first()) {
            self.ensure_session_started(event)?;
        }
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let outcome = store.admit_turn_or_queue(
            session_id,
            expected_settings_revision_id,
            &cancel_clear_event,
            &started_events,
            &queued_events,
        )?;
        match &outcome {
            SessionTurnAdmission::Started { seq_ids } => {
                let mut events = Vec::with_capacity(seq_ids.len());
                if seq_ids.len() == started_events.len() + 1 {
                    events.push(cancel_clear_event);
                }
                events.extend(started_events);
                for (event, seq_id) in events.into_iter().zip(seq_ids.iter().copied()) {
                    self.publish_committed_event(event, seq_id);
                }
            }
            SessionTurnAdmission::Queued { seq_ids, .. } => {
                for (event, seq_id) in queued_events.into_iter().zip(seq_ids.iter().copied()) {
                    self.publish_committed_event(event, seq_id);
                }
            }
            SessionTurnAdmission::RetryWithSettings { .. } => {}
        }
        Ok(outcome)
    }

    pub(super) fn commit_queued_continuation(
        &self,
        session_id: SessionId,
        queue_event_id: bt_core::EventId,
        events: Vec<EventEnvelope>,
    ) -> Result<ContinuationClaim> {
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let outcome = store.claim_queued_continuation(session_id, queue_event_id, &events)?;
        if let ContinuationClaim::Claimed { seq_ids } = &outcome {
            for (event, seq_id) in events.into_iter().zip(seq_ids.iter().copied()) {
                self.publish_committed_event(event, seq_id);
            }
        }
        Ok(outcome)
    }

    pub(super) fn commit_steer_continuation(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        steer_event_ids: &[bt_core::EventId],
        events: Vec<EventEnvelope>,
    ) -> Result<ContinuationClaim> {
        self.take_store_append_fault_for_test()?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        let outcome =
            store.claim_steer_continuation(session_id, branch_id, steer_event_ids, &events)?;
        if let ContinuationClaim::Claimed { seq_ids } = &outcome {
            for (event, seq_id) in events.into_iter().zip(seq_ids.iter().copied()) {
                self.publish_committed_event(event, seq_id);
            }
        }
        Ok(outcome)
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Injects a one-shot storage failure for cross-crate invariant tests.
    ///
    /// This is feature-gated so production runtime construction cannot
    /// accidentally bypass the concrete SQLite store path.
    pub fn inject_next_store_append_error_for_test(&self, message: impl Into<String>) {
        *self
            .store_append_fault
            .lock()
            .expect("store append fault lock") =
            Some(bt_core::BelltowerError::Storage(message.into()));
    }

    #[cfg(any(test, feature = "test-support"))]
    fn take_store_append_fault_for_test(&self) -> Result<()> {
        match self
            .store_append_fault
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("fault lock poisoned".to_owned()))?
            .take()
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[cfg(not(any(test, feature = "test-support")))]
    fn take_store_append_fault_for_test(&self) -> Result<()> {
        Ok(())
    }

    fn ensure_session_started(&self, event: &EventEnvelope) -> Result<()> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| bt_core::BelltowerError::InvalidState("store lock poisoned".to_owned()))?;
        if !store.session_has_events(event.session_id)? {
            let session = store.load_session(event.session_id)?.ok_or_else(|| {
                bt_core::BelltowerError::InvalidState("session not found".to_owned())
            })?;
            let mut started_event = EventEnvelope::new(
                session.session_id,
                event.branch_id,
                SpanKind::Session,
                EventPayload::SessionStarted {
                    project_root: session.project_root.to_string(),
                    connection_id: session.connection_id.to_string(),
                },
            );
            started_event.seq_id = Some(store.append_event(&started_event)?);
            bt_otel::mirror_event(&started_event);
            let _ = self.event_bus.send(started_event);
        }
        Ok(())
    }
}
